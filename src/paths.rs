//! Every path the two sites serve. The routers register these patterns and links are
//! built from the same ones, so a path changes in one place.

use std::{borrow::Borrow, fmt::Display};

use uuid::Uuid;

use crate::{
    i18n::Lang,
    theme::{self, Theme},
};

// The public site. A `{link}` is a signed link from `crate::link`.
pub const HOME: &str = "/";
pub const PRIVACY: &str = "/privacy";
pub const LOST: &str = "/lost";
pub const STYLESHEET: &str = "/static/style.css";
pub const ROBOTS: &str = "/robots.txt";
pub const HEALTH: &str = "/healthz";
pub const READY: &str = "/readyz";
pub const RSVP: &str = "/r/{link}";
pub const RSVP_NAME: &str = "/r/{link}/name";
pub const RSVP_GUESTS: &str = "/r/{link}/guests";
pub const RSVP_LISTING: &str = "/r/{link}/listing";
pub const RSVP_EXPORT: &str = "/r/{link}/export";
pub const RSVP_DELETE: &str = "/r/{link}/delete";
pub const RSVP_REMEMBER: &str = "/r/{link}/remember";
pub const RSVP_FORGET: &str = "/r/{link}/forget";
pub const FORGET: &str = "/forget";
pub const RSVP_EMAIL: &str = "/r/{link}/email";
pub const RSVP_EMAIL_REMOVE: &str = "/r/{link}/email/remove";
pub const JOIN: &str = "/join/{link}";
pub const JOIN_GUESTS: &str = "/join/{link}/guests";
pub const CONFIRM_EMAIL: &str = "/e/{link}";
/// What the minigames call from their own origin, authorised by a score token rather
/// than the page they're on; see `web::games`.
pub const API: &str = "/api/";
pub const GAME_SCORES: &str = "/api/games/{game}/scores";
/// The same for a game's own server, on the internal port (`INNBY_INTERNAL_LISTEN`).
pub const INTERNAL_GAME_SCORES: &str = "/internal/games/{game}/scores";

// The admin site. `HOME` lists the events; each event's pages are under `EVENT`, and its
// guests and groups have pages of their own. Each parameter is named for what it is,
// which `admin::role::for_event` reads to find the event a page is about.
pub const EVENTS: &str = "/events";
pub const EVENT: &str = "/events/{event}";
pub const EVENT_INVITATIONS: &str = "/events/{event}/invitations";
pub const EVENT_GROUPS: &str = "/events/{event}/groups";
pub const EVENT_DETAILS: &str = "/events/{event}/details";
pub const EVENT_DELETE: &str = "/events/{event}/delete";
pub const EVENT_CSV: &str = "/events/{event}/guests.csv";
pub const EVENT_EXPORT: &str = "/events/{event}/event.toml";
pub const EVENTS_IMPORT: &str = "/events/import";
pub const EVENT_REMINDERS: &str = "/events/{event}/reminders";
pub const EVENT_QR: &str = "/events/{event}/qr-style";
pub const EVENT_GAMES: &str = "/events/{event}/games";
pub const EVENT_ACCESS: &str = "/events/{event}/access";
pub const EVENT_ACCESS_REMOVE: &str = "/events/{event}/access/remove";
pub const GUEST: &str = "/guests/{guest}";
pub const GUEST_REVOKE: &str = "/guests/{guest}/revoke";
pub const GUEST_DELETE: &str = "/guests/{guest}/delete";
pub const GUEST_QR: &str = "/guests/{guest}/qr.svg";
pub const GROUP: &str = "/groups/{group}";
pub const GROUP_REVOKE: &str = "/groups/{group}/revoke";
pub const GROUP_DELETE: &str = "/groups/{group}/delete";
pub const GROUP_QR: &str = "/groups/{group}/qr.svg";
/// The password users an admin makes, by name.
pub const USERS: &str = "/users";
pub const USER: &str = "/users/{user}";
pub const USER_DELETE: &str = "/users/{user}/delete";
pub const LOGIN: &str = "/login";
/// Logging in through the OpenID Connect provider, and coming back from it.
pub const LOGIN_OIDC: &str = "/login/oidc";
pub const LOGIN_OIDC_CALLBACK: &str = crate::config::OIDC_CALLBACK;
pub const LOGOUT: &str = "/logout";

/// A pattern with its one parameter filled in: `path(RSVP, link)` is `/r/{link}`.
pub fn path(pattern: &str, value: impl Display) -> String {
    let (head, rest) = pattern
        .split_once('{')
        .expect("the pattern has a parameter");
    let (_, tail) = rest.split_once('}').expect("the parameter is closed");
    format!("{head}{value}{tail}")
}

/// The path in a language, and in the colour scheme the guest picked. Links carry both,
/// since the site sets no cookies.
pub fn localized(path: &str, lang: Lang) -> String {
    format!("{path}{}", themed(lang, Theme::current()))
}

/// The query for this page in a language and colour scheme, for the switches. Takes
/// references too, as templates pass them.
pub fn themed(lang: impl Borrow<Lang>, theme: impl Borrow<Theme>) -> String {
    let (lang, theme) = (lang.borrow(), theme.borrow());
    format!("?lang={}{}", lang.code(), theme.param())
}

/// The colour scheme switch on a public page in this language.
pub fn theme_choices(lang: impl Borrow<Lang>) -> Vec<theme::Choice> {
    let lang = *lang.borrow();
    Theme::choices(|theme| themed(lang, theme))
}

