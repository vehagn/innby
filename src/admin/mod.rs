//! `innby admin`: the host's UI for events, their invitations, group links and
//! headcount, in the site's languages. Served on its own listener behind the internal
//! gateway and a login.

pub mod auth;
mod events;
mod groups;
pub mod jwt;
pub mod lang;
pub mod oidc;
mod qr;
pub mod role;

use std::sync::Arc;

use askama::Template;
use axum::{
    Form, Router,
    extract::{FromRef, Path, Query, State},
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::{AdminConfig, Config},
    db::{
        self,
        event::{self, Event},
        group::{self, Group},
        outbox,
        rsvp::{self, EmailStatus, Headcount, MAX_PARTY_SIZE, NameDisplay, Rsvp, Source},
    },
    i18n::{count, fill, filled},
    link::Purpose,
    paths::{self, AdminLinks, EventLinks},
    web::{self, render, render_with, server_error},
};
use qr as admin_qr;
use role::Role;

#[derive(Clone)]
struct AdminState {
    config: Arc<Config>,
    db: PgPool,
    auth: Arc<auth::Auth>,
}

impl FromRef<AdminState> for Arc<auth::Auth> {
    fn from_ref(state: &AdminState) -> Self {
        Arc::clone(&state.auth)
    }
}

impl FromRef<AdminState> for PgPool {
    fn from_ref(state: &AdminState) -> Self {
        state.db.clone()
    }
}

pub async fn serve(config: Config, admin: AdminConfig, db: PgPool) -> anyhow::Result<()> {
    let address = admin.listen;
    web::listen(address, router(config, &admin, db)).await
}

tokio::task_local! {
    static PREFIX: Arc<str>;
}

/// The path the admin UI is under, such as `/admin`, or empty at the root of its own
/// hostname. Links read it from here, as they read the language.
pub fn prefix() -> Arc<str> {
    PREFIX
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::from(""))
}

pub(crate) fn router(config: Config, admin_config: &AdminConfig, db: PgPool) -> Router {
    // The admin hostname is served the same way as the public one.
    let https = config.is_https();
    let auth = Arc::new(auth::Auth::new(admin_config, https, db.clone()));
    let state = AdminState {
        config: Arc::new(config),
        db,
        auth: Arc::clone(&auth),
    };
    // Each path needs one role for all its methods; see `role` for what each may do.
    let viewer = Router::new()
        .route(paths::HOME, get(events::list))
        .route(paths::EVENT, get(event_guests));
    let editor = Router::new()
        .route(
            paths::EVENT_INVITATIONS,
            get(invitations).post(create_invitations),
        )
        .route(paths::EVENT_GROUPS, get(groups::list).post(groups::create))
        .route(
            paths::EVENT_DETAILS,
            get(events::details).post(events::edit),
        )
        .route(paths::EVENT_CSV, get(export_csv))
        .route(paths::EVENT_REMINDERS, post(queue_reminders))
        .route(paths::GROUP, get(groups::show).post(groups::edit))
        .route(paths::GROUP_REVOKE, post(groups::revoke))
        .route(paths::GROUP_DELETE, post(groups::delete))
        .route(paths::GUEST, get(show).post(edit))
        .route(paths::GUEST_REVOKE, post(revoke))
        .route(paths::GUEST_DELETE, post(delete))
        .route(paths::GUEST_QR, get(guest_qr))
        .route(paths::GROUP_QR, get(groups::qr))
        .route_layer(middleware::from_fn(role::editor));
    let admin = Router::new()
        .route(paths::EVENTS, post(events::create))
        .route(paths::EVENT_DELETE, post(events::delete))
        .route(paths::QR_STYLE, get(qr_style).post(save_qr_style))
        .route_layer(middleware::from_fn(role::admin));
    let mut pages = viewer
        .merge(editor)
        .merge(admin)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth),
            auth::require_login,
        ))
        .route(paths::LOGOUT, post(auth::log_out));
    pages = if admin_config.login.is_some() {
        pages.route(paths::LOGIN, get(auth::login_page).post(auth::log_in))
    } else {
        pages.route(paths::LOGIN, get(auth::login_page))
    };
    if admin_config.oidc.is_some() {
        pages = pages
            .route(paths::LOGIN_OIDC, get(oidc::start))
            .route(paths::LOGIN_OIDC_CALLBACK, get(oidc::callback));
    }
    let prefix: Arc<str> = Arc::from(admin_config.path.as_str());
    let scoped = Arc::clone(&prefix);
    let router: Router = web::no_store(pages)
        // Probes don't log in, and the login page has its style before anyone does.
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(web::ready))
        .route(paths::STYLESHEET, get(web::stylesheet))
        .layer(middleware::from_fn(crate::theme::scope))
        .layer(middleware::from_fn(lang::scope))
        .layer(middleware::from_fn(
            move |request, next: middleware::Next| {
                PREFIX.scope(Arc::clone(&scoped), next.run(request))
            },
        ))
        .with_state(state.clone());
    let router = if prefix.is_empty() {
        router
    } else {
        // Under a path of the site's hostname, as `/admin`. The probes stay at the
        // root, where the kubelet asks.
        let home = paths::admin_under(&prefix, paths::HOME);
        Router::<AdminState>::new()
            .route(paths::HEALTH, get(|| async { "ok" }))
            .route(paths::READY, get(web::ready))
            .with_state(state)
            .nest(&prefix, router)
            .route(
                &format!("{prefix}/"),
                get(move || async move { Redirect::permanent(&home) }),
            )
    };
    web::harden(router, https)
}

/// The admin's sections, as tabs under the header: the events and the QR code style,
/// or, within an event, its own pages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Events,
    Qr,
    Guests,
    Invitations,
    Groups,
    Details,
}

pub struct TabLink {
    pub href: String,
    pub label: &'static str,
    pub current: bool,
}

/// The tabs of the site, or with `event`, that event's tabs.
pub fn tabs(event: Option<&Uuid>, current: Tab) -> Vec<TabLink> {
    let text = lang::text();
    // Only the tabs the host's role may open.
    let tabs = match event {
        None => vec![
            (Tab::Events, paths::admin(paths::HOME), &text.events, true),
            (
                Tab::Qr,
                paths::admin(paths::QR_STYLE),
                &text.qr_heading,
                Role::is_admin(),
            ),
        ],
        Some(&id) => {
            let links = EventLinks::new(id);
            let edit = Role::can_edit();
            vec![
                (Tab::Guests, links.guests, &text.guests, true),
                (
                    Tab::Invitations,
                    links.invitations,
                    &text.invitation_links,
                    edit,
                ),
                (Tab::Groups, links.groups, &text.group_links, edit),
                (Tab::Details, links.details, &text.details, edit),
            ]
        }
    };
    tabs.into_iter()
        .filter(|(_, _, _, allowed)| *allowed)
        .map(|(tab, href, label, _)| (tab, href, label))
        .map(|(tab, href, label)| TabLink {
            href,
            label,
            current: tab == current,
        })
        .collect()
}

