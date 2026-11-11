//! A guest's personal pages under `/r/{link}`, their tabs: the reply (the event, the
//! answer and the calendar) and the guest list here, the games in `games`, and their
//! details in `you`. `Guest` finds the guest behind a link for all of them.

use askama::Template;
use axum::{
    Form,
    extract::{FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION, games,
    pages::{Page, PageError},
    remember,
    views::{
        DisplayChoice, EventView, FormPage, GuestList, GuestTab, GuestsPage, PartySize, Question,
        YourListing, guest_tabs,
    },
};
use crate::{
    db::{
        board::BoardState,
        event::Event,
        rsvp::{self, NameDisplay, Rsvp},
    },
    http::{cookie, render},
    i18n::{Asked, Lang, Messages, Offer},
    icons::TabLink,
    link::{IdLink, Purpose},
    paths::{self, GuestLinks},
};

/// The guest behind the `{link}` in the path. Malformed, forged, revoked and deleted
/// links are all turned away with the same page.
pub(super) struct Guest {
    pub link: String,
    /// The language to answer them in. On their pages it's what the request asks for
    /// of the languages their event is offered in, which may be fewer than the site's,
    /// so handlers go by this rather than asking for a `Lang` of their own. For a guest
    /// found another way, it's the one they last used.
    pub lang: Lang,
    pub rsvp: Rsvp,
    /// The event the guest is invited to.
    pub event: Event,
    /// The games the event offers, by id, which the config may no longer have.
    pub games: Vec<String>,
    /// What the event's message board is.
    pub board: BoardState,
}

#[derive(Deserialize)]
struct LinkPath {
    link: String,
}

impl FromRequestParts<AppState> for Guest {
    type Rejection = PageError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, PageError> {
        let asked = Asked::from_parts(parts);
        // A link that doesn't work has no event: its page is in a language of the site's.
        let gone = || PageError::Gone(asked.among(Offer::current()));
        // By name: a path may hold more than the link, as a post's id.
        let Ok(Path(params)) = Path::<LinkPath>::from_request_parts(parts, state).await else {
            return Err(gone());
        };
        let link = params.link;
        let mut guest = Self::find(state, &link).await?.ok_or_else(gone)?;
        // From here on the request is offered the event's languages, the menu too.
        guest.lang = asked.among(guest.event.details.offer());
        Ok(guest)
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
            .and_then(|found| Self::checked(state, link, &parsed, found)))
    }

    /// The guest, if the link's signature is the one their RSVP has now: not forged, and
    /// not replaced by a new one.
    pub(super) fn checked(
        state: &AppState,
        link: &str,
        parsed: &IdLink,
        found: rsvp::WithEvent,
    ) -> Option<Self> {
        let rsvp::WithEvent {
            rsvp,
            event,
            games,
            board,
        } = found;
        let genuine = state
            .config
            .link_key
            .verify(Purpose::Rsvp, parsed, rsvp.link_nonce);
        genuine.then(|| Self {
            link: link.to_owned(),
            lang: rsvp.lang,
            rsvp,
            event,
            games,
            board,
        })
    }

    /// The guest's tabs, with Games and Board only when the event has them for guests.
    pub(super) fn tabs(
        &self,
        state: &AppState,
        links: &GuestLinks,
        current: GuestTab,
    ) -> Vec<TabLink> {
        let games = games::offers_any(state, self);
        guest_tabs(
            self.lang.messages(),
            links,
            games,
            self.board.for_guests(),
            current,
        )
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
    /// Whether they've said they aren't: the answer's button shows which is theirs.
    declined: bool,
    party_sizes: Vec<PartySize>,
    /// The host's question, with the guest's pick, if the event asks one.
    question: Option<Question>,
    /// For the answer form: the guest's choice, or their full name if they haven't made
    /// one, since that's the default once they're coming.
    display_choices: Vec<DisplayChoice>,
    first_visit: bool,
    /// Whether the event's time or place changed after they answered.
    changed: bool,
    accepts_answers: bool,
    /// A one-off confirmation after a change, e.g. "Saved!".
    flash: Option<&'static str>,
    /// Confetti, right after the guest says they're coming.
    celebrate: bool,
    /// The full link, shown right after a group sign-up so the guest can keep it.
    new_link: Option<String>,
    /// Whether this device remembers the guest, for the front page.
    remembered: bool,
    /// The guest's tabs, set by the handler, which knows whether the event offers games.
    tabs: Vec<TabLink>,
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
            declined: rsvp.response == rsvp::Response::Declined,
            party_sizes: PartySize::choices(t, rsvp.max_party_size, rsvp.party_size),
            question: Question::new(&guest.event.details, lang, rsvp.choice),
            display_choices: DisplayChoice::choices(t, Some(&rsvp.name), chosen),
            first_visit: rsvp.privacy_version.is_none(),
            changed: matches!(
                (guest.event.changed_at, rsvp.responded_at),
                (Some(changed), Some(answered)) if changed > answered
            ),
            accepts_answers: guest.event.details.accepts_answers(Utc::now()),
            flash: None,
            celebrate: false,
            new_link: None,
            remembered: false,
            tabs: Vec::new(),
            error: None,
        }
    }
}

