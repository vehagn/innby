//! The guest's personal page at `/r/{link}`: answer, change, export and delete.

use askama::Template;
use axum::{
    Form, Json,
    extract::{FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    games::{self, GameView},
    pages::{MessagePage, gone},
    remember,
    views::{DisplayChoice, EventView, GuestList, GuestsPage, PartySize, YourListing},
};
use crate::{
    db::{
        event::Event,
        rsvp::{self, NameDisplay, Rsvp},
    },
    http::{cookie, render, render_with, server_error},
    i18n::{Lang, Messages},
    link::{IdLink, Purpose},
    paths::{self, GuestLinks},
};

/// The guest behind the `{link}` in the path. Malformed, forged, revoked and deleted
/// links are all turned away with the same page.
pub(super) struct Guest {
    pub link: String,
    pub rsvp: Rsvp,
    /// The event the guest is invited to.
    pub event: Event,
}

impl FromRequestParts<AppState> for Guest {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let Ok(lang) = Lang::from_request_parts(parts, state).await;
        let Path(link) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| gone(lang))?;
        match Self::find(state, &link).await {
            Ok(Some(guest)) => Ok(guest),
            Ok(None) => Err(gone(lang)),
            Err(err) => Err(server_error(err)),
        }
    }
}

impl Guest {
    /// The guest a link leads to, if it's well-formed, genuine and current.
    pub(super) async fn find(state: &AppState, link: &str) -> sqlx::Result<Option<Self>> {
        let Ok(parsed) = link.parse::<IdLink>() else {
            return Ok(None);
        };
        let found = rsvp::find_with_events(&state.db, &[parsed.id]).await?;
        Ok(found
            .into_iter()
            .next()
            .and_then(|(rsvp, event)| Self::checked(state, link, &parsed, rsvp, event)))
    }

    /// The guest, if the link's signature is the one their RSVP has now: not forged, and
    /// not replaced by a new one.
    pub(super) fn checked(
        state: &AppState,
        link: &str,
        parsed: &IdLink,
        rsvp: Rsvp,
        event: Event,
    ) -> Option<Self> {
        let genuine = state
            .config
            .link_key
            .verify(Purpose::Rsvp, parsed, rsvp.link_nonce);
        genuine.then(|| Self {
            link: link.to_owned(),
            rsvp,
            event,
        })
    }
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
    /// Whether they're coming, and so are offered the event for their calendar.
    attending: bool,
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
    /// Whether this device remembers the guest, for the front page.
    remembered: bool,
    /// The minigames the event offers, with their best scores.
    games: Vec<GameView>,
    error: Option<&'static str>,
}

impl<'a> RsvpPage<'a> {
    pub(super) fn new(lang: Lang, guest: &'a Guest) -> Self {
        let t = lang.messages();
        let rsvp = &guest.rsvp;
        let chosen = rsvp.name_display.unwrap_or(NameDisplay::Full);
        Self {
            lang,
            t,
            event: EventView::new(&guest.event.details, lang),
            links: GuestLinks::new(&guest.link, lang),
            rsvp,
            status: t.answer(rsvp.response, rsvp.party_size),
            attending: rsvp.response == rsvp::Response::Attending,
            party_sizes: PartySize::choices(t, rsvp.max_party_size, rsvp.party_size),
            display_choices: DisplayChoice::choices(t, Some(&rsvp.name), chosen),
            first_visit: rsvp.privacy_version.is_none(),
            accepts_answers: guest.event.details.accepts_answers(Utc::now()),
            email_confirmed: rsvp.email_status == rsvp::EmailStatus::Confirmed,
            flash: None,
            celebrate: false,
            new_link: None,
            remembered: false,
            games: Vec::new(),
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
    Remembered,
    Forgotten,
}

impl Notice {
    const ALL: [Self; 6] = [
        Self::Saved,
        Self::EmailSent,
        Self::EmailRemoved,
        Self::EmailConfirmed,
        Self::Remembered,
        Self::Forgotten,
    ];

    fn code(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::EmailSent => "email_sent",
            Self::EmailRemoved => "email_removed",
            Self::EmailConfirmed => "email_confirmed",
            Self::Remembered => "remembered",
            Self::Forgotten => "forgotten",
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
            Self::Remembered => &t.remembered_notice,
            Self::Forgotten => &t.forgotten_notice,
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
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Response {
    let mut page = RsvpPage::new(lang, &guest);
    page.remembered = remember::remembers(&headers, state.config.is_https(), &guest);
    page.games = match games::for_page(&state, lang, &guest.event, &guest.rsvp).await {
        Ok(games) => games,
        Err(err) => return server_error(err),
    };
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
    let guests = match GuestList::load(&state.db, guest.event.id, Some(guest.rsvp.id)).await {
        Ok(guests) => guests,
        Err(err) => return server_error(err),
    };
    let t = lang.messages();
    let links = GuestLinks::new(&guest.link, lang);
    render(GuestsPage {
        lang,
        t,
        event: EventView::new(&guest.event.details, lang),
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
    /// Ticked to have this device remember the guest; see `remember`.
    remember: Option<String>,
}

pub async fn answer(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<AnswerForm>,
) -> Response {
    let https = state.config.is_https();
    // Shown again to fix, with the box as this device has it.
    let page = || {
        let mut page = RsvpPage::new(lang, &guest);
        page.remembered = remember::remembers(&headers, https, &guest);
        page
    };
    if !guest.event.details.accepts_answers(Utc::now()) {
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
            let saved = redirect(&guest.link, lang, Notice::Saved);
            match remember::as_answered(https, &headers, &guest, form.remember.is_some()) {
                Some(cookie) => cookie::with(saved, cookie),
                None => saved,
            }
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
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return RsvpPage::new(lang, &guest)
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
/// Everything kept about the guest (GDPR Art. 15 and 20): their reply, and their best
/// score in each game.
#[derive(serde::Serialize)]
struct Export<'a> {
    #[serde(flatten)]
    rsvp: &'a Rsvp,
    scores: Vec<crate::db::games::Best>,
}

pub async fn export(State(state): State<AppState>, guest: Guest) -> Response {
    let scores = match crate::db::games::of_guest(&state.db, guest.rsvp.id).await {
        Ok(scores) => scores,
        Err(err) => return server_error(err),
    };
    let export = Export {
        rsvp: &guest.rsvp,
        scores,
    };
    (
        [(
            header::CONTENT_DISPOSITION,
            r#"attachment; filename="rsvp.json""#,
        )],
        Json(export),
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
    headers: HeaderMap,
    Form(form): Form<DeleteForm>,
) -> Response {
    if form.confirm.is_none() {
        return RsvpPage::new(lang, &guest)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.confirm_delete);
    }
    match rsvp::delete(&state.db, guest.rsvp.id).await {
        Ok(_) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp deleted by guest");
            let mut response = render(MessagePage::new(lang, |t| {
                (&t.deleted_title, &t.deleted_text)
            }));
            // A device that remembers the guest forgets them too.
            let https = state.config.is_https();
            if remember::remembers(&headers, https, &guest) {
                let cookie = remember::forget_cookie(https, guest.event.id);
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests;
