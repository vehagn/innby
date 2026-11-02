//! The guest's personal page at `/r/{link}`: answer, change, export and delete.

use askama::Template;
use axum::{
    Form, Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{EventView, MessagePage, render, render_with, server_error},
};
use crate::{
    db::rsvp::{self, Rsvp},
    i18n::{Lang, Messages},
    link::RsvpLink,
};

#[derive(Template)]
#[template(path = "rsvp.html")]
struct RsvpPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
    link: &'a str,
    rsvp: &'a Rsvp,
    /// The guest's current answer, e.g. "Coming · 2 people".
    status: String,
    /// Party size choices: value, label and whether it's the current one.
    party_sizes: Vec<(i32, String, bool)>,
    first_visit: bool,
    accepts_answers: bool,
    saved: bool,
    error: Option<&'static str>,
}

impl<'a> RsvpPage<'a> {
    fn new(state: &'a AppState, lang: Lang, link: &'a str, rsvp: &'a Rsvp) -> Self {
        let t = lang.messages();
        let status = match rsvp.response {
            rsvp::Response::Pending => t.answer_pending.to_owned(),
            rsvp::Response::Attending => {
                format!(
                    "{} · {}",
                    t.answer_attending,
                    t.people_count(rsvp.party_size)
                )
            }
            rsvp::Response::Declined => t.answer_declined.to_owned(),
        };
        Self {
            lang,
            t,
            event: EventView::new(&state.config.event, lang),
            link,
            rsvp,
            status,
            party_sizes: (1..=rsvp.max_party_size)
                .map(|n| (n, t.people_count(n), n == rsvp.party_size))
                .collect(),
            first_visit: rsvp.privacy_version.is_none(),
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
            saved: false,
            error: None,
        }
    }

    fn with_error(mut self, pick: impl Fn(&Messages) -> &'static str) -> Self {
        self.error = Some(pick(self.t));
        self
    }
}

/// Malformed, forged, revoked and deleted links all get this same page.
fn gone(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (t.gone_title, t.gone_text)),
    )
}

/// Why a link didn't lead to a guest.
enum LoadError {
    Gone(Lang),
    Database(sqlx::Error),
}

impl IntoResponse for LoadError {
    fn into_response(self) -> Response {
        match self {
            LoadError::Gone(lang) => gone(lang),
            LoadError::Database(err) => server_error(err),
        }
    }
}

/// Finds the guest behind a link.
async fn load(state: &AppState, lang: Lang, link: &str) -> Result<Rsvp, LoadError> {
    let parsed = link
        .parse::<RsvpLink>()
        .map_err(|()| LoadError::Gone(lang))?;
    match rsvp::find(&state.db, parsed.id).await {
        Ok(Some(rsvp)) if state.config.link_key.verify_rsvp(&parsed, rsvp.link_nonce) => Ok(rsvp),
        Ok(_) => Err(LoadError::Gone(lang)),
        Err(err) => Err(LoadError::Database(err)),
    }
}

fn saved_redirect(link: &str, lang: Lang) -> Response {
    Redirect::to(&format!("/r/{link}?lang={}&saved", lang.code())).into_response()
}

#[derive(Deserialize)]
pub struct ShowQuery {
    saved: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
    Query(query): Query<ShowQuery>,
) -> Response {
    let rsvp = match load(&state, lang, &link).await {
        Ok(rsvp) => rsvp,
        Err(err) => return err.into_response(),
    };
    let mut page = RsvpPage::new(&state, lang, &link, &rsvp);
    page.saved = query.saved.is_some();
    render(page)
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Attending,
    Declined,
}

#[derive(Deserialize)]
pub struct AnswerForm {
    response: Answer,
    party_size: i32,
}

pub async fn answer(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
    Form(form): Form<AnswerForm>,
) -> Response {
    let rsvp = match load(&state, lang, &link).await {
        Ok(rsvp) => rsvp,
        Err(err) => return err.into_response(),
    };
    let page = || RsvpPage::new(&state, lang, &link, &rsvp);
    if !state.config.event.accepts_answers(Utc::now()) {
        return render_with(
            StatusCode::FORBIDDEN,
            page().with_error(|t| t.answers_closed),
        );
    }
    if !(1..=rsvp.max_party_size).contains(&form.party_size) {
        return render_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            page().with_error(|t| t.invalid_party_size),
        );
    }

    let response = match form.response {
        Answer::Attending => rsvp::Response::Attending,
        Answer::Declined => rsvp::Response::Declined,
    };
    let result = rsvp::respond(
        &state.db,
        rsvp.id,
        response,
        form.party_size,
        lang,
        PRIVACY_VERSION,
    )
    .await;
    match result {
        Ok(Some(_)) => {
            tracing::info!(rsvp = %rsvp.id, ?response, "rsvp answered");
            saved_redirect(&link, lang)
        }
        // Deleted in the meantime.
        Ok(None) => gone(lang),
        Err(err) => server_error(err),
    }
}

