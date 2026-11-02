//! `fest admin`: the host's UI for invitations and the headcount. English only, and
//! served on its own listener behind the internal gateway and basic auth.

pub mod auth;

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
use qrcode::{QrCode, render::svg};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::{AdminConfig, Config},
    db::{
        outbox,
        rsvp::{self, EmailStatus, Headcount, MAX_PARTY_SIZE, Rsvp, Source},
    },
    web::{self, render, render_with, server_error},
};

#[derive(Clone)]
struct AdminState {
    config: Arc<Config>,
    db: PgPool,
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
    let https = config.base_url.starts_with("https://");
    let state = AdminState {
        config: Arc::new(config),
        db,
    };
    let credentials = Arc::new(auth::Credentials::from(admin));
    let pages = Router::new()
        .route("/", get(index))
        .route("/invitations", post(create))
        .route("/rsvp/{id}", get(show).post(edit))
        .route("/rsvp/{id}/revoke", post(revoke))
        .route("/rsvp/{id}/delete", post(delete))
        .route("/rsvp.csv", get(export_csv))
        .route("/reminders", post(queue_reminders))
        .route("/static/style.css", get(web::stylesheet))
        .layer(middleware::from_fn_with_state(
            credentials,
            auth::require_login,
        ));
    let router = web::no_store(pages)
        // Probes don't log in.
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(web::ready))
        .with_state(state);
    web::harden(router, https)
}

/// One guest as the admin tables show it.
struct Row {
    id: Uuid,
    name: String,
    source: &'static str,
    answer: &'static str,
    party: String,
    email: &'static str,
    responded: String,
    created: String,
}

