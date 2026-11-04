use askama::Template;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use sqlx::PgPool;
use uuid::Uuid;

use super::{AppState, PRIVACY_VERSION, remember::Remembered};
use crate::{
    config::Controller,
    db::{
        event::Details,
        rsvp::{self, NameDisplay, Response as Answer, Rsvp},
    },
    http::{render, render_with, server_error},
    i18n::{Lang, Messages, fill},
    janitor::{KEEP_AFTER_EVENT, KEEP_SENT_MAIL},
    paths,
};

/// The event details shown at the top of the public pages.
pub struct EventView<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub location: &'a str,
    pub starts_at: String,
    pub starts_at_iso: String,
    pub rsvp_deadline: Option<String>,
}

impl<'a> EventView<'a> {
    pub fn new(event: &'a Details, lang: Lang) -> Self {
        Self {
            title: event.title.get(lang),
            description: event.description.get(lang),
            location: &event.location,
            starts_at: lang.format_date(event.starts_at),
            starts_at_iso: event.starts_at.to_rfc3339(),
            rsvp_deadline: event
                .rsvp_deadline
                .map(|deadline| lang.format_date(deadline)),
        }
    }
}

/// One choice in the "How many are coming?" list (`_party_size.html`).
pub struct PartySize {
    pub value: i32,
    pub label: String,
    pub selected: bool,
}

impl PartySize {
    /// The choices from 1 to `max`, with `selected` preselected.
    pub fn choices(t: &Messages, max: i32, selected: i32) -> Vec<Self> {
        (1..=max)
            .map(|value| Self {
                value,
                label: t.people_count(value),
                selected: value == selected,
            })
            .collect()
    }
}

/// One choice for how a name shows in the guest list (`_name_display.html`).
pub struct DisplayChoice {
    pub value: &'static str,
    pub label: String,
    pub selected: bool,
}

impl DisplayChoice {
    /// The choices, with `selected` preselected. Given the guest's name, each label shows
    /// what others would see, e.g. "First name and initial (Kari N.)".
    pub fn choices(t: &Messages, name: Option<&str>, selected: NameDisplay) -> Vec<Self> {
        NameDisplay::ALL
            .into_iter()
            .map(|display| {
                let label = t.name_display(display);
                Self {
                    value: display.code(),
                    label: match name.and_then(|name| display.apply(name)) {
                        Some(shown) => {
                            fill(&t.display_choice, &[("label", &label), ("name", &shown)])
                        }
                        None => label.to_owned(),
                    },
                    selected: display == selected,
                }
            })
            .collect()
    }
}

/// The guest list that every personal and group link shows (`_guest_list.html`), with
/// each name as its guest chose to show it.
#[derive(Default)]
pub struct GuestList {
    pub coming: Vec<ListedGuest>,
    pub pending: Vec<ListedGuest>,
    /// How many aren't listed: those coming who chose not to be, with everyone they
    /// bring, and the invitations not answered yet that chose not to be.
    pub coming_hidden: usize,
    pub pending_hidden: usize,
    /// Everyone coming, with those they bring, listed or not.
    pub coming_people: i32,
}

pub struct ListedGuest {
    pub name: String,
    /// How many a coming guest brings besides themselves, shown as "+1".
    pub plus: i32,
    /// Whether this is the guest looking at the list.
    pub you: bool,
}

impl GuestList {
    /// The event's list as the guest `you` sees it, or as someone who hasn't signed up
    /// yet.
    pub async fn load(db: &PgPool, event_id: Uuid, you: Option<Uuid>) -> sqlx::Result<Self> {
        let mut list = Self::default();
        for guest in rsvp::guest_list(db, event_id).await? {
            let (names, hidden, plus) = match guest.response {
                Answer::Attending => {
                    list.coming_people += guest.party_size;
                    (
                        &mut list.coming,
                        &mut list.coming_hidden,
                        guest.party_size - 1,
                    )
                }
                Answer::Pending | Answer::Declined => {
                    (&mut list.pending, &mut list.pending_hidden, 0)
                }
            };
            match NameDisplay::listed(guest.response, guest.name_display).apply(&guest.name) {
                Some(name) => names.push(ListedGuest {
                    name,
                    plus,
                    you: Some(guest.id) == you,
                }),
                None => *hidden += 1 + plus as usize,
            }
        }
        for names in [&mut list.coming, &mut list.pending] {
            names.sort_by_cached_key(|guest| guest.name.to_lowercase());
        }
        Ok(list)
    }

    pub fn has_coming(&self) -> bool {
        !self.coming.is_empty() || self.coming_hidden > 0
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || self.pending_hidden > 0
    }

    /// How many invitations have yet to be answered, listed or not.
    pub fn pending_count(&self) -> usize {
        self.pending.len() + self.pending_hidden
    }
}

