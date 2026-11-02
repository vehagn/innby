use std::fmt::Display;

use askama::Template;
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

use super::{AppState, PRIVACY_VERSION};
use crate::{
    config::{Controller, Event},
    i18n::{Lang, Messages},
    janitor::{KEEP_AFTER_EVENT, KEEP_SENT_MAIL},
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
    pub fn new(event: &'a Event, lang: Lang) -> Self {
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

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
}

pub async fn index(State(state): State<AppState>, lang: Lang) -> Response {
    render(IndexPage {
        lang,
        t: lang.messages(),
        event: EventView::new(&state.config.event, lang),
    })
}

#[derive(Template)]
#[template(path = "privacy.html")]
struct PrivacyPage<'a> {
    lang: Lang,
    t: &'static Messages,
    nb: bool,
    version: &'static str,
    event_title: &'a str,
    controller: &'a Controller,
    keep_after_event_days: i64,
    keep_sent_mail_days: i64,
}

/// The privacy notice (GDPR Art. 13 and 14). Its version is recorded with each answer.
pub async fn privacy(State(state): State<AppState>, lang: Lang) -> Response {
    render(PrivacyPage {
        lang,
        t: lang.messages(),
        nb: lang == Lang::Nb,
        version: PRIVACY_VERSION,
        event_title: state.config.event.title.get(lang),
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

/// The fallback for unknown paths, and for the open RSVP while it's closed.
pub async fn not_found(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (t.not_found_title, t.not_found_text)),
    )
}

pub async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../static/style.css"),
    )
}

pub fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => server_error(err),
    }
}

pub fn render_with(status: StatusCode, template: impl Template) -> Response {
    let mut response = render(template);
    if response.status().is_success() {
        *response.status_mut() = status;
    }
    response
}

/// Logs the cause and answers 500 without details.
pub fn server_error(err: impl Display) -> Response {
    tracing::error!(error = %err, "request failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}