/// The event behind an admin path's `{id}`.
async fn load_event(state: &AdminState, id: Uuid) -> Result<Event, PageError> {
    event::find(&state.db, id).await?.ok_or(PageError::Missing)
}

/// One guest as the admin tables show it.
struct Row {
    /// The guest's admin page.
    path: String,
    name: String,
    /// An invitation, or which group link the guest signed up through.
    source: String,
    /// The admin page of that group.
    group_path: Option<String>,
    answer: &'static str,
    /// The answer as a class for its colour: `pending`, `attending` or `declined`.
    status: &'static str,
    party: String,
    email: &'static str,
    /// How other guests see the name in the guest list.
    listed: &'static str,
    responded: String,
    created: String,
}

impl Row {
    fn new(rsvp: &Rsvp, tz: Tz, groups: &[Group]) -> Self {
        let group = groups.iter().find(|group| Some(group.id) == rsvp.group_id);
        let text = lang::text();
        Self {
            path: paths::admin(&paths::path(paths::GUEST, rsvp.id)),
            name: rsvp.name.clone(),
            source: match (rsvp.source, group) {
                (Source::Invite, _) => text.source_invitation.clone(),
                (Source::Open, Some(group)) => filled(&text.source_group, "group", &group.name),
                (Source::Open, None) => text.source_sign_up.clone(),
            },
            group_path: rsvp
                .group_id
                .map(|id| paths::admin(&paths::path(paths::GROUP, id))),
            answer: match rsvp.response {
                rsvp::Response::Pending => &text.answer_pending,
                rsvp::Response::Attending => &text.answer_attending,
                rsvp::Response::Declined => &text.answer_declined,
            },
            status: response_code(rsvp.response),
            party: format!("{} / {}", rsvp.party_size, rsvp.max_party_size),
            email: match rsvp.email_status {
                EmailStatus::None => "",
                EmailStatus::Unconfirmed => &text.email_unconfirmed,
                EmailStatus::Confirmed => &text.email_confirmed,
            },
            listed: match rsvp.listed() {
                NameDisplay::Full => &text.listed_full,
                NameDisplay::Partial => &text.listed_partial,
                NameDisplay::Hidden => &text.listed_hidden,
            },
            responded: rsvp
                .responded_at
                .map(|at| local(at, tz))
                .unwrap_or_default(),
            created: local(rsvp.created_at, tz),
        }
    }
}

fn response_code(response: rsvp::Response) -> &'static str {
    match response {
        rsvp::Response::Pending => "pending",
        rsvp::Response::Attending => "attending",
        rsvp::Response::Declined => "declined",
    }
}

