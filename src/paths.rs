//! Every path the two sites serve. The routers register these patterns and links are
//! built from the same ones, so a path changes in one place.

use std::fmt::Display;

use uuid::Uuid;

use crate::i18n::Lang;

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
pub const RSVP_EMAIL: &str = "/r/{link}/email";
pub const RSVP_EMAIL_REMOVE: &str = "/r/{link}/email/remove";
pub const JOIN: &str = "/join/{link}";
pub const JOIN_GUESTS: &str = "/join/{link}/guests";
pub const CONFIRM_EMAIL: &str = "/e/{link}";

// The admin site.
pub const INVITATIONS: &str = "/invitations";
pub const GUEST: &str = "/guests/{id}";
pub const GUEST_REVOKE: &str = "/guests/{id}/revoke";
pub const GUEST_DELETE: &str = "/guests/{id}/delete";
pub const GUESTS_CSV: &str = "/guests.csv";
pub const GROUPS: &str = "/groups";
pub const GROUP: &str = "/groups/{id}";
pub const GROUP_REVOKE: &str = "/groups/{id}/revoke";
pub const GROUP_DELETE: &str = "/groups/{id}/delete";
pub const REMINDERS: &str = "/reminders";

/// A pattern with its one parameter filled in: `path(RSVP, link)` is `/r/{link}`.
pub fn path(pattern: &str, value: impl Display) -> String {
    let (head, rest) = pattern
        .split_once('{')
        .expect("the pattern has a parameter");
    let (_, tail) = rest.split_once('}').expect("the parameter is closed");
    format!("{head}{value}{tail}")
}

/// The path in a language. Links carry it, since the site sets no cookies.
pub fn localized(path: &str, lang: Lang) -> String {
    format!("{path}?lang={}", lang.code())
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
        }
    }
}

/// An admin page for one guest or group, and the actions on it.
pub struct AdminLinks {
    pub page: String,
    pub revoke: String,
    pub delete: String,
}

impl AdminLinks {
    pub fn guest(id: Uuid) -> Self {
        Self {
            page: path(GUEST, id),
            revoke: path(GUEST_REVOKE, id),
            delete: path(GUEST_DELETE, id),
        }
    }

    pub fn group(id: Uuid) -> Self {
        Self {
            page: path(GROUP, id),
            revoke: path(GROUP_REVOKE, id),
            delete: path(GROUP_DELETE, id),
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
