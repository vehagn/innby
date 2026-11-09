//! The event as a calendar file (iCalendar, RFC 5545), for a guest to add to their own
//! calendar from their page: `/r/{link}/event.ics`. It holds the event alone, as everyone
//! invited sees it, and nothing about the guest: not their name, nor their link, which
//! would let whoever sees their calendar answer for them.
//!
//! A file, rather than a link to a calendar service, sends nothing to anyone else, and
//! needs no JavaScript: phones and computers open it in their calendar.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, TimeZone, Utc};

use super::{AppState, rsvp::Guest};
use crate::{db::event::Event, i18n::Lang};

/// How long an event with no set end lasts in a calendar.
pub const UNSET_LENGTH: Duration = Duration::hours(3);

/// The guest's event, in their language.
pub async fn show(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let file = ics(&guest.event, lang, host(&state.config.base_url), Utc::now());
    (
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            // Inline, so a phone offers to add it to the calendar rather than save it.
            (
                header::CONTENT_DISPOSITION,
                r#"inline; filename="event.ics""#,
            ),
        ],
        file,
    )
        .into_response()
}

/// The site's host, which makes the event's id unique among everyone's calendars.
fn host(base_url: &str) -> &str {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// The calendar file: one event, its times in UTC, which every calendar shows in its own
/// zone.
fn ics(event: &Event, lang: Lang, host: &str, now: DateTime<Utc>) -> String {
    let details = &event.details;
    let starts = details.starts_at.to_utc();
    let ends = details
        .ends_at
        .map_or(starts + UNSET_LENGTH, |ends| ends.to_utc());
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_owned(),
        "VERSION:2.0".to_owned(),
        "PRODID:-//Innby//Innby//EN".to_owned(),
        "CALSCALE:GREGORIAN".to_owned(),
        "METHOD:PUBLISH".to_owned(),
        "BEGIN:VEVENT".to_owned(),
        // The same for every guest and every download, so adding it again updates it
        // in calendars that look.
        format!("UID:{}@{host}", event.id),
        format!("DTSTAMP:{}", utc(now)),
        format!("DTSTART:{}", utc(starts)),
        format!("DTEND:{}", utc(ends)),
        format!("SUMMARY:{}", text(details.title(lang))),
    ];
    if !details.location.is_empty() {
        lines.push(format!("LOCATION:{}", text(&details.location)));
    }
    let description = details.description.get(lang);
    if !description.is_empty() {
        lines.push(format!("DESCRIPTION:{}", text(description)));
    }
    lines.extend(["END:VEVENT".to_owned(), "END:VCALENDAR".to_owned()]);
    lines.iter().map(|line| fold(line)).collect()
}

/// A moment in UTC, as `20261212T180000Z`.
fn utc<Z: TimeZone>(at: DateTime<Z>) -> String {
    at.to_utc().format("%Y%m%dT%H%M%SZ").to_string()
}

/// A text value, with the characters that mean something in the format escaped, and
/// line breaks as `\n`.
fn text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// A line as the format wants it: no longer than 75 bytes, the rest on lines that start
/// with a space, never splitting a character, each ending in CRLF.
fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests;
