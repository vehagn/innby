//! `innby admin`: the host's UI for invitations, group links and the headcount, in the
//! site's languages. Served on its own listener behind the internal gateway and basic
//! auth.

pub mod auth;
mod groups;
pub mod lang;
mod qr;

use std::sync::Arc;

use askama::Template;
use axum::{
    Form, Router,
    extract::{FromRef, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
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
        group::{self, Group},
        outbox,
        rsvp::{self, EmailStatus, Headcount, MAX_PARTY_SIZE, NameDisplay, Rsvp, Source},
    },
    i18n::{count, fill, filled},
    link::Purpose,
    paths::{self, AdminLinks},
    web::{self, render, render_with, server_error},
};
use qr as admin_qr;

#[derive(Clone)]
struct AdminState {
    config: Arc<Config>,
    db: PgPool,
    login: Arc<auth::Login>,
}

impl FromRef<AdminState> for Arc<auth::Login> {
    fn from_ref(state: &AdminState) -> Self {
        Arc::clone(&state.login)
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

fn router(config: Config, admin: &AdminConfig, db: PgPool) -> Router {
    // The admin hostname is served the same way as the public one.
    let https = config.is_https();
    let login = Arc::new(auth::Login::new(admin));
    let state = AdminState {
        config: Arc::new(config),
        db,
        login: Arc::clone(&login),
    };
    let pages = Router::new()
        .route(paths::HOME, get(index))
        .route(
            paths::INVITATIONS,
            get(invitations).post(create_invitations),
        )
        .route(paths::GROUPS, post(groups::create))
        .route(paths::GROUP, get(groups::show).post(groups::edit))
        .route(paths::GROUP_REVOKE, post(groups::revoke))
        .route(paths::GROUP_DELETE, post(groups::delete))
        .route(paths::GUEST, get(show).post(edit))
        .route(paths::GUEST_REVOKE, post(revoke))
        .route(paths::GUEST_DELETE, post(delete))
        .route(paths::GUEST_QR, get(guest_qr))
        .route(paths::GROUP_QR, get(groups::qr))
        .route(paths::QR_STYLE, post(save_qr_style))
        .route(paths::GUESTS_CSV, get(export_csv))
        .route(paths::REMINDERS, post(queue_reminders))
        .route(paths::LOGOUT, post(log_out))
        .layer(middleware::from_fn_with_state(login, auth::require_login));
    let router = web::no_store(pages)
        // Probes don't log in, and neither do the pages around logging in.
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(web::ready))
        .route(paths::STYLESHEET, get(web::stylesheet))
        .route(
            paths::LOGGED_OUT,
            get(|| async { render(auth::LoginPage::logged_out()) }),
        )
        .layer(middleware::from_fn(crate::theme::scope))
        .layer(middleware::from_fn(lang::scope))
        .with_state(state);
    web::harden(router, https)
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

fn local(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

#[derive(Template)]
#[template(path = "admin/index.html")]
struct IndexPage<'a> {
    title: &'a str,
    headcount: Headcount,
    /// Guests with a confirmed address.
    subscribers: usize,
    rows: Vec<Row>,
    groups: Vec<groups::GroupRow>,
    max_party_size: i32,
    /// False once the answer deadline has passed, which closes the group links too.
    accepts_answers: bool,
    qr_style: db::qr::Style,
    /// A code for the event page, in that style.
    qr_preview: String,
    feedback: Feedback,
    error: Option<String>,
}

/// What the index page shows on top of the data: a notice after a change, and the text
/// of the batch forms, kept when a line needs fixing.
#[derive(Default)]
struct Feedback {
    notice: Option<String>,
    invitations: String,
    groups: String,
}

async fn index_page(state: &AdminState, error: Option<String>, feedback: Feedback) -> Response {
    let loaded = tokio::try_join!(
        rsvp::list(&state.db),
        rsvp::headcount(&state.db),
        group::list(&state.db),
        db::qr::style(&state.db),
    );
    let (rsvps, headcount, groups, qr_style) = match loaded {
        Ok(loaded) => loaded,
        Err(err) => return server_error(err),
    };
    let tz = state.config.event.starts_at.timezone();
    let status = match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    };
    let page = IndexPage {
        title: state.config.event.title.get(lang::current()),
        headcount,
        subscribers: rsvps
            .iter()
            .filter(|rsvp| rsvp.email_status == EmailStatus::Confirmed)
            .count(),
        rows: rsvps
            .iter()
            .map(|rsvp| Row::new(rsvp, tz, &groups))
            .collect(),
        groups: groups
            .iter()
            .map(|group| groups::GroupRow::new(&state.config, group, &rsvps))
            .collect(),
        max_party_size: MAX_PARTY_SIZE,
        accepts_answers: state.config.event.accepts_answers(Utc::now()),
        qr_preview: admin_qr::svg(&state.config.url(paths::HOME), &qr_style, None),
        qr_style,
        feedback,
        error,
    };
    render_with(status, page)
}

#[derive(Deserialize)]
struct IndexQuery {
    reminders: Option<usize>,
    groups: Option<usize>,
    /// The QR code style was saved.
    qr: Option<String>,
}

async fn index(State(state): State<AdminState>, Query(query): Query<IndexQuery>) -> Response {
    let text = lang::text();
    let notice = match (query.reminders, query.groups) {
        (Some(n), _) => Some(count(
            n,
            &text.reminders_queued_one,
            &text.reminders_queued_other,
        )),
        (_, Some(n)) => Some(count(
            n,
            &text.groups_created_one,
            &text.groups_created_other,
        )),
        (None, None) => query.qr.map(|_| text.qr_saved.clone()),
    };
    let feedback = Feedback {
        notice,
        ..Feedback::default()
    };
    index_page(&state, None, feedback).await
}

/// Saves the QR code style that every code on these pages uses.
async fn save_qr_style(
    State(state): State<AdminState>,
    Form(form): Form<admin_qr::StyleForm>,
) -> Response {
    let style = match form.validate() {
        Ok(style) => style,
        Err(error) => return index_page(&state, Some(error), Feedback::default()).await,
    };
    if let Err(err) = db::qr::save(&state.db, &style).await {
        return server_error(err);
    }
    tracing::info!("qr style saved");
    Redirect::to(&paths::admin(&format!("{}?qr#qr", paths::HOME))).into_response()
}

/// Logs the browser out: it asks for the password on the next page.
async fn log_out(State(login): State<Arc<auth::Login>>, headers: HeaderMap) -> Response {
    if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        login.log_out(authorization);
    }
    tracing::info!("host logged out");
    Redirect::to(&paths::admin(paths::LOGGED_OUT)).into_response()
}

