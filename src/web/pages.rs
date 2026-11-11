//! The pages that need no link: the front page, the privacy notice, and the message
//! pages for a link that's gone or a page that isn't there; and `PageError`, what a
//! handler of the site ends in when it has no page to give.

use askama::Template;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use uuid::Uuid;

use super::{AppState, PRIVACY_VERSION, remember::Remembered, rsvp::Guest};
use crate::{
    config::Controller,
    db::event::Details,
    http::{render, render_compressed, render_with, server_error},
    i18n::{Lang, Messages, escape_html, fill, filled},
    janitor::{KEEP_AFTER_EVENT, KEEP_SENT_MAIL},
    paths,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    lang: Lang,
    t: &'static Messages,
    /// The invitations this device remembers to events still to come, or under way,
    /// the soonest first.
    coming: Vec<Welcome>,
    /// Those to events that are over, the latest first: their pages stay until the
    /// event's data is deleted, but they matter less.
    past: Vec<Welcome>,
}

/// An invitation this device remembers, greeted on the front page.
struct Welcome {
    title: String,
    when: String,
    name: String,
    /// Their answer, e.g. "Coming · 2 people".
    status: String,
    page: String,
    /// For forgetting it again.
    invitation: Uuid,
    /// Whether the device remembers another invitation to the same event, which a guest
    /// should know of, in case one isn't theirs.
    shared: bool,
}

/// Why a request ended without its page, as the admin's `PageError` is there. Handlers
/// return it with `?`.
pub(super) enum PageError {
    /// The link doesn't work: mistyped, replaced by a new one, or its guest deleted.
    /// Every such link gets the same page, in the request's language.
    Gone(Lang),
    Database(sqlx::Error),
}

/// What a handler of the site answers with.
pub(super) type Page = Result<Response, PageError>;

impl From<sqlx::Error> for PageError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        match self {
            Self::Gone(lang) => gone(lang),
            Self::Database(err) => server_error(err),
        }
    }
}

/// The front page: what Innby is, and the way back to each invitation this device
/// remembers.
pub async fn index(State(state): State<AppState>, lang: Lang, headers: HeaderMap) -> Page {
    let t = lang.messages();
    let remembered = Remembered::load(&state, &headers).await?;
    let mut guests = remembered.guests;
    guests.sort_by_key(|guest| guest.event.details.starts_at);
    // An event without a set end is over when a calendar would show it ending.
    let now = Utc::now();
    let over = |details: &Details| {
        details
            .ends_at
            .unwrap_or(details.starts_at + crate::calendar::UNSET_LENGTH)
            < now
    };
    let (past, coming): (Vec<_>, Vec<_>) =
        guests.iter().partition(|guest| over(&guest.event.details));
    let welcome = |guest: &&Guest| {
        let event = &guest.event.details;
        Welcome {
            title: event.title(lang).to_owned(),
            when: lang.format_date(event.starts_at),
            name: guest.rsvp.name.clone(),
            status: t.answer(guest.rsvp.response, guest.rsvp.party_size),
            page: paths::localized(&paths::path(paths::RSVP, &guest.link), lang),
            invitation: guest.rsvp.id,
            shared: guests
                .iter()
                .filter(|other| other.event.id == guest.event.id)
                .nth(1)
                .is_some(),
        }
    };
    let mut response = render(IndexPage {
        lang,
        t,
        coming: coming.iter().map(welcome).collect(),
        past: past.iter().rev().map(welcome).collect(),
    });
    // Links that stopped working are forgotten.
    for cookie in remembered.cookies {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

#[derive(Template)]
#[template(path = "privacy.html")]
struct PrivacyPage {
    lang: Lang,
    t: &'static Messages,
    /// The language's notice, filled in: markup, from a file in `templates/privacy/`.
    notice: String,
}

/// The privacy notice (GDPR Art. 13 and 14). Its version is recorded with each answer.
/// The notice holds nothing secret or typed by a visitor, so it may be gzipped.
pub async fn privacy(State(state): State<AppState>, lang: Lang, headers: HeaderMap) -> Response {
    let notice = notice(lang, &state.config.controller);
    let t = lang.messages();
    render_compressed(&headers, PrivacyPage { lang, t, notice })
}

/// A language's privacy notice, its own or its fallback's (`Lang::notice`), with what
/// the config says filled in: who's responsible, who handles guests' data for them, the
/// authority guests may complain to, the notice's version and how long things are kept.
/// Those values come from the config, so each is escaped as it goes into the markup.
fn notice(lang: Lang, controller: &Controller) -> String {
    let (after_event, sent_mail) = (KEEP_AFTER_EVENT.num_days(), KEEP_SENT_MAIL.num_days());
    let t = lang.messages();
    // A sentence of the language's, with a setting in it, or without one when it's unset.
    let either = |setting: &Option<String>, with: &str, key: &str, without: &str| match setting {
        Some(value) => escape_html(&filled(with, key, value)),
        None => escape_html(without),
    };
    let mail_processor = either(
        &controller.mail_processor,
        &t.privacy_mail_processor,
        "processor",
        &t.privacy_mail_own,
    );
    let hosting = either(
        &controller.hosting,
        &t.privacy_hosting,
        "provider",
        &t.privacy_hosting_own,
    );
    // The authority is a link: its sentence is escaped around it.
    let authority = match &controller.authority {
        Some(authority) => {
            let (before, after) = t
                .privacy_authority
                .split_once("{authority}")
                .unwrap_or((&t.privacy_authority, ""));
            format!(
                r#"{}<a href="{}">{}</a>{}"#,
                escape_html(before),
                escape_html(&authority.url),
                escape_html(&authority.name),
                escape_html(after)
            )
        }
        None => escape_html(&t.privacy_authority_any),
    };
    let values = [
        ("controller_name", controller.name.clone()),
        ("controller_email", controller.email.clone()),
        ("version", PRIVACY_VERSION.to_owned()),
        ("keep_after_event_days", after_event.to_string()),
        ("keep_sent_mail_days", sent_mail.to_string()),
    ]
    .map(|(key, value)| (key, escape_html(&value)));
    let sentences = [
        ("mail_processor", mail_processor),
        ("hosting", hosting),
        ("authority", authority),
    ];
    let values: Vec<_> = values.into_iter().chain(sentences).collect();
    let values: Vec<(&str, &dyn std::fmt::Display)> = values
        .iter()
        .map(|(key, value)| (*key, value as &dyn std::fmt::Display))
        .collect();
    fill(lang.notice(), &values)
}

/// A page with a heading, a paragraph and a way back to the event.
#[derive(Template)]
#[template(path = "message.html")]
pub struct MessagePage {
    pub lang: Lang,
    pub t: &'static Messages,
    pub title: &'static str,
    pub text: &'static str,
    /// Whether to offer the way to a lost link, on the page of a link that doesn't work.
    pub lost: bool,
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
            lost: false,
        }
    }
}

/// The page every guest or group link that doesn't work gets; see `PageError::Gone`.
fn gone(lang: Lang) -> Response {
    // A guest who added their address can have the link they do have sent again.
    let page = MessagePage {
        lost: true,
        ..MessagePage::new(lang, |t| (&t.gone_title, &t.gone_text))
    };
    render_with(StatusCode::NOT_FOUND, page)
}

/// The fallback for unknown paths. A mistyped path under a guest's link still has the
/// link in its address, so the answer isn't to be kept, like the link's own pages.
pub async fn not_found(lang: Lang) -> Response {
    let page = render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (&t.not_found_title, &t.not_found_text)),
    );
    ([(header::CACHE_CONTROL, "no-store")], page).into_response()
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
