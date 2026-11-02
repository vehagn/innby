//! The open RSVP at `/join/{code}`: anyone with the shared link can say they're coming
//! with just a name, and gets their own RSVP link back.

use std::net::{IpAddr, Ipv4Addr};

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{EventView, render, render_with, server_error},
    rate_limit::ClientIp,
};
use crate::{
    db::rsvp,
    i18n::{Lang, Messages},
};

/// The guest plus one.
pub const OPEN_MAX_PARTY_SIZE: i32 = 2;

#[derive(Template)]
#[template(path = "join.html")]
struct JoinPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
    code: &'a str,
    name: &'a str,
    party_sizes: Vec<(i32, String, bool)>,
    accepts_answers: bool,
    error: Option<&'static str>,
}

impl<'a> JoinPage<'a> {
    fn new(state: &'a AppState, lang: Lang, code: &'a str) -> Self {
        let t = lang.messages();
        Self {
            lang,
            t,
            event: EventView::new(&state.config.event, lang),
            code,
            name: "",
            party_sizes: (1..=OPEN_MAX_PARTY_SIZE)
                .map(|n| (n, t.people_count(n), n == 1))
                .collect(),
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
            error: None,
        }
    }
}

/// The open RSVP exists only while a code is configured, and only under that code.
fn is_open(state: &AppState, code: &str) -> bool {
    state
        .config
        .open_rsvp_code
        .as_deref()
        .is_some_and(|expected| expected == code)
}

pub async fn show(State(state): State<AppState>, lang: Lang, Path(code): Path<String>) -> Response {
    if !is_open(&state, &code) {
        return super::pages::not_found(lang).await;
    }
    render(JoinPage::new(&state, lang, &code))
}

#[derive(Deserialize)]
pub struct JoinForm {
    name: String,
    party_size: i32,
    /// Hidden from people; bots fill it in.
    #[serde(default)]
    website: String,
}

pub async fn join(
    State(state): State<AppState>,
    lang: Lang,
    ClientIp(client): ClientIp,
    Path(code): Path<String>,
    Form(form): Form<JoinForm>,
) -> Response {
    if !is_open(&state, &code) {
        return super::pages::not_found(lang).await;
    }
    if !form.website.is_empty() {
        tracing::info!("open rsvp honeypot triggered");
        return Redirect::to("/").into_response();
    }
    let mut page = JoinPage::new(&state, lang, &code);
    page.name = &form.name;
    if !page.accepts_answers {
        return render_with(StatusCode::FORBIDDEN, page);
    }
    let Some(name) = rsvp::normalize_name(&form.name) else {
        page.error = Some(page.t.invalid_name);
        return render_with(StatusCode::UNPROCESSABLE_ENTITY, page);
    };
    if !(1..=OPEN_MAX_PARTY_SIZE).contains(&form.party_size) {
        page.error = Some(page.t.invalid_party_size);
        return render_with(StatusCode::UNPROCESSABLE_ENTITY, page);
    }
    // Only valid submissions count, so a typo doesn't use up a household's allowance.
    if !state
        .join_limiter
        .check(client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    {
        tracing::warn!("open rsvp rate limit hit");
        page.error = Some(page.t.too_many_attempts);
        return render_with(StatusCode::TOO_MANY_REQUESTS, page);
    }

    let created = rsvp::create_open(
        &state.db,
        &name,
        form.party_size,
        OPEN_MAX_PARTY_SIZE,
        lang,
        PRIVACY_VERSION,
    )
    .await;
    match created {
        Ok(rsvp) => {
            tracing::info!(rsvp = %rsvp.id, "open rsvp created");
            let link = state.config.link_key.rsvp_link(rsvp.id, rsvp.link_nonce);
            Redirect::to(&format!("/r/{link}?lang={}&new", lang.code())).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::web::testing::{Site, body, location};

    const CODE: &str = "jul-2026-hemmelig";

    fn open_site(db: PgPool) -> Site {
        Site::with(db, &[("FEST_OPEN_RSVP_CODE", CODE)])
    }

    async fn all(site: &Site) -> Vec<rsvp::Rsvp> {
        rsvp::list(&site.db).await.unwrap()
    }

    #[sqlx::test]
    async fn closed_without_a_code_or_with_the_wrong_one(db: PgPool) {
        let closed = Site::new(db.clone());
        let response = closed.get(&format!("/join/{CODE}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(body(response).await.contains("Siden finnes ikke"));

        let open = open_site(db);
        assert_eq!(
            open.get("/join/wrong-code").await.status(),
            StatusCode::NOT_FOUND
        );
        let response = open.post("/join/wrong-code", "name=Per&party_size=1").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(all(&open).await.is_empty());
    }

    #[sqlx::test]
    async fn guest_joins_and_gets_their_link(db: PgPool) {
        let site = open_site(db);
        let response = site.get(&format!("/join/{CODE}?lang=en")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        let html = body(response).await;
        assert!(html.contains("Let us know you"));
        assert!(html.contains(r#"name="website""#));

        let response = site
            .post(
                &format!("/join/{CODE}?lang=en"),
                "name=+Per+Hansen+&party_size=2&website=",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let target = location(&response).to_owned();
        assert!(target.starts_with("/r/") && target.ends_with("?lang=en&new"));

        let [guest] = all(&site).await.try_into().unwrap();
        assert_eq!(guest.name, "Per Hansen");
        assert_eq!(guest.source, rsvp::Source::Open);
        assert_eq!(guest.response, rsvp::Response::Attending);
        assert_eq!((guest.party_size, guest.max_party_size), (2, 2));
        assert_eq!(guest.lang, Lang::En);

        let html = body(site.get(&target).await).await;
        assert!(html.contains("Save your link!"));
        let path = target.split('?').next().unwrap();
        assert!(html.contains(&format!(r#"value="https://fest.example.com{path}""#)));
        assert!(html.contains("Coming · 2 people"));
    }

    #[sqlx::test]
    async fn rejects_bad_input_and_bots(db: PgPool) {
        let site = open_site(db);
        let join = format!("/join/{CODE}");
        for form in ["name=Per&party_size=3", "name=+&party_size=1"] {
            let response = site.post(&join, form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
        }
        let response = site
            .post(&join, "name=Bot&party_size=1&website=http%3A%2F%2Fspam")
            .await;
        assert_eq!(location(&response), "/");
        assert!(all(&site).await.is_empty());
    }

    #[sqlx::test]
    async fn limits_sign_ups_per_client(db: PgPool) {
        let site = open_site(db);
        let join = format!("/join/{CODE}");
        for n in 0..10 {
            let response = site
                .post(&join, &format!("name=Guest+{n}&party_size=1"))
                .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
        }
        let response = site.post(&join, "name=One+more&party_size=1").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(body(response).await.contains("For mange påmeldinger"));
        assert_eq!(all(&site).await.len(), 10);
    }

    #[sqlx::test]
    async fn closes_at_the_deadline(db: PgPool) {
        let site = Site::with(
            db,
            &[
                ("FEST_OPEN_RSVP_CODE", CODE),
                ("FEST_RSVP_DEADLINE", "2020-01-01T12:00"),
            ],
        );
        let join = format!("/join/{CODE}");
        assert!(
            body(site.get(&join).await)
                .await
                .contains("Svarfristen har gått ut.")
        );
        let response = site.post(&join, "name=Per&party_size=1").await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(all(&site).await.is_empty());
    }
}