/// Queues a reminder for everyone with a confirmed address who hasn't declined. Each
/// guest gets at most one.
async fn queue_reminders(State(state): State<AdminState>) -> Response {
    match outbox::queue_reminders(&state.db).await {
        Ok(count) => {
            tracing::info!(count, "reminders queued");
            Redirect::to(&paths::admin(&format!("{}?reminders={count}", paths::HOME)))
                .into_response()
        }
        Err(err) => server_error(err),
    }
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
    Form(form): Form<BatchForm>,
) -> Response {
    let entries = match form.parse() {
        Ok(entries) => entries,
        Err(error) => {
            let feedback = Feedback {
                invitations: form.names,
                ..Feedback::default()
            };
            return index_page(&state, Some(error), feedback).await;
        }
    };
    let created = async {
        let mut tx = state.db.begin().await?;
        let mut ids = Vec::new();
        for (name, max_party_size) in &entries {
            ids.push(
                rsvp::create_invite(&mut *tx, name, *max_party_size)
                    .await?
                    .id,
            );
        }
        tx.commit().await.map(|()| ids)
    }
    .await;
    match created {
        Ok(ids) => {
            tracing::info!(count = ids.len(), "invitations created");
            // The ids are UUIDv7, so the new ones are those from the first on.
            let page = format!("{}?from={}", paths::INVITATIONS, ids[0]);
            Redirect::to(&paths::admin(&page)).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "admin/invitations.html")]
struct InvitationsPage {
    /// Only the invitations just created.
    new: bool,
    invitations: Vec<Invitation>,
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

/// Every invitation's link, for sending them out.
async fn invitations(
    State(state): State<AdminState>,
    Query(query): Query<InvitationsQuery>,
) -> Response {
    let rsvps = match rsvp::list(&state.db).await {
        Ok(rsvps) => rsvps,
        Err(err) => return server_error(err),
    };
    let tz = state.config.event.starts_at.timezone();
    let invitations = rsvps
        .iter()
        .filter(|rsvp| rsvp.source == Source::Invite)
        .filter(|rsvp| query.from.is_none_or(|from| rsvp.id >= from))
        .map(|rsvp| Invitation {
            row: Row::new(rsvp, tz, &[]),
            url: guest_url(&state.config, rsvp),
        })
        .collect();
    render(InvitationsPage {
        new: query.from.is_some(),
        invitations,
        error: None,
    })
}

#[derive(Template)]
#[template(path = "admin/guest.html")]
struct GuestPage {
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
    fn new(config: &Config, rsvp: Rsvp, groups: &[Group], style: &db::qr::Style) -> Self {
        let url = guest_url(config, &rsvp);
        Self {
            row: Row::new(&rsvp, config.event.starts_at.timezone(), groups),
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
        let (rsvp, groups, style) = tokio::try_join!(
            rsvp::find(&state.db, id),
            group::list(&state.db),
            db::qr::style(&state.db),
        )?;
        Ok(Self::new(
            &state.config,
            rsvp.ok_or(PageError::Missing)?,
            &groups,
            &style,
        ))
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

#[derive(Deserialize)]
struct ShowQuery {
    saved: Option<String>,
}

async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ShowQuery>,
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
    rsvp::delete(&state.db, id).await?;
    tracing::info!(rsvp = %id, "rsvp deleted by host");
    Ok(Redirect::to(&paths::admin(paths::HOME)).into_response())
}

/// The guest list as CSV, with each guest's link for a mail merge.
async fn export_csv(State(state): State<AdminState>) -> Response {
    let (rsvps, groups) = match tokio::try_join!(rsvp::list(&state.db), group::list(&state.db)) {
        Ok(loaded) => loaded,
        Err(err) => return server_error(err),
    };
    let tz = state.config.event.starts_at.timezone();
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
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                r#"attachment; filename="rsvp.csv""#,
            ),
        ],
        csv,
    )
        .into_response()
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

/// `innby invite`: creates an invitation and returns the guest's link.
pub async fn invite(
    config: &Config,
    db: &PgPool,
    name: &str,
    max_party_size: i32,
) -> anyhow::Result<String> {
    let form = InviteForm {
        name: name.to_owned(),
        max_party_size,
    };
    let name = form.validate().map_err(anyhow::Error::msg)?;
    let invite = rsvp::create_invite(db, &name, max_party_size).await?;
    tracing::info!(rsvp = %invite.id, "invitation created");
    Ok(guest_url(config, &invite))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use tower::ServiceExt;

    use super::*;
    use crate::{
        config::tests::config,
        link::IdLink,
        web::testing::{body, location},
    };

    struct Admin {
        db: PgPool,
        config: Config,
        admin: AdminConfig,
    }

    impl Admin {
        fn new(db: PgPool) -> Self {
            Self {
                db,
                config: config(),
                admin: AdminConfig {
                    listen: "127.0.0.1:0".parse().unwrap(),
                    user: "admin".into(),
                    password_hash: auth::tests::cheap_hash("secret"),
                },
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

        fn login(
            request: axum::http::request::Builder,
            user_password: &str,
        ) -> axum::http::request::Builder {
            request.header(
                header::AUTHORIZATION,
                format!("Basic {}", STANDARD.encode(user_password)),
            )
        }

        async fn get(&self, uri: &str) -> Response {
            self.send(Self::login(Request::get(uri), "admin:secret"), "")
                .await
        }

        async fn post(&self, uri: &str, form: &str) -> Response {
            self.send(Self::login(Request::post(uri), "admin:secret"), form)
                .await
        }
    }

    #[sqlx::test]
    async fn requires_login_except_for_probes(db: PgPool) {
        let admin = Admin::new(db);
        let response = admin.send(Request::get("/"), "").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));

        for wrong in ["admin:wrong", "root:secret", "garbage"] {
            let response = admin.send(Admin::login(Request::get("/"), wrong), "").await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{wrong}");
        }
        let response = admin.send(Request::get("/healthz"), "").await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = admin.get("/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[sqlx::test]
    async fn logging_out_asks_for_the_password_again(db: PgPool) {
        let admin = Admin::new(db);
        // One router throughout, as it remembers the login.
        let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
        let send = |method: &str, uri: &str| {
            let request = Admin::login(Request::builder().method(method).uri(uri), "admin:secret")
                .header(header::ACCEPT_LANGUAGE, "en-GB")
                .header("sec-fetch-site", "same-origin")
                .header("sec-fetch-mode", "navigate")
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(request)
        };
        let response = send("POST", "/logout").await.unwrap();
        assert_eq!(location(&response), "/logged-out");
        let response = admin.send(Request::get("/logged-out"), "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body(response).await.contains("<h1>Logged out</h1>"));

        let response = send("GET", "/").await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
        assert!(body(response).await.contains("<h1>Log in</h1>"));
        // The browser asked, and the password was typed again.
        let response = send("GET", "/").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[sqlx::test]
    async fn speaks_the_browsers_language_or_the_one_picked(db: PgPool) {
        let admin = Admin::new(db);
        let request = Admin::login(Request::get("/"), "admin:secret")
            .header(header::ACCEPT_LANGUAGE, "nn-NO")
            .body(Body::empty())
            .unwrap();
        let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
        let html = body(app.oneshot(request).await.unwrap()).await;
        assert!(html.contains(r#"<html lang="nn""#), "{html}");
        assert!(html.contains("Nye invitasjonar"), "{html}");
        // The browser's language needs no carrying.
        assert!(html.contains(r#"action="/invitations""#), "{html}");

        // A picked one is carried, through forms and redirects too.
        let html = body(admin.get("/?lang=zh-hant&theme=dark").await).await;
        assert!(html.contains("新邀請"), "{html}");
        assert!(
            html.contains(r#"action="/invitations?lang=zh-hant&#38;theme=dark""#),
            "{html}"
        );
        let response = admin
            .post(
                "/groups?lang=zh-hant",
                "names=%E9%84%B0%E5%B1%85&max_party_size=1",
            )
            .await;
        assert_eq!(location(&response), "/?groups=1&lang=zh-hant#groups");
        let html = body(admin.get(location(&response)).await).await;
        assert!(html.contains("已建立 1 個群組。"), "{html}");
        // A form with a mistake says so in the picked language.
        let response = admin
            .post("/invitations?lang=nb", "names=Kari%2C+0&max_party_size=1")
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
        let html = body(admin.get("/").await).await;
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
        assert_eq!(location(&response), "/?qr#qr");
        let html = body(admin.get("/?qr").await).await;
        assert!(html.contains("QR code style saved."), "{html}");
        assert!(html.contains(r#"value="dots" checked"#), "{html}");
        assert!(html.contains("🎉</text>"), "{html}");

        admin
            .post("/invitations", "names=Kari+Nordmann&max_party_size=1")
            .await;
        let id = rsvp::list(&admin.db).await.unwrap()[0].id;
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
        let html = body(admin.get("/?theme=dark").await).await;
        assert!(html.contains(r#"data-theme="dark""#), "{html}");
        assert!(
            html.contains(r#"action="/invitations?theme=dark""#),
            "{html}"
        );
        assert!(html.contains(r#"action="/logout?theme=dark""#), "{html}");

        let response = admin
            .post("/invitations?theme=dark", "names=Eva&max_party_size=1")
            .await;
        assert!(
            location(&response).ends_with("&theme=dark"),
            "{}",
            location(&response)
        );
        let response = admin
            .post("/groups?theme=light", "names=Climbers&max_party_size=1")
            .await;
        assert_eq!(location(&response), "/?groups=1&theme=light#groups");

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
        let response = admin
            .post(
                "/invitations",
                "names=Kari+%26+Ola%2C+2%0D%0A%0D%0APer+Hansen%0D%0ANordmann%2C+Anne&max_party_size=1",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let page = location(&response).to_owned();
        assert!(page.starts_with("/invitations?from="), "{page}");

        let html = body(admin.get(&page).await).await;
        assert!(html.contains("3 invitations created."), "{html}");
        let invites = rsvp::list(&admin.db).await.unwrap();
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
            .post("/invitations", "names=Eva&max_party_size=1")
            .await;
        let html = body(admin.get(location(&response)).await).await;
        assert_eq!(links(&html).len(), 1);
        let html = body(admin.get("/invitations").await).await;
        assert_eq!(links(&html).len(), 4);
    }

    #[sqlx::test]
    async fn rejects_a_batch_with_an_invalid_line(db: PgPool) {
        let admin = Admin::new(db);
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
            let response = admin.post("/invitations", form).await;
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
                .post("/invitations", "names=Kari%0D%0AOla%2C+21&max_party_size=2")
                .await,
        )
        .await;
        assert!(html.contains(">Kari\r\nOla, 21</textarea>"), "{html}");
        assert!(rsvp::list(&admin.db).await.unwrap().is_empty());
    }

    #[sqlx::test]
    async fn edits_revokes_and_deletes(db: PgPool) {
        let admin = Admin::new(db);
        let invite = rsvp::create_invite(&admin.db, "Kari", 1).await.unwrap();
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
        assert_eq!(location(&response), "/");
        assert!(rsvp::find(&admin.db, invite.id).await.unwrap().is_none());
        assert_eq!(admin.get(&path).await.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn exports_csv_with_links(db: PgPool) {
        let admin = Admin::new(db);
        rsvp::create_invite(&admin.db, "=cmd|' /C calc'!A0", 1)
            .await
            .unwrap();
        rsvp::create_invite(&admin.db, "Kari \"K\" Ø", 2)
            .await
            .unwrap();

        let response = admin.get("/guests.csv").await;
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
        let response = admin
            .post(
                "/groups",
                "names=Climbing+club%0D%0ANeighbours%2C+4&max_party_size=2",
            )
            .await;
        assert_eq!(location(&response), "/?groups=2#groups");
        let html = body(admin.get("/?groups=2").await).await;
        assert!(html.contains("2 groups created."));
        let [climbing, neighbours] = group::list(&admin.db).await.unwrap().try_into().unwrap();
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
        let html = body(admin.get("/").await).await;
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
        assert_eq!(location(&response), "/#groups");
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
        let url = invite(&config, &db, "  Per  ", 2).await.unwrap();
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

        assert!(super::invite(&config, &db, "Per", 0).await.is_err());
    }
}
