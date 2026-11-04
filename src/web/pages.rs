//! The pages that need no link: the front page, the privacy notice, and the message
//! pages for a link that's gone or a page that isn't there.

use askama::Template;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{AppState, PRIVACY_VERSION, remember::Remembered};
use crate::{
    config::Controller,
    http::{render, render_with, server_error},
    i18n::{Lang, Messages},
    janitor::{KEEP_AFTER_EVENT, KEEP_SENT_MAIL},
    paths,
};

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