#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
    Form(form): Form<NameForm>,
) -> Response {
    let rsvp = match load(&state, lang, &link).await {
        Ok(rsvp) => rsvp,
        Err(err) => return err.into_response(),
    };
    let Some(name) = rsvp::normalize_name(&form.name) else {
        return render_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            RsvpPage::new(&state, lang, &link, &rsvp).with_error(|t| t.invalid_name),
        );
    };
    match rsvp::rename(&state.db, rsvp.id, &name).await {
        Ok(_) => {
            tracing::info!(rsvp = %rsvp.id, "rsvp renamed");
            saved_redirect(&link, lang)
        }
        Err(err) => server_error(err),
    }
}

/// Everything stored about the guest, as a download (GDPR Art. 15 and 20).
pub async fn export(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
) -> Response {
    match load(&state, lang, &link).await {
        Ok(rsvp) => (
            [(
                header::CONTENT_DISPOSITION,
                r#"attachment; filename="rsvp.json""#,
            )],
            Json(rsvp),
        )
            .into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: Option<String>,
}

/// Deletes the RSVP for good (GDPR Art. 17).
pub async fn delete(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let rsvp = match load(&state, lang, &link).await {
        Ok(rsvp) => rsvp,
        Err(err) => return err.into_response(),
    };
    if form.confirm.is_none() {
        return render_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            RsvpPage::new(&state, lang, &link, &rsvp).with_error(|t| t.confirm_delete),
        );
    }
    match rsvp::delete(&state.db, rsvp.id).await {
        Ok(_) => {
            tracing::info!(rsvp = %rsvp.id, "rsvp deleted by guest");
            render(MessagePage::new(lang, |t| {
                (t.deleted_title, t.deleted_text)
            }))
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, tests::parse};

    struct Site {
        db: PgPool,
        config: Config,
    }

    impl Site {
        fn new(db: PgPool) -> Self {
            Self::with(db, &[])
        }

        fn with(db: PgPool, overrides: &[(&str, &str)]) -> Self {
            Self {
                db,
                config: parse(overrides).unwrap(),
            }
        }

        async fn invite(&self, name: &str, max_party_size: i32) -> (Rsvp, String) {
            let rsvp = rsvp::create_invite(&self.db, name, max_party_size)
                .await
                .unwrap();
            let link = self.config.link_key.rsvp_link(rsvp.id, rsvp.link_nonce);
            (rsvp, link)
        }

        async fn send(&self, request: Request<Body>) -> Response {
            super::super::router(self.config.clone(), self.db.clone())
                .oneshot(request)
                .await
                .unwrap()
        }

        async fn get(&self, uri: &str) -> Response {
            self.send(Request::get(uri).body(Body::empty()).unwrap())
                .await
        }

        /// A same-origin form post, as a browser sends it.
        async fn post(&self, uri: &str, form: &str) -> Response {
            self.send(form_post(uri, form, "same-origin")).await
        }

        async fn find(&self, rsvp: &Rsvp) -> Option<Rsvp> {
            rsvp::find(&self.db, rsvp.id).await.unwrap()
        }
    }

    fn form_post(uri: &str, form: &str, fetch_site: &str) -> Request<Body> {
        Request::post(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("sec-fetch-site", fetch_site)
            .body(Body::from(form.to_owned()))
            .unwrap()
    }

    async fn body(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[sqlx::test]
    async fn guest_answers_through_their_link(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari & Ola", 2).await;

        let response = site.get(&format!("/r/{link}")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex");
        let html = body(response).await;
        assert!(html.contains("Hei, Kari &#38; Ola!"), "{html}");
        assert!(html.contains("Du er invitert!"));
        assert!(html.contains("Ikke svart ennå"));
        assert!(html.contains(r#"<option value="2">2 personer</option>"#));

        let response = site
            .post(
                &format!("/r/{link}?lang=en"),
                "party_size=2&response=attending",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            format!("/r/{link}?lang=en&saved").as_str()
        );

        let answered = site.find(&invite).await.unwrap();
        assert_eq!(answered.response, rsvp::Response::Attending);
        assert_eq!((answered.party_size, answered.lang), (2, Lang::En));
        assert_eq!(answered.privacy_version.as_deref(), Some(PRIVACY_VERSION));

        let html = body(site.get(&format!("/r/{link}?lang=en&saved")).await).await;
        assert!(html.contains("Saved!"));
        assert!(html.contains("Coming · 2 people"));
        assert!(!html.contains("You're invited!"));

        site.post(&format!("/r/{link}"), "party_size=1&response=declined")
            .await;
        let declined = site.find(&invite).await.unwrap();
        assert_eq!(declined.response, rsvp::Response::Declined);
    }

    #[sqlx::test]
    async fn bad_links_all_look_the_same(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;
        let (_, other) = site.invite("Ola", 1).await;
        let forged = format!("{}{}", &link[..23], &other[23..]);
        rsvp::revoke_link(&site.db, invite.id).await.unwrap();

        for bad in [link.as_str(), &forged, "garbage", "a.b"] {
            let response = site.get(&format!("/r/{bad}")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
            assert!(body(response).await.contains("Denne lenken virker ikke"));
        }
        let response = site
            .post(&format!("/r/{link}"), "party_size=1&response=attending")
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            site.find(&invite).await.unwrap().response,
            rsvp::Response::Pending
        );

        let fresh = site.config.link_key.rsvp_link(invite.id, 1);
        assert_eq!(
            site.get(&format!("/r/{fresh}")).await.status(),
            StatusCode::OK
        );
    }

    #[sqlx::test]
    async fn party_size_is_limited_to_the_invitation(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 2).await;
        for size in ["3", "0"] {
            let response = site
                .post(
                    &format!("/r/{link}"),
                    &format!("party_size={size}&response=attending"),
                )
                .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(body(response).await.contains("Velg et gyldig antall."));
        }
        assert_eq!(
            site.find(&invite).await.unwrap().response,
            rsvp::Response::Pending
        );
    }

    #[sqlx::test]
    async fn cross_site_posts_are_rejected(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;
        let response = site
            .send(form_post(
                &format!("/r/{link}/delete"),
                "confirm=yes",
                "cross-site",
            ))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(site.find(&invite).await.is_some());
    }

    #[sqlx::test]
    async fn answers_close_at_the_deadline(db: PgPool) {
        let site = Site::with(db, &[("FEST_RSVP_DEADLINE", "2020-01-01T12:00")]);
        let (invite, link) = site.invite("Kari", 1).await;

        let html = body(site.get(&format!("/r/{link}")).await).await;
        assert!(html.contains("Svarfristen har gått ut."));
        assert!(!html.contains(r#"value="attending""#));

        let response = site
            .post(&format!("/r/{link}"), "party_size=1&response=attending")
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            site.find(&invite).await.unwrap().response,
            rsvp::Response::Pending
        );
    }

    #[sqlx::test]
    async fn guest_can_rename(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;

        let response = site
            .post(&format!("/r/{link}/name"), "name=++Kari++Nordmann+")
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(site.find(&invite).await.unwrap().name, "Kari Nordmann");

        let response = site.post(&format!("/r/{link}/name"), "name=+++").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(site.find(&invite).await.unwrap().name, "Kari Nordmann");
    }

    #[sqlx::test]
    async fn guest_can_export_their_data(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;

        let response = site.get(&format!("/r/{link}/export")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment")
        );
        let json: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
        assert_eq!(json["id"], invite.id.to_string());
        assert_eq!(json["name"], "Kari");
        assert_eq!(json["response"], "pending");
        assert_eq!(json["email"], serde_json::Value::Null);
    }

    #[sqlx::test]
    async fn guest_can_delete_everything(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;

        let response = site.post(&format!("/r/{link}/delete"), "").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(site.find(&invite).await.is_some());

        let response = site
            .post(&format!("/r/{link}/delete?lang=en"), "confirm=yes")
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body(response).await.contains("Your RSVP has been deleted"));
        assert!(site.find(&invite).await.is_none());
        assert_eq!(
            site.get(&format!("/r/{link}")).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
