//! Every path the two sites serve. The routers register these patterns and links are
//! built from the same ones, so a path changes in one place.

use std::{borrow::Borrow, fmt::Display};

use uuid::Uuid;

use crate::{i18n::Lang, theme::Theme};

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
pub const LOGOUT: &str = "/logout";
pub const LOGGED_OUT: &str = "/logged-out";

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

/// An admin path in the language and colour scheme the host picked, if they did.
pub fn admin(path: &str) -> String {
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

/// The query for this admin page in another colour scheme, keeping a picked language.
pub fn admin_themed(theme: impl Borrow<Theme>) -> String {
    let theme = theme.borrow().code();
    match crate::admin::lang::picked() {
        Some(lang) => format!("?lang={}&theme={theme}", lang.code()),
        None => format!("?theme={theme}"),
    }
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
            page: admin(&path(GUEST, id)),
            revoke: admin(&path(GUEST_REVOKE, id)),
            delete: admin(&path(GUEST_DELETE, id)),
        }
    }

    pub fn group(id: Uuid) -> Self {
        Self {
            page: admin(&path(GROUP, id)),
            revoke: admin(&path(GROUP_REVOKE, id)),
            delete: admin(&path(GROUP_DELETE, id)),
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
