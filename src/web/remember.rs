//! "Remember me on this device": a guest can ask the browser to keep their link in a
//! cookie, so the front page leads them back to their page. There's one cookie per event,
//! set only when the guest asks for it, so a device can remember invitations to several
//! events. They ask with a box on their answer or their group sign-up, unticked until
//! they tick it (a box ticked for them wouldn't be asking), or once answers close, with a
//! button. A cookie authorises nothing: every change still needs the link in the address,
//! so it can at most show the way.

use axum::{
    Form,
    extract::State,
    http::{HeaderMap, HeaderValue},
    response::{IntoResponse, Redirect, Response},
};
use std::collections::HashMap;

use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AppState,
    rsvp::{Guest, Notice, redirect},
};
use crate::{
    db::{
        event::Event,
        rsvp::{self, Rsvp},
    },
    http::cookie,
    i18n::Lang,
    link::IdLink,
};

/// The most remembered links the front page reads. A guest is rarely invited to more
/// than a few events at once; a request with more is someone making the site check
/// links for nothing. They're looked up together, in one query.
const MOST_REMEMBERED: usize = 10;

/// The start of every cookie's name. On https, the `__Host-` prefix makes browsers
/// refuse one that another host or path tries to set.
fn prefix(https: bool) -> &'static str {
    if https { "__Host-innby-" } else { "innby-" }
}

/// The cookie for one event.
fn name(https: bool, event: Uuid) -> String {
    format!("{}{}", prefix(https), event.simple())
}

/// The links this browser remembers, with the name of the cookie each is in.
fn links(headers: &HeaderMap, https: bool) -> Vec<(&str, &str)> {
    let prefix = prefix(https);
    cookie::all(headers)
        .filter(|(name, link)| name.starts_with(prefix) && !link.is_empty())
        .collect()
}

/// Whether this browser remembers this guest's link.
pub(super) fn remembers(headers: &HeaderMap, https: bool, guest: &Guest) -> bool {
    let name = name(https, guest.event.id);
    links(headers, https)
        .into_iter()
        .any(|(cookie, link)| cookie == name && link == guest.link)
}

/// The cookie that remembers a guest's link until their data is deleted, after the event.
pub(super) fn remember_cookie(https: bool, event: &Event, link: &str) -> HeaderValue {
    let max_age = (event.deleted_at() - Utc::now()).num_seconds().max(0);
    cookie::set(&name(https, event.id), link, "/", max_age, https)
}

/// Tells the browser to drop the cookie for this event.
pub(super) fn forget_cookie(https: bool, event: Uuid) -> HeaderValue {
    cookie::set(&name(https, event), "", "/", 0, https)
}

/// The cookie an answer sets, as its box says: remembering the guest when it's ticked,
/// and forgetting them when it isn't on a device that remembers them.
pub(super) fn as_answered(
    https: bool,
    headers: &HeaderMap,
    guest: &Guest,
    ticked: bool,
) -> Option<HeaderValue> {
    if ticked {
        tracing::info!(rsvp = %guest.rsvp.id, "device remembered");
        Some(remember_cookie(https, &guest.event, &guest.link))
    } else {
        remembers(headers, https, guest).then(|| forget_cookie(https, guest.event.id))
    }
}

/// Remembers the guest's link on this device until their data is deleted, once answers
/// have closed and there's no answer to tick the box on.
pub async fn remember(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let cookie = remember_cookie(state.config.is_https(), &guest.event, &guest.link);
    tracing::info!(rsvp = %guest.rsvp.id, "device remembered");
    cookie::with(redirect(&guest.link, lang, Notice::Remembered), cookie)
}

/// Forgets the guest on this device, from their page.
pub async fn forget_here(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let cookie = forget_cookie(state.config.is_https(), guest.event.id);
    cookie::with(redirect(&guest.link, lang, Notice::Forgotten), cookie)
}

#[derive(Deserialize)]
pub struct ForgetForm {
    event: Uuid,
}

/// Forgets one invitation this device remembers, from the front page.
pub async fn forget(
    State(state): State<AppState>,
    lang: Lang,
    Form(form): Form<ForgetForm>,
) -> Response {
    let cookie = forget_cookie(state.config.is_https(), form.event);
    cookie::with(Redirect::to(&lang.home()).into_response(), cookie)
}

/// The invitations the front page leads back to.
pub(super) struct Remembered {
    /// One per event.
    pub guests: Vec<Guest>,
    /// Cookies to drop: links that no longer work, revoked or their reply deleted.
    pub cookies: Vec<HeaderValue>,
}

impl Remembered {
    pub(super) async fn load(state: &AppState, headers: &HeaderMap) -> sqlx::Result<Self> {
        let https = state.config.is_https();
        let mut remembered = Self {
            guests: Vec::new(),
            cookies: Vec::new(),
        };
        let read: Vec<(&str, &str, Option<IdLink>)> = links(headers, https)
            .into_iter()
            .take(MOST_REMEMBERED)
            .map(|(cookie_name, link)| (cookie_name, link, link.parse().ok()))
            .collect();
        // One query for them all, whatever the cookies say.
        let ids: Vec<Uuid> = read
            .iter()
            .filter_map(|(_, _, parsed)| Some(parsed.as_ref()?.id))
            .collect();
        let mut found: HashMap<Uuid, (Rsvp, Event)> = if ids.is_empty() {
            HashMap::new()
        } else {
            rsvp::find_with_events(&state.db, &ids)
                .await?
                .into_iter()
                .map(|(rsvp, event)| (rsvp.id, (rsvp, event)))
                .collect()
        };
        for (cookie_name, link, parsed) in read {
            let guest = parsed.and_then(|parsed| {
                let (rsvp, event) = found.remove(&parsed.id)?;
                Guest::checked(state, link, &parsed, rsvp, event)
            });
            let Some(guest) = guest else {
                remembered
                    .cookies
                    .push(cookie::set(cookie_name, "", "/", 0, https));
                continue;
            };
            if !remembered
                .guests
                .iter()
                .any(|known| known.event.id == guest.event.id)
            {
                remembered.guests.push(guest);
            }
        }
        Ok(remembered)
    }
}

#[cfg(test)]
mod tests;
