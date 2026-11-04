//! "Remember me on this device": a guest can ask the browser to keep their link in a
//! cookie, so the front page leads them back to their page. There's one cookie per event,
//! set only when the guest asks for it, so a device can remember invitations to several
//! events. A cookie authorises nothing: every change still needs the link in the address,
//! so it can at most show the way.

use axum::{
    Form,
    extract::State,
    http::{HeaderMap, HeaderValue},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AppState,
    rsvp::{Guest, Notice, redirect},
};
use crate::{http::cookie, i18n::Lang};

/// The most remembered links the front page reads. Each costs a lookup, and a guest is
/// rarely invited to more than a few events at once; a request with more is someone
/// making the site look up links for nothing.
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

/// The cookie that remembers the guest until their data is deleted, after the event.
fn remember_cookie(https: bool, guest: &Guest) -> HeaderValue {
    let max_age = (guest.event.deleted_at() - Utc::now()).num_seconds().max(0);
    cookie::set(
        &name(https, guest.event.id),
        &guest.link,
        "/",
        max_age,
        https,
    )
}

/// Tells the browser to drop the cookie for this event.
pub(super) fn forget_cookie(https: bool, event: Uuid) -> HeaderValue {
    cookie::set(&name(https, event), "", "/", 0, https)
}

/// Remembers the guest's link on this device until their data is deleted.
pub async fn remember(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let cookie = remember_cookie(state.config.is_https(), &guest);
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
        for (cookie_name, link) in links(headers, https).into_iter().take(MOST_REMEMBERED) {
            let Some(guest) = Guest::find(state, link).await? else {
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