fn local(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

#[derive(Template)]
#[template(path = "admin/index.html")]
struct IndexPage {
    event: Event,
    links: EventLinks,
    headcount: Headcount,
    /// Guests with a confirmed address.
    subscribers: usize,
    /// The guests the table shows: everyone, or those with one answer.
    rows: Vec<Row>,
    /// The answer the table is filtered to, as in `?show=`.
    show: Option<&'static str>,
    notice: Option<String>,
    error: Option<String>,
}

impl IndexPage {
    /// Whether the table shows only the guests with this answer.
    fn showing(&self, code: &str) -> bool {
        self.show == Some(code)
    }

    /// The table with only the guests with this answer.
    fn show_link(&self, code: &str) -> String {
        let page = paths::path(paths::EVENT, self.event.id);
        paths::admin(&format!("{page}?show={code}"))
    }
}

#[derive(Deserialize)]
struct IndexQuery {
    /// `pending`, `attending` or `declined`.
    show: Option<String>,
    reminders: Option<usize>,
    /// The event was just created.
    created: Option<String>,
}

/// The event's guests, with the headcount on top; its figures filter the table.
async fn event_guests(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<IndexQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let (rsvps, headcount, groups) = tokio::try_join!(
        rsvp::list(&state.db, id),
        rsvp::headcount(&state.db, id),
        group::list(&state.db, id),
    )?;
    let show = [
        rsvp::Response::Pending,
        rsvp::Response::Attending,
        rsvp::Response::Declined,
    ]
    .into_iter()
    .find(|response| query.show.as_deref() == Some(response_code(*response)));
    let tz = event.details.starts_at.timezone();
    let text = lang::text();
    Ok(render(IndexPage {
        links: EventLinks::new(id),
        event,
        headcount,
        subscribers: rsvps
            .iter()
            .filter(|rsvp| rsvp.email_status == EmailStatus::Confirmed)
            .count(),
        rows: rsvps
            .iter()
            .filter(|rsvp| show.is_none_or(|show| rsvp.response == show))
            .map(|rsvp| Row::new(rsvp, tz, &groups))
            .collect(),
        show: show.map(response_code),
        notice: match (query.reminders, query.created) {
            (Some(n), _) => Some(count(
                n,
                &text.reminders_queued_one,
                &text.reminders_queued_other,
            )),
            (None, Some(_)) => Some(text.event_created.clone()),
            (None, None) => None,
        },
        error: None,
    }))
}

#[derive(Template)]
#[template(path = "admin/qr.html")]
struct QrPage {
    qr_style: db::qr::Style,
    /// A code for the event page, in that style.
    qr_preview: String,
    saved: bool,
    error: Option<String>,
}

impl QrPage {
    fn new(config: &Config, qr_style: db::qr::Style) -> Self {
        Self {
            qr_preview: admin_qr::svg(&config.url(paths::HOME), &qr_style, None),
            qr_style,
            saved: false,
            error: None,
        }
    }
}

#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}

/// The QR code style, with an example.
async fn qr_style(State(state): State<AdminState>, Query(query): Query<SavedQuery>) -> Response {
    match db::qr::style(&state.db).await {
        Ok(style) => render(QrPage {
            saved: query.saved.is_some(),
            ..QrPage::new(&state.config, style)
        }),
        Err(err) => server_error(err),
    }
}

/// Saves the QR code style that every code on these pages uses.
async fn save_qr_style(
    State(state): State<AdminState>,
    Form(form): Form<admin_qr::StyleForm>,
) -> Response {
    let style = match form.validate() {
        Ok(style) => style,
        Err(error) => {
            let saved = match db::qr::style(&state.db).await {
                Ok(saved) => saved,
                Err(err) => return server_error(err),
            };
            let page = QrPage {
                error: Some(error),
                ..QrPage::new(&state.config, saved)
            };
            return render_with(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    if let Err(err) = db::qr::save(&state.db, &style).await {
        return server_error(err);
    }
    tracing::info!("qr style saved");
    Redirect::to(&paths::admin(&format!("{}?saved", paths::QR_STYLE))).into_response()
}

/// Queues a reminder for everyone with a confirmed address who hasn't declined. Each
/// guest gets at most one.
async fn queue_reminders(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let count = outbox::queue_reminders(&state.db, event.id).await?;
    tracing::info!(count, event = %id, "reminders queued");
    let page = format!("{}?reminders={count}", paths::path(paths::EVENT, id));
    Ok(Redirect::to(&paths::admin(&page)).into_response())
}

/// A name and a party size: an invitation's, or a group's for each sign-up.
#[derive(Deserialize)]
struct InviteForm {
    name: String,
    max_party_size: i32,
}

impl InviteForm {
    fn validate(&self) -> Result<String, String> {
        validate(&self.name, self.max_party_size)
    }
}

fn validate(name: &str, max_party_size: i32) -> Result<String, String> {
    let text = lang::text();
    if !(1..=MAX_PARTY_SIZE).contains(&max_party_size) {
        return Err(fill(&text.error_party_size, &[("max", &MAX_PARTY_SIZE)]));
    }
    rsvp::normalize_name(name).ok_or_else(|| text.error_name.clone())
}

/// Several names at once, one per line.
#[derive(Deserialize)]
struct BatchForm {
    names: String,
    /// For lines that don't give their own.
    max_party_size: i32,
}

impl BatchForm {
    /// Each line's name and party size. A line can end in a comma and its own party size;
    /// blank lines are skipped. One invalid line fails the whole batch.
    fn parse(&self) -> Result<Vec<(String, i32)>, String> {
        let mut entries = Vec::new();
        for (index, line) in self.names.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, max_party_size) = line
                .rsplit_once(',')
                .and_then(|(name, size)| Some((name, size.trim().parse().ok()?)))
                .unwrap_or((line, self.max_party_size));
            let name = validate(name, max_party_size).map_err(|error| {
                let line = index + 1;
                fill(&lang::text().error_line, &[("n", &line), ("error", &error)])
            })?;
            entries.push((name, max_party_size));
        }
        if entries.is_empty() {
            return Err(lang::text().error_no_names.clone());
        }
        Ok(entries)
    }
}

/// Creates an invitation per line, all or none, then shows their links.
async fn create_invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<BatchForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let entries = match form.parse() {
        Ok(entries) => entries,
        Err(error) => {
            return invitations_page(&state, event, None, form.names, Some(error)).await;
        }
    };
    let mut tx = state.db.begin().await?;
    let mut ids = Vec::new();
    for (name, max_party_size) in &entries {
        ids.push(
            rsvp::create_invite(&mut *tx, id, name, *max_party_size)
                .await?
                .id,
        );
    }
    tx.commit().await?;
    tracing::info!(count = ids.len(), event = %id, "invitations created");
    // The ids are UUIDv7, so the new ones are those from the first on.
    let page = format!(
        "{}?from={}",
        paths::path(paths::EVENT_INVITATIONS, id),
        ids[0]
    );
    Ok(Redirect::to(&paths::admin(&page)).into_response())
}

#[derive(Template)]
#[template(path = "admin/invitations.html")]
struct InvitationsPage {
    event: Event,
    links: EventLinks,
    /// Only the invitations just created.
    new: bool,
    invitations: Vec<Invitation>,
    /// The batch form's text, kept when a line needs fixing.
    names: String,
    max_party_size: i32,
    error: Option<String>,
}

struct Invitation {
    row: Row,
    url: String,
}

#[derive(Deserialize)]
struct InvitationsQuery {
    from: Option<Uuid>,
}

/// Every invitation's link, for sending them out, and the form for new ones.
async fn invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<InvitationsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    invitations_page(&state, event, query.from, String::new(), None).await
}

/// The event's invitations from `from` on, or all of them.
async fn invitations_page(
    state: &AdminState,
    event: Event,
    from: Option<Uuid>,
    names: String,
    error: Option<String>,
) -> Result<Response, PageError> {
    let rsvps = rsvp::list(&state.db, event.id).await?;
    let tz = event.details.starts_at.timezone();
    let invitations = rsvps
        .iter()
        .filter(|rsvp| rsvp.source == Source::Invite)
        .filter(|rsvp| from.is_none_or(|from| rsvp.id >= from))
        .map(|rsvp| Invitation {
            row: Row::new(rsvp, tz, &[]),
            url: guest_url(&state.config, rsvp),
        })
        .collect();
    let status = match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    };
    Ok(render_with(
        status,
        InvitationsPage {
            links: EventLinks::new(event.id),
            event,
            new: from.is_some(),
            invitations,
            names,
            max_party_size: MAX_PARTY_SIZE,
            error,
        },
    ))
}

#[derive(Template)]
#[template(path = "admin/guest.html")]
struct GuestPage {
    event: Event,
    rsvp: Rsvp,
    row: Row,
    links: AdminLinks,
    url: String,
    qr: String,
    max_party_size: i32,
    saved: bool,
    error: Option<String>,
}

impl GuestPage {
    fn new(
        config: &Config,
        event: Event,
        rsvp: Rsvp,
        groups: &[Group],
        style: &db::qr::Style,
    ) -> Self {
        let url = guest_url(config, &rsvp);
        Self {
            row: Row::new(&rsvp, event.details.starts_at.timezone(), groups),
            event,
            links: AdminLinks::guest(rsvp.id),
            qr: admin_qr::svg(&url, style, None),
            url,
            rsvp,
            max_party_size: MAX_PARTY_SIZE,
            saved: false,
            error: None,
        }
    }

