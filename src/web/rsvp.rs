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
    pages::{
        DisplayChoice, EventView, GuestList, GuestsPage, MessagePage, PartySize, YourListing,
        render, render_with, server_error,
    },
};
use crate::{
    db::rsvp::{self, NameDisplay, Rsvp},
    i18n::{Lang, Messages},
    link::{IdLink, Purpose},
    paths::{self, GuestLinks},
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
        let Ok(parsed) = link.parse::<IdLink>() else {
            return Err(gone(lang));
        };
        match rsvp::find(&state.db, parsed.id).await {
            Ok(Some(rsvp))
                if state
                    .config
                    .link_key
                    .verify(Purpose::Rsvp, &parsed, rsvp.link_nonce) =>
            {
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
        MessagePage::new(lang, |t| (&t.gone_title, &t.gone_text)),
    )
}

#[derive(Template)]
#[template(path = "rsvp.html")]
pub(super) struct RsvpPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
    links: GuestLinks,
    rsvp: &'a Rsvp,
    /// The guest's current answer, e.g. "Coming · 2 people".
    status: String,
    party_sizes: Vec<PartySize>,
    /// For the answer form: the guest's choice, or their full name if they haven't made
    /// one, since that's the default once they're coming.
    display_choices: Vec<DisplayChoice>,
    first_visit: bool,
    accepts_answers: bool,
    email_confirmed: bool,
    /// A one-off confirmation after a change, e.g. "Saved!".
    flash: Option<&'static str>,
    /// Confetti, right after the guest says they're coming.
    celebrate: bool,
    /// The full link, shown right after a group sign-up so the guest can keep it.
    new_link: Option<String>,
    error: Option<&'static str>,
}

impl<'a> RsvpPage<'a> {
    pub(super) fn new(state: &'a AppState, lang: Lang, guest: &'a Guest) -> Self {
        let t = lang.messages();
        let rsvp = &guest.rsvp;
        let chosen = rsvp.name_display.unwrap_or(NameDisplay::Full);
        Self {
            lang,
            t,
            event: EventView::new(&state.config.event, lang),
            links: GuestLinks::new(&guest.link, lang),
            rsvp,
            status: t.answer(rsvp.response, rsvp.party_size),
            party_sizes: PartySize::choices(t, rsvp.max_party_size, rsvp.party_size),
            display_choices: DisplayChoice::choices(t, Some(&rsvp.name), chosen),
            first_visit: rsvp.privacy_version.is_none(),
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
            email_confirmed: rsvp.email_status == rsvp::EmailStatus::Confirmed,
            flash: None,
            celebrate: false,
            new_link: None,
            error: None,
        }
    }