impl Row {
    fn new(rsvp: &Rsvp, tz: Tz) -> Self {
        Self {
            id: rsvp.id,
            name: rsvp.name.clone(),
            source: match rsvp.source {
                Source::Invite => "invitation",
                Source::Open => "open RSVP",
            },
            answer: match rsvp.response {
                rsvp::Response::Pending => "no answer",
                rsvp::Response::Attending => "coming",
                rsvp::Response::Declined => "not coming",
            },
            party: format!("{} / {}", rsvp.party_size, rsvp.max_party_size),
            email: match rsvp.email_status {
                EmailStatus::None => "",
                EmailStatus::Unconfirmed => "unconfirmed",
                EmailStatus::Confirmed => "confirmed",
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
    notice: Option<String>,
    rows: Vec<Row>,
    max_party_size: i32,
    error: Option<&'static str>,
}

async fn index_page(
    state: &AdminState,
    error: Option<&'static str>,
    notice: Option<String>,
) -> Response {
    let (rsvps, headcount) =
        match tokio::try_join!(rsvp::list(&state.db), rsvp::headcount(&state.db)) {
            Ok(result) => result,
            Err(err) => return server_error(err),
        };
    let tz = state.config.event.starts_at.timezone();
    let page = IndexPage {
        title: &state.config.event.title.nb,
        headcount,
        subscribers: rsvps
            .iter()
            .filter(|rsvp| rsvp.email_status == EmailStatus::Confirmed)
            .count(),
        notice,
        rows: rsvps.iter().map(|rsvp| Row::new(rsvp, tz)).collect(),
        max_party_size: MAX_PARTY_SIZE,
        error,
    };
    match error {
        Some(_) => render_with(StatusCode::UNPROCESSABLE_ENTITY, page),
        None => render(page),
    }
}

#[derive(Deserialize)]
struct IndexQuery {
    reminders: Option<u64>,
}

async fn index(State(state): State<AdminState>, Query(query): Query<IndexQuery>) -> Response {
    let notice = query.reminders.map(|count| match count {
        1 => "1 reminder queued.".to_owned(),
        count => format!("{count} reminders queued."),
    });
    index_page(&state, None, notice).await
}

/// Queues a reminder for everyone with a confirmed address who hasn't declined. Each
/// guest gets at most one.
async fn queue_reminders(State(state): State<AdminState>) -> Response {
    match outbox::queue_reminders(&state.db).await {
        Ok(count) => {
            tracing::info!(count, "reminders queued");
            Redirect::to(&format!("/?reminders={count}")).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[derive(Deserialize)]
struct InviteForm {
    name: String,
    max_party_size: i32,
}

impl InviteForm {
    fn validate(&self) -> Result<String, &'static str> {
        if !(1..=MAX_PARTY_SIZE).contains(&self.max_party_size) {
            return Err("The party size must be between 1 and 20.");
        }
        rsvp::normalize_name(&self.name).ok_or("The name must be between 1 and 100 characters.")
    }
}

async fn create(State(state): State<AdminState>, Form(form): Form<InviteForm>) -> Response {
    let name = match form.validate() {
        Ok(name) => name,
        Err(error) => return index_page(&state, Some(error), None).await,
    };
    match rsvp::create_invite(&state.db, &name, form.max_party_size).await {
        Ok(invite) => {
            tracing::info!(rsvp = %invite.id, "invitation created");
            Redirect::to(&format!("/rsvp/{}", invite.id)).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "admin/rsvp.html")]
struct RsvpPage {
    rsvp: Rsvp,
    row: Row,
    url: String,
    qr: String,
    max_party_size: i32,
    saved: bool,
    error: Option<&'static str>,
}

impl RsvpPage {
    fn new(config: &Config, rsvp: Rsvp) -> Self {
        let url = guest_url(config, &rsvp);
        Self {
            row: Row::new(&rsvp, config.event.starts_at.timezone()),
            qr: qr_svg(&url),
            url,
            rsvp,
            max_party_size: MAX_PARTY_SIZE,
            saved: false,
            error: None,
        }
    }
}

fn guest_url(config: &Config, rsvp: &Rsvp) -> String {
    let link = config.link_key.rsvp_link(rsvp.id, rsvp.link_nonce);
    format!("{}/r/{link}", config.base_url)
}

/// The link as an inline SVG QR code, for printing on paper invitations.
fn qr_svg(url: &str) -> String {
    let Ok(code) = QrCode::new(url.as_bytes()) else {
        return String::new();
    };
    let svg = code.render::<svg::Color>().min_dimensions(200, 200).build();
    // Drop the XML declaration so the SVG can sit inside HTML.
    svg.find("<svg")
        .map_or(svg.clone(), |start| svg[start..].to_owned())
}

fn no_such_guest() -> Response {
    (StatusCode::NOT_FOUND, "no such guest").into_response()
}

/// Why a guest couldn't be shown.
enum FindError {
    Missing,
    Database(sqlx::Error),
}

impl IntoResponse for FindError {
    fn into_response(self) -> Response {
        match self {
            FindError::Missing => no_such_guest(),
            FindError::Database(err) => server_error(err),
        }
    }
}

async fn find(state: &AdminState, id: Uuid) -> Result<Rsvp, FindError> {
    rsvp::find(&state.db, id)
        .await
        .map_err(FindError::Database)?
        .ok_or(FindError::Missing)
}

#[derive(Deserialize)]
struct ShowQuery {
    saved: Option<String>,
}

async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ShowQuery>,
) -> Response {
    let rsvp = match find(&state, id).await {
        Ok(rsvp) => rsvp,
        Err(err) => return err.into_response(),
    };
    let mut page = RsvpPage::new(&state.config, rsvp);
    page.saved = query.saved.is_some();
    render(page)
}

fn saved_redirect(id: Uuid) -> Response {
    Redirect::to(&format!("/rsvp/{id}?saved")).into_response()
}

async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<InviteForm>,
) -> Response {
    let name = match form.validate() {
        Ok(name) => name,
        Err(error) => {
            return match find(&state, id).await {
                Ok(rsvp) => {
                    let mut page = RsvpPage::new(&state.config, rsvp);
                    page.error = Some(error);
                    render_with(StatusCode::UNPROCESSABLE_ENTITY, page)
                }
                Err(err) => err.into_response(),
            };
        }
    };
    match rsvp::update_invite(&state.db, id, &name, form.max_party_size).await {
        Ok(true) => saved_redirect(id),
        Ok(false) => no_such_guest(),
        Err(err) => server_error(err),
    }
}

async fn revoke(State(state): State<AdminState>, Path(id): Path<Uuid>) -> Response {
    match rsvp::revoke_link(&state.db, id).await {
        Ok(Some(_)) => {
            tracing::info!(rsvp = %id, "link revoked");
            saved_redirect(id)
        }
        Ok(None) => no_such_guest(),
        Err(err) => server_error(err),
    }
}

#[derive(Deserialize)]
struct DeleteForm {
    confirm: Option<String>,
}

async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DeleteForm>,
) -> Response {
    if form.confirm.is_none() {
        return match find(&state, id).await {
            Ok(rsvp) => {
                let mut page = RsvpPage::new(&state.config, rsvp);
                page.error = Some("Tick the box to confirm the deletion.");
                render_with(StatusCode::UNPROCESSABLE_ENTITY, page)
            }
            Err(err) => err.into_response(),
        };
    }
    match rsvp::delete(&state.db, id).await {
        Ok(_) => {
            tracing::info!(rsvp = %id, "rsvp deleted by host");
            Redirect::to("/").into_response()
        }
        Err(err) => server_error(err),
    }
}

/// The guest list as CSV, with each guest's link for a mail merge.
async fn export_csv(State(state): State<AdminState>) -> Response {
    let rsvps = match rsvp::list(&state.db).await {
        Ok(rsvps) => rsvps,
        Err(err) => return server_error(err),
    };
    let tz = state.config.event.starts_at.timezone();
    // The byte order mark makes spreadsheet apps read the file as UTF-8 (æ, ø, å).
    let mut csv = String::from("\u{feff}");
    csv_line(
        &mut csv,
        &[
            "name",
            "source",
            "answer",
            "people",
            "max people",
            "e-mail",
            "answered",
            "link",
        ],
    );
    for rsvp in &rsvps {
        let row = Row::new(rsvp, tz);
        csv_line(
            &mut csv,
            &[
                &row.name,
                row.source,
                row.answer,
                &rsvp.party_size.to_string(),
                &rsvp.max_party_size.to_string(),
                row.email,
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

/// `fest invite`: creates an invitation and returns the guest's link.
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
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::{config::tests::config, link::RsvpLink};

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
            let request = request
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
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

    async fn body(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn location(response: &Response) -> &str {
        response.headers()[header::LOCATION].to_str().unwrap()
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
    async fn creates_an_invitation_with_a_working_link(db: PgPool) {
        let admin = Admin::new(db);
        let response = admin
            .post("/invitations", "name=Kari+%26+Ola&max_party_size=2")
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let detail = location(&response).to_owned();

        let html = body(admin.get(&detail).await).await;
        assert!(html.contains("<h1>Kari &#38; Ola</h1>"), "{html}");
        assert!(html.contains("<svg"));
        let url = html
            .split("value=\"https://fest.example.com/r/")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("guest link on the page");
        let link: RsvpLink = url.parse().unwrap();
        let invite = rsvp::find(&admin.db, link.id).await.unwrap().unwrap();
        assert!(admin.config.link_key.verify_rsvp(&link, invite.link_nonce));
        assert_eq!(
            (invite.name.as_str(), invite.max_party_size),
            ("Kari & Ola", 2)
        );

        let html = body(admin.get("/").await).await;
        assert!(html.contains("Kari &#38; Ola"));
        assert!(html.contains("no answer"));
    }

    #[sqlx::test]
    async fn rejects_invalid_invitations(db: PgPool) {
        let admin = Admin::new(db);
        for form in ["name=+&max_party_size=2", "name=Kari&max_party_size=21"] {
            let response = admin.post("/invitations", form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
        }
        assert!(rsvp::list(&admin.db).await.unwrap().is_empty());
    }

    #[sqlx::test]
    async fn edits_revokes_and_deletes(db: PgPool) {
        let admin = Admin::new(db);
        let invite = rsvp::create_invite(&admin.db, "Kari", 1).await.unwrap();
        let path = format!("/rsvp/{}", invite.id);

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

        let response = admin.get("/rsvp.csv").await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        let csv = body(response).await;
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with(r#""name","source","answer""#));
        assert!(lines[1].starts_with(r#""'=cmd|' /C calc'!A0","invitation","no answer","1","1""#));
        assert!(lines[2].starts_with(r#""Kari ""K"" Ø","#));
        assert!(lines[2].contains(r#""https://fest.example.com/r/"#));
    }

    #[sqlx::test]
    async fn invite_command_prints_a_working_link(db: PgPool) {
        let config = config();
        let url = invite(&config, &db, "  Per  ", 2).await.unwrap();
        let link: RsvpLink = url
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        let invite = rsvp::find(&db, link.id).await.unwrap().unwrap();
        assert_eq!(invite.name, "Per");
        assert!(config.link_key.verify_rsvp(&link, invite.link_nonce));

        assert!(super::invite(&config, &db, "Per", 0).await.is_err());
    }
}