    /// The guest's page again with an error, for an invalid form.
    async fn load(state: &AdminState, id: Uuid) -> Result<Self, PageError> {
        let rsvp = rsvp::find(&state.db, id).await?.ok_or(PageError::Missing)?;
        let (event, groups, style) = tokio::try_join!(
            load_event(state, rsvp.event_id),
            async { Ok::<_, PageError>(group::list(&state.db, rsvp.event_id).await?) },
            async { Ok::<_, PageError>(db::qr::style(&state.db).await?) },
        )?;
        Ok(Self::new(&state.config, event, rsvp, &groups, &style))
    }

    async fn render_error(
        state: &AdminState,
        id: Uuid,
        error: String,
    ) -> Result<Response, PageError> {
        let mut page = Self::load(state, id).await?;
        page.error = Some(error);
        Ok(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
    }
}

fn guest_url(config: &Config, rsvp: &Rsvp) -> String {
    let link = config
        .link_key
        .link(Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
    config.url(&paths::path(paths::RSVP, link))
}

/// A QR code as an SVG file to save, named after what it's for.
fn qr_file(url: &str, style: &db::qr::Style, name: &str) -> Response {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "code" } else { slug };
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}\n",
        admin_qr::svg(url, style, Some(1024))
    );
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!(r#"attachment; filename="qr-{slug}.svg""#),
            ),
        ],
        svg,
    )
        .into_response()
}

/// Why a request about one guest or group failed.
enum PageError {
    Missing,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for PageError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        match self {
            Self::Missing => {
                (StatusCode::NOT_FOUND, lang::text().not_found.as_str()).into_response()
            }
            Self::Database(err) => server_error(err),
        }
    }
}

/// The guest's QR code as a file, for printing.
async fn guest_qr(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let (rsvp, style) = tokio::try_join!(rsvp::find(&state.db, id), db::qr::style(&state.db))?;
    let rsvp = rsvp.ok_or(PageError::Missing)?;
    Ok(qr_file(
        &guest_url(&state.config, &rsvp),
        &style,
        &rsvp.name,
    ))
}

async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let mut page = GuestPage::load(&state, id).await?;
    page.saved = query.saved.is_some();
    Ok(render(page))
}

fn saved_redirect(id: Uuid) -> Response {
    let page = format!("{}?saved", paths::path(paths::GUEST, id));
    Redirect::to(&paths::admin(&page)).into_response()
}

async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<InviteForm>,
) -> Result<Response, PageError> {
    let name = match form.validate() {
        Ok(name) => name,
        Err(error) => return GuestPage::render_error(&state, id, error).await,
    };
    if !rsvp::update_invite(&state.db, id, &name, form.max_party_size).await? {
        return Err(PageError::Missing);
    }
    Ok(saved_redirect(id))
}

async fn revoke(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    rsvp::revoke_link(&state.db, id)
        .await?
        .ok_or(PageError::Missing)?;
    tracing::info!(rsvp = %id, "link revoked");
    Ok(saved_redirect(id))
}

#[derive(Deserialize)]
struct DeleteForm {
    confirm: Option<String>,
}

async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    if form.confirm.is_none() {
        let error = lang::text().error_confirm_delete.clone();
        return GuestPage::render_error(&state, id, error).await;
    }
    let rsvp = rsvp::find(&state.db, id).await?.ok_or(PageError::Missing)?;
    rsvp::delete(&state.db, id).await?;
    tracing::info!(rsvp = %id, "rsvp deleted by host");
    let event = paths::path(paths::EVENT, rsvp.event_id);
    Ok(Redirect::to(&paths::admin(&event)).into_response())
}

/// The event's guests as CSV, with each guest's link for a mail merge.
async fn export_csv(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let (rsvps, groups) = tokio::try_join!(rsvp::list(&state.db, id), group::list(&state.db, id))?;
    let tz = event.details.starts_at.timezone();
    let text = lang::text();
    // The byte order mark makes spreadsheet apps read the file as UTF-8 (æ, ø, å).
    let mut csv = String::from("\u{feff}");
    csv_line(
        &mut csv,
        &[
            &text.col_name,
            &text.col_source,
            &text.col_answer,
            &text.col_people,
            &text.col_max_people,
            &text.col_email,
            &text.col_guest_list,
            &text.col_answered,
            &text.col_link,
        ],
    );
    for rsvp in &rsvps {
        let row = Row::new(rsvp, tz, &groups);
        csv_line(
            &mut csv,
            &[
                &row.name,
                &row.source,
                row.answer,
                &rsvp.party_size.to_string(),
                &rsvp.max_party_size.to_string(),
                row.email,
                row.listed,
                &row.responded,
                &guest_url(&state.config, rsvp),
            ],
        );
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                r#"attachment; filename="rsvp.csv""#,
            ),
        ],
        csv,
    )
        .into_response())
}

/// Appends one CSV record. Every field is quoted, and fields that a spreadsheet would
/// run as a formula get a leading apostrophe.
fn csv_line(out: &mut String, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
            out.push('\'');
        }
        out.push_str(&field.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
}