impl FormPage for RsvpPage<'_> {
    fn error_slot(&mut self) -> (&'static Messages, &mut Option<&'static str>) {
        (self.t, &mut self.error)
    }
}

/// What a guest's page confirms after a change, passed as `?notice=`.
#[derive(Clone, Copy)]
pub(super) enum Notice {
    Saved,
    EmailSent,
    EmailRemoved,
    EmailConfirmed,
    Remembered,
    Forgotten,
    Posted,
    PostDeleted,
}

impl Notice {
    const ALL: [Self; 8] = [
        Self::Saved,
        Self::EmailSent,
        Self::EmailRemoved,
        Self::EmailConfirmed,
        Self::Remembered,
        Self::Forgotten,
        Self::Posted,
        Self::PostDeleted,
    ];

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::EmailSent => "email_sent",
            Self::EmailRemoved => "email_removed",
            Self::EmailConfirmed => "email_confirmed",
            Self::Remembered => "remembered",
            Self::Forgotten => "forgotten",
            Self::Posted => "posted",
            Self::PostDeleted => "post_deleted",
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
            Self::Posted => &t.board_posted,
            Self::PostDeleted => &t.board_deleted_notice,
        }
    }

    /// What `?notice=` says to confirm, if it names a notice.
    pub(super) fn flash(notice: Option<&str>, t: &'static Messages) -> Option<&'static str> {
        notice
            .and_then(Self::from_code)
            .map(|notice| notice.text(t))
    }

    /// Back to one of the guest's pages after a change, to confirm it there
    /// (post/redirect/get): `pattern` is the page's, as `paths::RSVP_YOU`.
    pub(super) fn redirect(self, pattern: &str, link: &str, lang: Lang) -> Response {
        let page = paths::localized(&paths::path(pattern, link), lang);
        Redirect::to(&format!("{page}&notice={}", self.code())).into_response()
    }
}

#[derive(Deserialize)]
pub struct ShowQuery {
    pub(super) notice: Option<String>,
    /// Set right after a group sign-up.
    new: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    guest: Guest,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Response {
    let lang = guest.lang;
    let mut page = RsvpPage::new(lang, &guest);
    page.remembered = remember::remembers(&headers, state.config.is_https(), &guest);
    page.tabs = guest.tabs(&state, &page.links, GuestTab::Reply);
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
    guest: Guest,
    Query(query): Query<ShowQuery>,
) -> Page {
    let lang = guest.lang;
    let guests = GuestList::load(&state.db, guest.event.id, Some(guest.rsvp.id)).await?;
    let t = lang.messages();
    let links = GuestLinks::new(&guest.link, lang);
    Ok(render(GuestsPage {
        lang,
        t,
        event: EventView::new(&guest.event.details, lang),
        guests,
        you: Some(YourListing::new(t, &guest.rsvp, links.listing.clone())),
        flash: Notice::flash(query.notice.as_deref(), t),
        tabs: guest.tabs(&state, &links, GuestTab::Guests),
        back: None,
    }))
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
    /// Their pick for the host's question: empty for none.
    choice: Option<String>,
}

pub async fn answer(
    State(state): State<AppState>,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<AnswerForm>,
) -> Page {
    let lang = guest.lang;
    let https = state.config.is_https();
    // Shown again to fix, with the box as this device has it.
    let refuse = |status, pick: fn(&'static Messages) -> &'static str| -> Page {
        let mut page = RsvpPage::new(lang, &guest);
        page.remembered = remember::remembers(&headers, https, &guest);
        Ok(page.render_error(status, pick))
    };
    if !guest.event.details.accepts_answers(Utc::now()) {
        return refuse(StatusCode::FORBIDDEN, |t| &t.answers_closed);
    }
    if !(1..=guest.rsvp.max_party_size).contains(&form.party_size) {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
    }
    let Ok(choice) = Question::parse(&guest.event.details, form.choice.as_deref()) else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_choice);
    };

    let response = match form.response {
        Answer::Attending => rsvp::Response::Attending,
        Answer::Declined => rsvp::Response::Declined,
    };
    let id = guest.rsvp.id;
    let reply = rsvp::Reply {
        response,
        party_size: form.party_size,
        name_display: form.name_display,
        choice,
    };
    // None if they were deleted in the meantime.
    rsvp::respond(&state.db, id, &reply, lang, PRIVACY_VERSION)
        .await?
        .ok_or(PageError::Gone(lang))?;
    tracing::info!(rsvp = %id, ?response, "rsvp answered");
    let saved = Notice::Saved.redirect(paths::RSVP, &guest.link, lang);
    Ok(
        match remember::as_answered(https, &headers, &guest, form.remember.is_some()) {
            Some(cookie) => cookie::with(saved, cookie),
            None => saved,
        },
    )
}

#[derive(Deserialize)]
pub struct ListingForm {
    name_display: NameDisplay,
}

/// Changes how the guest's name shows in the guest list.
pub async fn listing(
    State(state): State<AppState>,
    guest: Guest,
    Form(form): Form<ListingForm>,
) -> Page {
    let lang = guest.lang;
    if !rsvp::set_name_display(&state.db, guest.rsvp.id, form.name_display).await? {
        return Err(PageError::Gone(lang));
    }
    Ok(Notice::Saved.redirect(paths::RSVP_GUESTS, &guest.link, lang))
}

#[cfg(test)]
mod tests;
