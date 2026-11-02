//! The guest's personal page at `/r/{link}`: answer, change, export and delete.

use askama::Template;
use axum::{
    Form, Json,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{EventView, MessagePage, PartySize, render, render_with, server_error},
};
use crate::{
    db::rsvp::{self, Rsvp},
    i18n::{Lang, Messages},
    link::RsvpLink,
};

/// The guest behind the `{link}` in the path. Malformed, forged, revoked and deleted
/// links are all turned away with the same page.
pub(super) struct Guest {
    pub link: String,
    pub rsvp: Rsvp,
}

impl FromRequestParts<AppState> for Guest {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let Ok(lang) = Lang::from_request_parts(parts, state).await;
        let Path(link) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| gone(lang))?;
        let Ok(parsed) = link.parse::<RsvpLink>() else {
            return Err(gone(lang));
        };
        match rsvp::find(&state.db, parsed.id).await {
            Ok(Some(rsvp)) if state.config.link_key.verify_rsvp(&parsed, rsvp.link_nonce) => {
                Ok(Self { link, rsvp })
            }
            Ok(_) => Err(gone(lang)),
            Err(err) => Err(server_error(err)),
        }
    }
}

/// The page every link that doesn't lead to a guest gets.
fn gone(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (t.gone_title, t.gone_text)),
    )
}

#[derive(Template)]
#[template(path = "rsvp.html")]
pub(super) struct RsvpPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
    link: &'a str,
    rsvp: &'a Rsvp,
    /// The guest's current answer, e.g. "Coming · 2 people".
    status: String,
    party_sizes: Vec<PartySize>,
    first_visit: bool,
    accepts_answers: bool,
    email_confirmed: bool,
    /// A one-off confirmation after a change, e.g. "Saved!".
    flash: Option<&'static str>,
    /// The full link, shown right after an open RSVP so the guest can keep it.
    new_link: Option<String>,
    error: Option<&'static str>,
}

impl<'a> RsvpPage<'a> {
    pub(super) fn new(state: &'a AppState, lang: Lang, guest: &'a Guest) -> Self {
        let t = lang.messages();
        let rsvp = &guest.rsvp;
        Self {
            lang,
            t,
            event: EventView::new(&state.config.event, lang),
            link: &guest.link,
            rsvp,
            status: t.answer(rsvp.response, rsvp.party_size),
            party_sizes: PartySize::choices(t, rsvp.max_party_size, rsvp.party_size),
            first_visit: rsvp.privacy_version.is_none(),
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
            email_confirmed: rsvp.email_status == rsvp::EmailStatus::Confirmed,
            flash: None,
            new_link: None,
            error: None,
        }
    }

    /// The page again with an error message, e.g. for an invalid form.
    pub(super) fn render_error(
        mut self,
        status: StatusCode,
        pick: impl Fn(&Messages) -> &'static str,
    ) -> Response {
        self.error = Some(pick(self.t));
        render_with(status, self)
    }
}

/// What the RSVP page confirms after a change, passed as `?notice=`.
#[derive(Clone, Copy)]
pub(super) enum Notice {
    Saved,
    EmailSent,
    EmailRemoved,
    EmailConfirmed,
}

impl Notice {
    const ALL: [Self; 4] = [
        Self::Saved,
        Self::EmailSent,
        Self::EmailRemoved,
        Self::EmailConfirmed,
    ];

    fn code(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::EmailSent => "email_sent",
            Self::EmailRemoved => "email_removed",
            Self::EmailConfirmed => "email_confirmed",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notice| notice.code() == code)
    }

    fn text(self, t: &Messages) -> &'static str {
        match self {
            Self::Saved => t.saved,
            Self::EmailSent => t.email_sent_notice,
            Self::EmailRemoved => t.email_removed_notice,
            Self::EmailConfirmed => t.email_confirmed_notice,
        }
    }
}

/// Back to the RSVP page after a change (post/redirect/get).
pub(super) fn redirect(link: &str, lang: Lang, notice: Notice) -> Response {
    Redirect::to(&format!(
        "/r/{link}?lang={}&notice={}",
        lang.code(),
        notice.code()
    ))
    .into_response()
}

#[derive(Deserialize)]
pub struct ShowQuery {
    notice: Option<String>,
    /// Set right after an open RSVP.
    new: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Query(query): Query<ShowQuery>,
) -> Response {
    let mut page = RsvpPage::new(&state, lang, &guest);
    page.flash = query
        .notice
        .as_deref()
        .and_then(Notice::from_code)
        .map(|notice| notice.text(page.t));
    page.new_link = query
        .new
        .map(|_| format!("{}/r/{}", state.config.base_url, guest.link));
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
    guest: Guest,
    Form(form): Form<AnswerForm>,
) -> Response {
    let page = || RsvpPage::new(&state, lang, &guest);
    if !state.config.event.accepts_answers(Utc::now()) {
        return page().render_error(StatusCode::FORBIDDEN, |t| t.answers_closed);
    }
    if !(1..=guest.rsvp.max_party_size).contains(&form.party_size) {
        return page().render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| t.invalid_party_size);
    }

    let response = match form.response {
        Answer::Attending => rsvp::Response::Attending,
        Answer::Declined => rsvp::Response::Declined,
    };
    let id = guest.rsvp.id;
    match rsvp::respond(
        &state.db,
        id,
        response,
        form.party_size,
        lang,
        PRIVACY_VERSION,
    )
    .await
    {
        Ok(Some(_)) => {
            tracing::info!(rsvp = %id, ?response, "rsvp answered");
            redirect(&guest.link, lang, Notice::Saved)
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
    guest: Guest,
    Form(form): Form<NameForm>,
) -> Response {
    let Some(name) = rsvp::normalize_name(&form.name) else {
        return RsvpPage::new(&state, lang, &guest)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| t.invalid_name);
    };
    match rsvp::rename(&state.db, guest.rsvp.id, &name).await {
        Ok(true) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp renamed");
            redirect(&guest.link, lang, Notice::Saved)
        }
        Ok(false) => gone(lang),
        Err(err) => server_error(err),
    }
}

/// Everything stored about the guest, as a download (GDPR Art. 15 and 20).
pub async fn export(guest: Guest) -> Response {
    (
        [(
            header::CONTENT_DISPOSITION,
            r#"attachment; filename="rsvp.json""#,
        )],
        Json(guest.rsvp),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: Option<String>,
}

/// Deletes the RSVP for good (GDPR Art. 17).
pub async fn delete(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Form(form): Form<DeleteForm>,
) -> Response {
    if form.confirm.is_none() {
        return RsvpPage::new(&state, lang, &guest)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| t.confirm_delete);
    }
    match rsvp::delete(&state.db, guest.rsvp.id).await {
        Ok(_) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp deleted by guest");
            render(MessagePage::new(lang, |t| {
                (t.deleted_title, t.deleted_text)
            }))
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::web::testing::{Site, body, form_post};

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
            format!("/r/{link}?lang=en&notice=saved").as_str()
        );

        let answered = site.find(&invite).await.unwrap();
        assert_eq!(answered.response, rsvp::Response::Attending);
        assert_eq!((answered.party_size, answered.lang), (2, Lang::En));
        assert_eq!(answered.privacy_version.as_deref(), Some(PRIVACY_VERSION));

        let html = body(site.get(&format!("/r/{link}?lang=en&notice=saved")).await).await;
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