/// `innby invite`: creates an invitation to the event, or to the only one there is, and
/// returns the guest's link.
pub async fn invite(
    config: &Config,
    db: &PgPool,
    event: Option<Uuid>,
    name: &str,
    max_party_size: i32,
) -> anyhow::Result<String> {
    let form = InviteForm {
        name: name.to_owned(),
        max_party_size,
    };
    let name = form.validate().map_err(anyhow::Error::msg)?;
    let event = match event {
        Some(id) => event::find(db, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("there's no event {id}"))?,
        None => {
            let mut events = event::list(db).await?;
            match events.len() {
                1 => events.remove(0),
                0 => anyhow::bail!("there are no events yet: create one in the admin UI"),
                _ => {
                    let list: Vec<String> = events
                        .iter()
                        .map(|event| format!("{} {}", event.id, event.details.title.nb))
                        .collect();
                    anyhow::bail!(
                        "which event? Pass --event with one of:\n{}",
                        list.join("\n")
                    )
                }
            }
        }
    };
    let invite = rsvp::create_invite(db, event.id, &name, max_party_size).await?;
    tracing::info!(rsvp = %invite.id, "invitation created");
    Ok(guest_url(config, &invite))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::tests::config,
        link::IdLink,
        web::testing::{body, escape, location},
    };

    struct Admin {
        db: PgPool,
        config: Config,
        admin: AdminConfig,
        /// The `token` cookie of a login.
        token: String,
    }

    impl Admin {
        fn new(db: PgPool) -> Self {
            Self::as_role(db, Role::Admin)
        }

        /// A host with this role, as single sign-on would log them in.
        fn as_role(db: PgPool, role: Role) -> Self {
            let admin = auth::tests::admin_config();
            Self {
                token: format!("token={}", auth::tests::token(&admin, db.clone(), role)),
                db,
                config: config(),
                admin,
            }
        }

        async fn send(&self, request: axum::http::request::Builder, body: &str) -> Response {
            // The tests read the British texts.
            let request = request
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ACCEPT_LANGUAGE, "en-GB")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body.to_owned()))
                .unwrap();
            router(self.config.clone(), &self.admin, self.db.clone())
                .oneshot(request)
                .await
                .unwrap()
        }

        fn logged_in(&self, request: axum::http::request::Builder) -> axum::http::request::Builder {
            request.header(header::COOKIE, &self.token)
        }

        async fn get(&self, uri: &str) -> Response {
            self.send(self.logged_in(Request::get(uri)), "").await
        }

        async fn post(&self, uri: &str, form: &str) -> Response {
            self.send(self.logged_in(Request::post(uri)), form).await
        }

        /// The path of the tests' event, under which its tabs are.
        async fn event(&self) -> String {
            let event = crate::db::event::tests::event(&self.db).await;
            paths::path(paths::EVENT, event.id)
        }
    }

    /// The `name=value` of each cookie the response sets, and the whole header.
    fn set_cookies(response: &Response) -> Vec<(String, String)> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| {
                let value = value.to_str().unwrap();
                (
                    value.split(';').next().unwrap().to_owned(),
                    value.to_owned(),
                )
            })
            .collect()
    }

    #[sqlx::test]
    async fn each_role_reaches_only_its_pages(db: PgPool) {
        let event = crate::db::event::tests::event(&db).await;
        let invite = rsvp::create_invite(&db, event.id, "Kari", 2).await.unwrap();
        let group = group::create(&db, event.id, "Climbing club", 2)
            .await
            .unwrap();
        let on = |pattern: &str, id: Uuid| paths::path(pattern, id);
        let (e, i, g) = (event.id, invite.id, group.id);
        // Every page behind the login, and the role it needs. Forms are sent empty, so
        // the ones a role may use turn the request down for lack of a confirmation or
        // a name, rather than change anything worth checking here.
        let pages = [
            (Method::GET, paths::HOME.to_owned(), Role::Viewer),
            (Method::GET, on(paths::EVENT, e), Role::Viewer),
            (Method::GET, on(paths::EVENT_INVITATIONS, e), Role::Editor),
            (Method::POST, on(paths::EVENT_INVITATIONS, e), Role::Editor),
            (Method::GET, on(paths::EVENT_GROUPS, e), Role::Editor),
            (Method::POST, on(paths::EVENT_GROUPS, e), Role::Editor),
            (Method::GET, on(paths::EVENT_DETAILS, e), Role::Editor),
            (Method::POST, on(paths::EVENT_DETAILS, e), Role::Editor),
            (Method::GET, on(paths::EVENT_CSV, e), Role::Editor),
            (Method::POST, on(paths::EVENT_REMINDERS, e), Role::Editor),
            (Method::GET, on(paths::GUEST, i), Role::Editor),
            (Method::POST, on(paths::GUEST, i), Role::Editor),
            (Method::GET, on(paths::GUEST_QR, i), Role::Editor),
            (Method::POST, on(paths::GUEST_DELETE, i), Role::Editor),
            (Method::GET, on(paths::GROUP, g), Role::Editor),
            (Method::POST, on(paths::GROUP, g), Role::Editor),
            (Method::GET, on(paths::GROUP_QR, g), Role::Editor),
            (Method::POST, on(paths::GROUP_DELETE, g), Role::Editor),
            (Method::POST, paths::EVENTS.to_owned(), Role::Admin),
            (Method::POST, on(paths::EVENT_DELETE, e), Role::Admin),
            (Method::GET, paths::QR_STYLE.to_owned(), Role::Admin),
            (Method::POST, paths::QR_STYLE.to_owned(), Role::Admin),
        ];
        for role in Role::ALL {
            let host = Admin::as_role(db.clone(), role);
            for (method, path, needs) in &pages {
                let request = Request::builder().method(method.clone()).uri(path);
                let response = host.send(host.logged_in(request), "").await;
                let forbidden = response.status() == StatusCode::FORBIDDEN;
                assert_eq!(forbidden, role < *needs, "{role:?} {method} {path}");
            }
        }
        // The revoking forms change something even when empty, so they're tried last.
        for (pattern, id) in [(paths::GUEST_REVOKE, i), (paths::GROUP_REVOKE, g)] {
            let path = on(pattern, id);
            let viewer = Admin::as_role(db.clone(), Role::Viewer);
            assert_eq!(viewer.post(&path, "").await.status(), StatusCode::FORBIDDEN);
            let editor = Admin::as_role(db.clone(), Role::Editor);
            assert_ne!(editor.post(&path, "").await.status(), StatusCode::FORBIDDEN);
        }

        // Pages show only what the role may use.
        let viewer = Admin::as_role(db.clone(), Role::Viewer);
        let html = body(viewer.get(&on(paths::EVENT, e)).await).await;
        assert!(html.contains("Kari") && !html.contains(&on(paths::GUEST, i)));
        for hidden in ["guests.csv", "/reminders\"", "/invitations\"", "/details\""] {
            assert!(!html.contains(hidden), "{hidden}");
        }
        let html = body(viewer.get(paths::HOME).await).await;
        assert!(!html.contains(paths::QR_STYLE) && !html.contains(r#"action="/events""#));

        let editor = Admin::as_role(db.clone(), Role::Editor);
        let html = body(editor.get(&on(paths::EVENT, e)).await).await;
        assert!(html.contains(&on(paths::GUEST, i)) && html.contains("guests.csv"));
        let html = body(editor.get(&on(paths::EVENT_DETAILS, e)).await).await;
        assert!(!html.contains("/delete"));
        let html = body(editor.get(paths::HOME).await).await;
        assert!(!html.contains(paths::QR_STYLE) && !html.contains(r#"action="/events""#));

        let admin = Admin::new(db);
        let html = body(admin.get(&on(paths::EVENT_DETAILS, e)).await).await;
        assert!(html.contains("/delete"));
        let html = body(admin.get(paths::HOME).await).await;
        assert!(html.contains(paths::QR_STYLE) && html.contains(r#"action="/events""#));
    }

    #[sqlx::test]
    async fn serves_under_a_path_with_the_probes_at_the_root(db: PgPool) {
        let mut admin = Admin::new(db);
        admin.admin.path = "/admin".into();
        let status = |response: Response| response.status();
        for probe in [paths::HEALTH, paths::READY] {
            assert_eq!(
                status(admin.send(Request::get(probe), "").await),
                StatusCode::OK
            );
        }
        let response = admin.send(Request::get("/admin"), "").await;
        assert_eq!(location(&response), "/admin/login");
        let response = admin.send(Request::get("/admin/"), "").await;
        assert_eq!(location(&response), "/admin");
        let html = body(admin.send(Request::get("/admin/login"), "").await).await;
        assert!(html.contains(r#"href="/admin/static/style.css"#), "{html}");
        assert!(html.contains(r#"action="/admin/login""#));
        let css = admin.send(
            Request::get(paths::STYLESHEET).uri("/admin/static/style.css"),
            "",
        );
        assert_eq!(status(css.await), StatusCode::OK);
        // Nothing of the admin's outside its path.
        assert_eq!(status(admin.get("/events").await), StatusCode::NOT_FOUND);

        let response = admin.get("/admin").await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body(response).await;
        assert!(html.contains(r#"href="/admin/qr-style""#), "{html}");
    }

    #[sqlx::test]
    async fn requires_login_except_for_probes(db: PgPool) {
        let admin = Admin::new(db);
        for cookie in ["", "token=garbage", "refresh_token=garbage"] {
            let response = admin
                .send(Request::get("/").header(header::COOKIE, cookie), "")
                .await;
            assert_eq!(location(&response), "/login", "{cookie}");
        }
        // A form posted without a login doesn't run either.
        let response = admin.send(Request::post("/events"), "title=Fest").await;
        assert_eq!(location(&response), "/login");
        assert!(
            rsvp::list(
                &admin.db,
                crate::db::event::tests::event(&admin.db).await.id
            )
            .await
            .unwrap()
            .is_empty()
        );

        let response = admin.send(Request::get("/healthz"), "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = admin.send(Request::get("/login"), "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body(response).await.contains(r#"type="password""#));

        let response = admin.get("/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[sqlx::test]
    async fn logs_in_refreshes_and_logs_out(db: PgPool) {
        let admin = Admin::new(db);
        let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
        let send = |method: &str, uri: &str, cookie: &str, form: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ACCEPT_LANGUAGE, "en-GB")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(form.to_owned()))
                .unwrap();
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap() }
        };

        let response = send("POST", "/login", "", "user=admin&password=wrong").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&response).is_empty());
        let html = body(response).await;
        assert!(html.contains("Wrong user name or password."), "{html}");
        assert!(html.contains(r#"value="admin""#), "{html}");

        let response = send("POST", "/login", "", "user=admin&password=secret").await;
        assert_eq!(location(&response), "/");
        let cookies = set_cookies(&response);
        let [(token, token_header), (refresh, refresh_header)] = &cookies[..] else {
            panic!("{cookies:?}");
        };
        assert!(token.starts_with("token=ey"), "{token}");
        assert!(token_header.ends_with("; HttpOnly; SameSite=Lax; Secure"));
        assert!(refresh.starts_with("refresh_token="), "{refresh}");
        assert!(refresh_header.contains("; Max-Age=2592000;"));
        let both = format!("{token}; {refresh}");

        let response = send("GET", "/", &both, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(set_cookies(&response).is_empty());
        // Without a JWT, as once it expires, the refresh token gets a new one.
        let response = send("GET", "/", refresh, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let renewed = set_cookies(&response);
        assert!(renewed[0].0.starts_with("token=ey"), "{renewed:?}");
        assert_ne!(&renewed[0].0, token);

        let response = send("POST", "/logout", &both, "").await;
        assert_eq!(location(&response), "/login?logged_out");
        let cleared = set_cookies(&response);
        assert_eq!(
            cleared
                .iter()
                .map(|(cookie, _)| cookie.as_str())
                .collect::<Vec<_>>(),
            ["token=", "refresh_token="]
        );
        assert!(cleared[0].1.contains("; Max-Age=0;"));
        let html = body(send("GET", "/login?logged_out", "", "").await).await;
        assert!(html.contains(&escape("You're logged out.")), "{html}");

        // Copies of the cookies stop working too.
        for cookie in [both.as_str(), token, refresh] {
            let response = send("GET", "/", cookie, "").await;
            assert_eq!(location(&response), "/login", "{cookie}");
        }
        // Another login still works.
        let response = send("POST", "/login", "", "user=admin&password=secret").await;
        assert_eq!(location(&response), "/");
    }

    #[sqlx::test]
    async fn speaks_the_browsers_language_or_the_one_picked(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let request = admin
            .logged_in(Request::get(format!("{e}/invitations")))
            .header(header::ACCEPT_LANGUAGE, "nn-NO")
            .body(Body::empty())
            .unwrap();
        let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
        let html = body(app.oneshot(request).await.unwrap()).await;
        assert!(html.contains(r#"<html lang="nn""#), "{html}");
        assert!(html.contains("Nye invitasjonar"), "{html}");
        // The browser's language needs no carrying.
        assert!(
            html.contains(&format!(r#"action="{e}/invitations""#)),
            "{html}"
        );

        // A picked one is carried, through forms and redirects too.
        let html = body(
            admin
                .get(&format!("{e}/invitations?lang=zh-hant&theme=dark"))
                .await,
        )
        .await;
        assert!(html.contains("新邀請"), "{html}");
        assert!(
            html.contains(&format!(
                r#"action="{e}/invitations?lang=zh-hant&#38;theme=dark""#
            )),
            "{html}"
        );
        let response = admin
            .post(
                &format!("{e}/groups?lang=zh-hant"),
                "names=%E9%84%B0%E5%B1%85&max_party_size=1",
            )
            .await;
        assert_eq!(
            location(&response),
            format!("{e}/groups?created=1&lang=zh-hant")
        );
        let html = body(admin.get(location(&response)).await).await;
        assert!(html.contains("已建立 1 個群組。"), "{html}");
        // A form with a mistake says so in the picked language.
        let response = admin
            .post(
                &format!("{e}/invitations?lang=nb"),
                "names=Kari%2C+0&max_party_size=1",
            )
            .await;
        let html = body(response).await;
        assert!(
            html.contains("Linje 1: Antall personer må være mellom 1 og 20."),
            "{html}"
        );
    }

    #[sqlx::test]
    async fn styles_and_downloads_qr_codes(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let html = body(admin.get("/qr-style").await).await;
        assert!(html.contains(r#"value="rounded" checked"#), "{html}");

        // A colour phones can't read on white is refused.
        let response = admin
            .post("/qr-style", "shape=dots&color=%23ffeb3b&icon=none")
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("Pick a darker colour"));
        let response = admin
            .post("/qr-style", "shape=dots&color=%23000000&icon=emoji&emoji=")
            .await;
        assert!(body(response).await.contains("Enter one emoji"));

        let response = admin
            .post(
                "/qr-style",
                "shape=dots&color=%23000000&icon=emoji&emoji=%F0%9F%8E%89",
            )
            .await;
        assert_eq!(location(&response), "/qr-style?saved");
        let html = body(admin.get("/qr-style?saved").await).await;
        assert!(html.contains("QR code style saved."), "{html}");
        assert!(html.contains(r#"value="dots" checked"#), "{html}");
        assert!(html.contains("🎉</text>"), "{html}");

        admin
            .post(
                &format!("{e}/invitations"),
                "names=Kari+Nordmann&max_party_size=1",
            )
            .await;
        let id = rsvp::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap()[0]
            .id;
        let html = body(admin.get(&format!("/guests/{id}")).await).await;
        assert!(html.contains(&format!(r#"href="/guests/{id}/qr.svg" download"#)));
        let response = admin.get(&format!("/guests/{id}/qr.svg")).await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            r#"attachment; filename="qr-kari-nordmann.svg""#
        );
        let svg = body(response).await;
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains(r##"fill="#000000""##));
        assert!(svg.contains(r#"width="1024""#));
    }

    #[sqlx::test]
    async fn links_carry_the_colour_scheme(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let html = body(admin.get(&format!("{e}?theme=dark")).await).await;
        assert!(html.contains(r#"data-theme="dark""#), "{html}");
        assert!(
            html.contains(&format!(r#"href="{e}/invitations?theme=dark""#)),
            "{html}"
        );
        let html = body(admin.get(&format!("{e}/invitations?theme=dark")).await).await;
        assert!(
            html.contains(&format!(r#"action="{e}/invitations?theme=dark""#)),
            "{html}"
        );
        assert!(html.contains(r#"action="/logout?theme=dark""#), "{html}");

        let response = admin
            .post(
                &format!("{e}/invitations?theme=dark"),
                "names=Eva&max_party_size=1",
            )
            .await;
        assert!(
            location(&response).ends_with("&theme=dark"),
            "{}",
            location(&response)
        );
        let response = admin
            .post(
                &format!("{e}/groups?theme=light"),
                "names=Climbers&max_party_size=1",
            )
            .await;
        assert_eq!(
            location(&response),
            format!("{e}/groups?created=1&theme=light")
        );

        // The login page has its style before anyone logs in.
        let response = admin.send(Request::get("/static/style.css"), "").await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// The `value` of every read-only link field on the page.
    fn links(html: &str) -> Vec<&str> {
        html.split(r#"readonly value=""#)
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect()
    }

    #[sqlx::test]
    async fn creates_invitations_in_a_batch(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let response = admin
            .post(
                &format!("{e}/invitations"),
                "names=Kari+%26+Ola%2C+2%0D%0A%0D%0APer+Hansen%0D%0ANordmann%2C+Anne&max_party_size=1",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let page = location(&response).to_owned();
        assert!(
            page.starts_with(&format!("{e}/invitations?from=")),
            "{page}"
        );

        let html = body(admin.get(&page).await).await;
        assert!(html.contains("3 invitations created."), "{html}");
        let invites = rsvp::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap();
        let sizes: Vec<_> = invites
            .iter()
            .map(|invite| (invite.name.as_str(), invite.max_party_size))
            .collect();
        assert_eq!(
            sizes,
            [("Kari & Ola", 2), ("Per Hansen", 1), ("Nordmann, Anne", 1)]
        );
        let urls = links(&html);
        assert_eq!(urls.len(), 3);
        let link: IdLink = urls[0]
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(link.id, invites[0].id);
        assert!(
            admin
                .config
                .link_key
                .verify(Purpose::Rsvp, &link, invites[0].link_nonce)
        );

        // Later batches list only their own invitations; the full list has them all.
        let response = admin
            .post(&format!("{e}/invitations"), "names=Eva&max_party_size=1")
            .await;
        let html = body(admin.get(location(&response)).await).await;
        assert_eq!(links(&html).len(), 1);
        let html = body(admin.get(&format!("{e}/invitations")).await).await;
        assert_eq!(links(&html).len(), 4);
    }

    #[sqlx::test]
    async fn rejects_a_batch_with_an_invalid_line(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        for (form, error) in [
            (
                "names=Kari%0D%0AOla%2C+21&max_party_size=2",
                "Line 2: The party size must be between 1 and 20.",
            ),
            (
                "names=Kari%0D%0A+-+%0D%0A&max_party_size=2",
                "Line 2: Enter a name of up to 100 characters.",
            ),
            ("names=+%0D%0A&max_party_size=2", "Enter at least one name."),
            (
                "names=Kari&max_party_size=0",
                "Line 1: The party size must be between 1 and 20.",
            ),
        ] {
            let response = admin.post(&format!("{e}/invitations"), form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
            let html = body(response).await;
            assert!(html.contains(error), "{form}: {html}");
        }
        // The text stays in the form to fix.
        let html = body(
            admin
                .post(
                    &format!("{e}/invitations"),
                    "names=Kari%0D%0AOla%2C+21&max_party_size=2",
                )
                .await,
        )
        .await;
        assert!(html.contains(">Kari\r\nOla, 21</textarea>"), "{html}");
        assert!(
            rsvp::list(
                &admin.db,
                crate::db::event::tests::event(&admin.db).await.id
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[sqlx::test]
    async fn edits_revokes_and_deletes(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let invite = rsvp::create_invite(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
            "Kari",
            1,
        )
        .await
        .unwrap();
        let path = format!("/guests/{}", invite.id);

        let response = admin
            .post(&path, "name=Kari+Nordmann&max_party_size=3")
            .await;
        assert_eq!(location(&response), format!("{path}?saved"));
        let edited = rsvp::find(&admin.db, invite.id).await.unwrap().unwrap();
        assert_eq!(
            (edited.name.as_str(), edited.max_party_size),
            ("Kari Nordmann", 3)
        );

        admin.post(&format!("{path}/revoke"), "").await;
        let revoked = rsvp::find(&admin.db, invite.id).await.unwrap().unwrap();
        assert_eq!(revoked.link_nonce, invite.link_nonce + 1);

        let response = admin.post(&format!("{path}/delete"), "").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = admin.post(&format!("{path}/delete"), "confirm=yes").await;
        assert_eq!(location(&response), e);
        assert!(rsvp::find(&admin.db, invite.id).await.unwrap().is_none());
        assert_eq!(admin.get(&path).await.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn the_headcount_filters_the_guests(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let kari = rsvp::create_invite(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
            "Kari",
            3,
        )
        .await
        .unwrap();
        rsvp::create_invite(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
            "Ola",
            1,
        )
        .await
        .unwrap();
        rsvp::respond(
            &admin.db,
            kari.id,
            rsvp::Response::Attending,
            3,
            None,
            crate::i18n::Lang::EnGb,
            web::PRIVACY_VERSION,
        )
        .await
        .unwrap();

        let html = body(admin.get(&e).await).await;
        assert!(html.contains("<strong>3</strong> people coming"), "{html}");
        assert!(html.contains("<strong>1</strong> no answer"), "{html}");
        assert!(
            html.contains(&format!(r#"<a href="{e}" aria-current="page">Guests</a>"#)),
            "{html}"
        );
        assert!(html.contains("Kari") && html.contains("Ola"));
        assert!(!html.contains("Show everyone"));

        let html = body(admin.get(&format!("{e}?show=attending")).await).await;
        assert!(
            html.contains(&format!(r#"href="{e}?show=attending" aria-current="true""#)),
            "{html}"
        );
        assert!(
            html.contains(r#"<span class="status attending">coming</span>"#),
            "{html}"
        );
        assert!(html.contains("Kari") && !html.contains(">Ola<"), "{html}");
        assert!(html.contains("Show everyone"));
        // An unknown answer shows everyone.
        let html = body(admin.get(&format!("{e}?show=maybe")).await).await;
        assert!(html.contains(">Ola<") && !html.contains("Show everyone"));
    }

    #[sqlx::test]
    async fn exports_csv_with_links(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        rsvp::create_invite(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
            "=cmd|' /C calc'!A0",
            1,
        )
        .await
        .unwrap();
        rsvp::create_invite(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
            "Kari \"K\" Ø",
            2,
        )
        .await
        .unwrap();

        let response = admin.get(&format!("{e}/guests.csv")).await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        let csv = body(response).await;
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with(r#""Name","Source","Answer""#));
        assert!(lines[1].starts_with(r#""'=cmd|' /C calc'!A0","invitation","no answer","1","1""#));
        assert!(lines[2].starts_with(r#""Kari ""K"" Ø","#));
        assert!(lines[2].contains(r#""https://fest.example.com/r/"#));
    }

    #[sqlx::test]
    async fn creates_and_manages_groups(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let response = admin
            .post(
                &format!("{e}/groups"),
                "names=Climbing+club%0D%0ANeighbours%2C+4&max_party_size=2",
            )
            .await;
        assert_eq!(location(&response), format!("{e}/groups?created=2"));
        let html = body(admin.get(&format!("{e}/groups?created=2")).await).await;
        assert!(html.contains("2 groups created."));
        let [climbing, neighbours] = group::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap()
        .try_into()
        .unwrap();
        assert_eq!(
            (neighbours.name.as_str(), neighbours.max_party_size),
            ("Neighbours", 4)
        );
        let url = format!(
            "https://fest.example.com/join/{}",
            admin.config.link_key.link(Purpose::Group, climbing.id, 0)
        );
        assert!(links(&html).contains(&url.as_str()));

        let page = format!("/groups/{}", climbing.id);
        let html = body(admin.get(&page).await).await;
        assert!(html.contains("<h1>Climbing club</h1>"));
        assert!(html.contains(&format!(r#"value="{url}""#)));
        assert!(html.contains("<svg"));

        // Someone signs up; the group shows them and can't be deleted any more.
        let sign_up = rsvp::SignUp {
            group: &climbing,
            name: "Per Hansen",
            party_size: 1,
            name_display: rsvp::NameDisplay::Partial,
            lang: crate::i18n::Lang::Nb,
        };
        rsvp::create_open(&admin.db, &sign_up, web::PRIVACY_VERSION)
            .await
            .unwrap();
        let html = body(admin.get(&page).await).await;
        assert!(html.contains("Per Hansen"));
        assert!(html.contains("first name and initial"));
        let html = body(admin.get(&e).await).await;
        assert!(html.contains("sign-up: Climbing club"));
        let response = admin.post(&format!("{page}/delete"), "confirm=yes").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("can&#39;t be deleted"));

        admin
            .post(&page, "name=Climbing+friends&max_party_size=3")
            .await;
        admin.post(&format!("{page}/revoke"), "").await;
        let edited = group::find(&admin.db, climbing.id).await.unwrap().unwrap();
        assert_eq!(
            (
                edited.name.as_str(),
                edited.max_party_size,
                edited.link_nonce
            ),
            ("Climbing friends", 3, 1)
        );

        let response = admin
            .post(&format!("/groups/{}/delete", neighbours.id), "confirm=yes")
            .await;
        assert_eq!(location(&response), format!("{e}/groups"));
        assert!(
            group::find(&admin.db, neighbours.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test]
    async fn invite_command_prints_a_working_link(db: PgPool) {
        let config = config();
        // With no event yet there's nothing to invite to; with one, it's that one.
        assert!(invite(&config, &db, None, "Per", 2).await.is_err());
        let event = crate::db::event::tests::event(&db).await;
        let url = invite(&config, &db, None, "  Per  ", 2).await.unwrap();
        let link: IdLink = url
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        let invite = rsvp::find(&db, link.id).await.unwrap().unwrap();
        assert_eq!(invite.name, "Per");
        assert!(
            config
                .link_key
                .verify(Purpose::Rsvp, &link, invite.link_nonce)
        );

        assert_eq!(invite.event_id, event.id);
        assert!(super::invite(&config, &db, None, "Per", 0).await.is_err());
        // With two, it has to be told which.
        crate::db::event::create(&db, &crate::db::event::tests::details())
            .await
            .unwrap();
        assert!(super::invite(&config, &db, None, "Ola", 1).await.is_err());
        assert!(
            super::invite(&config, &db, Some(event.id), "Ola", 1)
                .await
                .is_ok()
        );
    }
}