    /// The page again with an error message, e.g. for an invalid form.
    pub(super) fn render_error(
        mut self,
        status: StatusCode,
        pick: impl Fn(&'static Messages) -> &'static str,
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

    fn text(self, t: &'static Messages) -> &'static str {
        match self {
            Self::Saved => &t.saved,
            Self::EmailSent => &t.email_sent_notice,
            Self::EmailRemoved => &t.email_removed_notice,
            Self::EmailConfirmed => &t.email_confirmed_notice,
        }
    }
}

/// Back to the RSVP page after a change (post/redirect/get).
pub(super) fn redirect(link: &str, lang: Lang, notice: Notice) -> Response {
    redirect_to(paths::RSVP, link, lang, notice)
}

/// Back to one of the guest's pages after a change.
fn redirect_to(pattern: &str, link: &str, lang: Lang, notice: Notice) -> Response {
    let page = paths::localized(&paths::path(pattern, link), lang);
    Redirect::to(&format!("{page}&notice={}", notice.code())).into_response()
}

impl Notice {
    fn flash(notice: Option<&str>, t: &'static Messages) -> Option<&'static str> {
        notice
            .and_then(Self::from_code)
            .map(|notice| notice.text(t))
    }
}

#[derive(Deserialize)]
pub struct ShowQuery {
    notice: Option<String>,
    /// Set right after a group sign-up.
    new: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Query(query): Query<ShowQuery>,
) -> Response {
    let mut page = RsvpPage::new(&state, lang, &guest);
    let notice = query.notice.as_deref().and_then(Notice::from_code);
    page.flash = notice.map(|notice| notice.text(page.t));
    page.celebrate = guest.rsvp.response == rsvp::Response::Attending
        && (matches!(notice, Some(Notice::Saved)) || query.new.is_some());
    page.new_link = query
        .new
        .map(|_| state.config.url(&paths::path(paths::RSVP, &guest.link)));
    render(page)
}

/// The guest list, with how the guest shows up in it.
pub async fn guests(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Query(query): Query<ShowQuery>,
) -> Response {
    let guests = match GuestList::load(&state.db, Some(guest.rsvp.id)).await {
        Ok(guests) => guests,
        Err(err) => return server_error(err),
    };
    let t = lang.messages();
    let links = GuestLinks::new(&guest.link, lang);
    render(GuestsPage {
        lang,
        t,
        event: EventView::new(&state.config.event, lang),
        guests,
        you: Some(YourListing::new(t, &guest.rsvp, links.listing)),
        flash: Notice::flash(query.notice.as_deref(), t),
        back: links.page,
        back_label: &t.back_to_your_page,
    })
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
    name_display: Option<NameDisplay>,
}

pub async fn answer(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Form(form): Form<AnswerForm>,
) -> Response {
    let page = || RsvpPage::new(&state, lang, &guest);
    if !state.config.event.accepts_answers(Utc::now()) {
        return page().render_error(StatusCode::FORBIDDEN, |t| &t.answers_closed);
    }
    if !(1..=guest.rsvp.max_party_size).contains(&form.party_size) {
        return page().render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
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
        form.name_display,
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
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name);
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

#[derive(Deserialize)]
pub struct ListingForm {
    name_display: NameDisplay,
}

/// Changes how the guest's name shows in the guest list.
pub async fn listing(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    Form(form): Form<ListingForm>,
) -> Response {
    match rsvp::set_name_display(&state.db, guest.rsvp.id, form.name_display).await {
        Ok(true) => redirect_to(paths::RSVP_GUESTS, &guest.link, lang, Notice::Saved),
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
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.confirm_delete);
    }
    match rsvp::delete(&state.db, guest.rsvp.id).await {
        Ok(_) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp deleted by guest");
            render(MessagePage::new(lang, |t| {
                (&t.deleted_title, &t.deleted_text)
            }))
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::web::testing::{Site, body, en, escape, form_post, location};

    #[sqlx::test]
    async fn guest_answers_through_their_link(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari & Ola", 2).await;

        let response = site.get(&format!("/r/{link}")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
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
        assert_eq!((answered.party_size, answered.lang), (2, Lang::EnGb));
        assert_eq!(answered.privacy_version.as_deref(), Some(PRIVACY_VERSION));

        let html = body(site.get(&format!("/r/{link}?lang=en&notice=saved")).await).await;
        assert!(html.contains(&escape(&en().saved)));
        assert!(html.contains(&en().answer(rsvp::Response::Attending, 2)));
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

        let fresh = site.config.link_key.link(Purpose::Rsvp, invite.id, 1);
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
        let site = Site::with(db, &[("INNBY_RSVP_DEADLINE", "2020-01-01T12:00")]);
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
        assert_eq!(json["name_display"], serde_json::Value::Null);
    }

    #[sqlx::test]
    async fn guest_list_shows_names_as_each_guest_chose(db: PgPool) {
        let site = Site::new(db);
        let (_, kari) = site.invite("Kari Nordmann", 1).await;
        let (_, ola) = site.invite("Ola Hansen", 1).await;
        let (_, per) = site.invite("Per Berg", 1).await;
        site.invite("Anne Lie", 1).await;
        let guests = |link: &str| format!("/r/{link}/guests?lang=en");

        // Coming defaults to the full name.
        let html = body(site.get(&format!("/r/{kari}?lang=en")).await).await;
        assert!(html.contains(&format!(
            r#"<option value="full" selected>{} (Kari Nordmann)</option>"#,
            en().display_full
        )));
        // The page a link opens has no other guest's name: link previews fetch it.
        assert!(!html.contains("Ola") && !html.contains("Anne"), "{html}");
        assert!(html.contains(&format!(
            r#"<a class="button secondary" href="/r/{kari}/guests?lang=en" rel="nofollow">"#
        )));
        assert!(
            body(site.get(&guests(&kari)).await)
                .await
                .contains("<li>Ola H.</li>")
        );
        for (link, answer) in [
            (&kari, "attending&name_display=full"),
            (&ola, "attending&name_display=hidden"),
            (&per, "declined&name_display=full"),
        ] {
            let form = format!("party_size=1&response={answer}");
            site.post(&format!("/r/{link}"), &form).await;
        }

        let response = site.get(&guests(&kari)).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let html = body(response).await;
        let t = en();
        assert!(html.contains(&format!(
            r#"<li>Kari Nordmann <span class="muted">{}</span></li>"#,
            t.you
        )));
        assert!(html.contains("<li>Anne L.</li>"));
        // Ola is coming but hidden, and Per declined.
        assert!(html.contains(&format!(r#"<li class="muted">{}</li>"#, t.more_guests(1))));
        assert!(!html.contains("Ola H") && !html.contains("Per B"));
        assert!(html.contains(&format!(
            "{}</p>",
            crate::i18n::bold(&t.listed_as, "name", "Kari Nordmann")
        )));
        assert!(html.contains(&format!(
            r#"href="/r/{kari}?lang=en">{}"#,
            t.back_to_your_page
        )));

        let response = site
            .post(
                &format!("/r/{kari}/listing?lang=en"),
                "name_display=partial",
            )
            .await;
        let back = location(&response).to_owned();
        assert_eq!(back, format!("/r/{kari}/guests?lang=en&notice=saved"));
        assert!(
            body(site.get(&back).await)
                .await
                .contains(&escape(&t.saved))
        );
        let html = body(site.get(&guests(&ola)).await).await;
        assert!(html.contains("<li>Kari N.</li>"));
        assert!(html.contains(&escape(&t.listed_hidden)));
        // Answering again keeps the choice the form shows.
        let html = body(site.get(&format!("/r/{ola}?lang=en")).await).await;
        assert!(html.contains(r#"<option value="hidden" selected>"#));
        let html = body(site.get(&guests(&per)).await).await;
        assert!(html.contains(&escape(&t.listed_declined)));
        assert!(!html.contains(&t.show_instead));

        let response = site.get("/r/garbage/guests").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn chinese_pages_and_names(db: PgPool) {
        let site = Site::new(db);
        let (_, link) = site.invite("王小明", 1).await;
        let html = body(site.get(&format!("/r/{link}?lang=zh-hans")).await).await;
        assert!(html.contains(r#"<html lang="zh-Hans" "#));
        assert!(html.contains("王小明，你好！"));
        assert!(html.contains("你的回复：<strong>尚未回复</strong>"));
        // Before answering, other guests see only the family name.
        assert!(html.contains(r#"<option value="partial">部分姓名（王**）</option>"#));
        let html = body(site.get(&format!("/r/{link}/guests?lang=zh-hant")).await).await;
        assert!(html.contains("其他賓客看到的你：<strong>王**</strong>"));
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
        assert!(body(response).await.contains(&escape(&en().deleted_title)));
        assert!(site.find(&invite).await.is_none());
        assert_eq!(
            site.get(&format!("/r/{link}")).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