/// The guest list on a page of its own. Link previews in chat apps and mail scanners
/// fetch only the link that was shared, so they never see a name; a guest gets here
/// with a button.
#[derive(Template)]
#[template(path = "guests.html")]
pub struct GuestsPage<'a> {
    pub lang: Lang,
    pub t: &'static Messages,
    pub event: EventView<'a>,
    pub guests: GuestList,
    /// For a guest: how they show up, and the other ways they could.
    pub you: Option<YourListing>,
    pub flash: Option<&'static str>,
    /// The page the list was opened from.
    pub back: String,
    pub back_label: &'static str,
}

pub struct YourListing {
    /// The guest's name as the list shows it, if it does.
    pub name: Option<String>,
    pub declined: bool,
    pub others: Vec<DisplayChoice>,
    /// Where the buttons for the others post.
    pub action: String,
}

impl YourListing {
    pub fn new(t: &Messages, rsvp: &Rsvp, action: String) -> Self {
        Self {
            name: rsvp.listed().apply(&rsvp.name),
            declined: rsvp.response == Answer::Declined,
            others: DisplayChoice::choices(t, None, rsvp.listed())
                .into_iter()
                .filter(|choice| !choice.selected)
                .collect(),
            action,
        }
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    lang: Lang,
    t: &'static Messages,
    /// The invitations this device remembers, the soonest event first.
    remembered: Vec<Welcome>,
}

/// An invitation this device remembers, greeted on the front page.
struct Welcome {
    title: String,
    when: String,
    place: String,
    name: String,
    /// Their answer, e.g. "Coming · 2 people".
    status: String,
    page: String,
    /// For forgetting it again.
    event_id: Uuid,
}

/// The front page: what Innby is, and the way back to each invitation this device
/// remembers.
pub async fn index(State(state): State<AppState>, lang: Lang, headers: HeaderMap) -> Response {
    let t = lang.messages();
    let remembered = match Remembered::load(&state, &headers).await {
        Ok(remembered) => remembered,
        Err(err) => return server_error(err),
    };
    let mut guests = remembered.guests;
    guests.sort_by_key(|guest| guest.event.details.starts_at);
    let mut response = render(IndexPage {
        lang,
        t,
        remembered: guests
            .iter()
            .map(|guest| {
                let event = &guest.event.details;
                Welcome {
                    title: event.title(lang).to_owned(),
                    when: lang.format_date(event.starts_at),
                    place: event.location.clone(),
                    name: guest.rsvp.name.clone(),
                    status: t.answer(guest.rsvp.response, guest.rsvp.party_size),
                    page: paths::localized(&paths::path(paths::RSVP, &guest.link), lang),
                    event_id: guest.event.id,
                }
            })
            .collect(),
    });
    // Links that stopped working are forgotten, and an older cookie gets its new name.
    for cookie in remembered.cookies {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

#[derive(Template)]
#[template(path = "privacy.html")]
struct PrivacyPage<'a> {
    lang: Lang,
    t: &'static Messages,
    version: &'static str,
    controller: &'a Controller,
    keep_after_event_days: i64,
    keep_sent_mail_days: i64,
}

/// The privacy notice (GDPR Art. 13 and 14). Its version is recorded with each answer.
pub async fn privacy(State(state): State<AppState>, lang: Lang) -> Response {
    render(PrivacyPage {
        lang,
        t: lang.messages(),
        version: PRIVACY_VERSION,
        controller: &state.config.controller,
        keep_after_event_days: KEEP_AFTER_EVENT.num_days(),
        keep_sent_mail_days: KEEP_SENT_MAIL.num_days(),
    })
}

/// A page with a heading, a paragraph and a way back to the event.
#[derive(Template)]
#[template(path = "message.html")]
pub struct MessagePage {
    pub lang: Lang,
    pub t: &'static Messages,
    pub title: &'static str,
    pub text: &'static str,
}

impl MessagePage {
    pub fn new(
        lang: Lang,
        pick: impl Fn(&'static Messages) -> (&'static str, &'static str),
    ) -> Self {
        let t = lang.messages();
        let (title, text) = pick(t);
        Self {
            lang,
            t,
            title,
            text,
        }
    }
}

/// The page every guest or group link that doesn't work gets: mistyped, replaced by a
/// new one, or its guest deleted.
pub fn gone(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (&t.gone_title, &t.gone_text)),
    )
}

/// The fallback for unknown paths.
pub async fn not_found(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (&t.not_found_title, &t.not_found_text)),
    )
}

/// Turns every crawler away: an invitation-only event has nothing for them. Link
/// previews in chat apps may not ask, which is why the guest list is on a page of its
/// own (`GuestsPage`).
pub async fn robots() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "User-agent: *\nDisallow: /\n",
    )
}