/// An admin path under the admin UI's prefix: `/` under `/admin` is `/admin`, and
/// `/events` is `/admin/events`.
pub fn admin_under(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix('/') {
        Some(rest) if rest.is_empty() || rest.starts_with(['?', '#']) => format!("{prefix}{rest}"),
        _ => format!("{prefix}{path}"),
    }
}

/// An admin path, under the admin UI's prefix, in the language and colour scheme the
/// host picked, if they did.
pub fn admin(path: &str) -> String {
    let path = admin_under(&crate::admin::prefix(), path);
    let (path, fragment) = path.split_at(path.find('#').unwrap_or(path.len()));
    let mut url = path.to_owned();
    let lang = crate::admin::lang::picked().map(|lang| format!("lang={}", lang.code()));
    let theme = Theme::current()
        .param()
        .strip_prefix('&')
        .map(str::to_owned);
    for param in [lang, theme].into_iter().flatten() {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&param);
    }
    url + fragment
}

/// The colour scheme switch on an admin page, which keeps a picked language.
pub fn admin_theme_choices() -> Vec<theme::Choice> {
    Theme::choices(|theme| match crate::admin::lang::picked() {
        Some(lang) => format!("?lang={}&theme={}", lang.code(), theme.code()),
        None => format!("?theme={}", theme.code()),
    })
}

/// The pages every public page can link to, in this language.
impl Lang {
    pub fn home(self) -> String {
        localized(HOME, self)
    }

    pub fn privacy(self) -> String {
        localized(PRIVACY, self)
    }

    pub fn lost(self) -> String {
        localized(LOST, self)
    }

    pub fn forget(self) -> String {
        localized(FORGET, self)
    }
}

/// A guest's page and what its forms post to, in the page's language.
pub struct GuestLinks {
    pub page: String,
    pub guests: String,
    pub name: String,
    pub listing: String,
    pub email: String,
    pub email_remove: String,
    pub delete: String,
    pub export: String,
    pub remember: String,
    pub forget: String,
}

impl GuestLinks {
    pub fn new(link: &str, lang: Lang) -> Self {
        let at = |pattern| localized(&path(pattern, link), lang);
        Self {
            page: at(RSVP),
            guests: at(RSVP_GUESTS),
            name: at(RSVP_NAME),
            listing: at(RSVP_LISTING),
            email: at(RSVP_EMAIL),
            email_remove: at(RSVP_EMAIL_REMOVE),
            delete: at(RSVP_DELETE),
            export: path(RSVP_EXPORT, link),
            remember: at(RSVP_REMEMBER),
            forget: at(RSVP_FORGET),
        }
    }
}

/// An admin page for one guest or group, and the actions on it.
pub struct AdminLinks {
    pub page: String,
    pub revoke: String,
    pub delete: String,
    /// The QR code as a file.
    pub qr: String,
}

impl AdminLinks {
    pub fn guest(id: Uuid) -> Self {
        Self {
            page: admin(&path(GUEST, id)),
            revoke: admin(&path(GUEST_REVOKE, id)),
            delete: admin(&path(GUEST_DELETE, id)),
            qr: admin(&path(GUEST_QR, id)),
        }
    }

    pub fn group(id: Uuid) -> Self {
        Self {
            page: admin(&path(GROUP, id)),
            revoke: admin(&path(GROUP_REVOKE, id)),
            delete: admin(&path(GROUP_DELETE, id)),
            qr: admin(&path(GROUP_QR, id)),
        }
    }
}

/// The admin pages of one event, carrying the language and colour scheme.
pub struct EventLinks {
    pub guests: String,
    pub invitations: String,
    pub groups: String,
    pub details: String,
    /// The QR code style.
    pub qr_style: String,
    /// The minigames the event offers.
    pub games: String,
    /// Who else may see or run the event.
    pub access: String,
    pub access_remove: String,
    pub delete: String,
    pub csv: String,
    pub reminders: String,
    /// The event as a file.
    pub export: String,
    /// The events page, with the new event's form filled in from this one.
    pub copy: String,
}

impl EventLinks {
    pub fn new(id: Uuid) -> Self {
        let at = |pattern| admin(&path(pattern, id));
        Self {
            guests: at(EVENT),
            invitations: at(EVENT_INVITATIONS),
            groups: at(EVENT_GROUPS),
            details: at(EVENT_DETAILS),
            qr_style: at(EVENT_QR),
            games: at(EVENT_GAMES),
            access: at(EVENT_ACCESS),
            access_remove: at(EVENT_ACCESS_REMOVE),
            delete: at(EVENT_DELETE),
            csv: at(EVENT_CSV),
            reminders: at(EVENT_REMINDERS),
            export: at(EVENT_EXPORT),
            copy: admin(&format!("{HOME}?copy={id}#new-event")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_the_parameter() {
        assert_eq!(
            path(RSVP_EMAIL_REMOVE, "abc.def"),
            "/r/abc.def/email/remove"
        );
        assert_eq!(
            localized(&path(JOIN, "x.y"), Lang::EnGb),
            "/join/x.y?lang=en"
        );
        assert_eq!(Lang::Nb.privacy(), "/privacy?lang=nb");
        let id = Uuid::nil();
        assert_eq!(AdminLinks::group(id).revoke, format!("/groups/{id}/revoke"));
    }
}
