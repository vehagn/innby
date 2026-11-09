//! What the admin's pages share: their state, the tabs under the header, loading the
//! event a path names, how a moment shows in a table, and the error a page can end in.

use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;
use uuid::Uuid;

use super::{auth, lang, role::Role};
use crate::{
    config::Config,
    db::event::{self, Event},
    http::{ClientIp, server_error},
    paths::{self, EventLinks},
};

#[derive(Clone)]
pub(super) struct AdminState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub auth: Arc<auth::Auth>,
}

impl FromRef<AdminState> for Arc<auth::Auth> {
    fn from_ref(state: &AdminState) -> Self {
        Arc::clone(&state.auth)
    }
}

impl FromRef<AdminState> for PgPool {
    fn from_ref(state: &AdminState) -> Self {
        state.db.clone()
    }
}

/// The client, behind the gateway if the config trusts it, for the login's limits.
impl FromRequestParts<AdminState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self::from_parts(parts, state.config.trust_forwarded))
    }
}

/// The admin's sections, as tabs under the header: the events and, for an admin, a new
/// event and the users, or, within an event, its own pages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Events,
    NewEvent,
    Users,
    Guests,
    Invitations,
    Groups,
    Qr,
    Games,
    Details,
    Access,
}

/// An icon's drawing in the SVG every icon shares.
macro_rules! svg {
    ($drawing:literal) => {
        concat!(
            r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">"#,
            $drawing,
            "</svg>"
        )
    };
}

impl Tab {
    /// A line drawing beside the tab's name, in the text's colour, so the tabs can be
    /// told apart at a glance.
    fn icon(self) -> &'static str {
        match self {
            Tab::Events => svg!(
                r#"<rect x="4" y="5" width="16" height="15" rx="2"/><path d="M4 10h16M8 3v4M16 3v4"/>"#
            ),
            Tab::NewEvent => svg!(
                r#"<rect x="4" y="5" width="16" height="15" rx="2"/><path d="M4 10h16M8 3v4M16 3v4M12 13v5M9.5 15.5h5"/>"#
            ),
            Tab::Users => {
                svg!(r#"<circle cx="12" cy="8" r="4"/><path d="M4 21c0-4.4 3.6-8 8-8s8 3.6 8 8"/>"#)
            }
            Tab::Guests => svg!(
                r#"<circle cx="9" cy="8" r="3.5"/><path d="M3 20c0-3.3 2.7-6 6-6 1.6 0 3 .6 4.1 1.6M15 18l2 2 4-4"/>"#
            ),
            Tab::Invitations => {
                svg!(r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>"#)
            }
            Tab::Groups => svg!(
                r#"<circle cx="12" cy="8" r="3"/><circle cx="5" cy="10" r="2"/><circle cx="19" cy="10" r="2"/><path d="M6.5 20c0-3 2.5-5.5 5.5-5.5s5.5 2.5 5.5 5.5M2 18c0-1.9 1.1-3.4 2.7-3.9M22 18c0-1.9-1.1-3.4-2.7-3.9"/>"#
            ),
            Tab::Qr => svg!(
                r#"<rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><path d="M14 14h2v2M20 14v.01M14 20h.01M17 17v3h3"/>"#
            ),
            Tab::Games => svg!(
                r#"<rect x="2.5" y="7" width="19" height="11" rx="5.5"/><path d="M7.5 11v3M6 12.5h3M15.5 12h.01M18 14h.01"/>"#
            ),
            Tab::Details => svg!(r#"<path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4"/>"#),
            Tab::Access => svg!(
                r#"<circle cx="8" cy="15" r="4"/><path d="M10.8 12.2L20 3M16 7l3 3M14 9l2 2"/>"#
            ),
        }
    }
}

pub struct TabLink {
    pub href: String,
    pub label: &'static str,
    /// Inline SVG, hidden from screen readers, which read the label.
    pub icon: &'static str,
    pub current: bool,
}

/// The tabs of the site, or with `event`, that event's tabs.
pub fn tabs(event: Option<&Uuid>, current: Tab) -> Vec<TabLink> {
    let text = lang::text();
    // Only the tabs the host's role may open.
    let tabs = match event {
        None => vec![
            (Tab::Events, paths::admin(paths::HOME), &text.events, true),
            (
                Tab::NewEvent,
                paths::admin(paths::EVENTS_NEW),
                &text.new_event,
                Role::is_admin(),
            ),
            (
                Tab::Users,
                paths::admin(paths::USERS),
                &text.users,
                Role::is_admin(),
            ),
        ],
        Some(&id) => {
            let links = EventLinks::new(id);
            let (edit, admin) = (Role::can_edit(), Role::is_admin());
            vec![
                (Tab::Guests, links.guests, &text.guests, true),
                (
                    Tab::Invitations,
                    links.invitations,
                    &text.invitation_links,
                    edit,
                ),
                (Tab::Groups, links.groups, &text.group_links, edit),
                (Tab::Qr, links.qr_style, &text.qr_heading, edit),
                (
                    Tab::Games,
                    links.games,
                    &text.games,
                    edit && super::has_games(),
                ),
                (Tab::Details, links.details, &text.details, edit),
                (Tab::Access, links.access, &text.access, admin),
            ]
        }
    };
    tabs.into_iter()
        .filter(|(_, _, _, allowed)| *allowed)
        .map(|(tab, href, label, _)| (tab, href, label))
        .map(|(tab, href, label)| TabLink {
            href,
            label,
            icon: tab.icon(),
            current: tab == current,
        })
        .collect()
}

/// A moment as the admin's tables show it, in the event's time zone: `2026-12-12 19:00`.
pub(super) fn timestamp(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

/// The event behind an admin path's `{event}`.
pub(super) async fn load_event(state: &AdminState, id: Uuid) -> Result<Event, PageError> {
    event::find(&state.db, id).await?.ok_or(PageError::Missing)
}

/// Why a request about one guest or group failed.
pub(super) enum PageError {
    Missing,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for PageError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        match self {
            Self::Missing => {
                (StatusCode::NOT_FOUND, lang::text().not_found.as_str()).into_response()
            }
            Self::Database(err) => server_error(err),
        }
    }
}

/// Sends the browser on to an admin page, under the admin's path and in the host's
/// language and colour scheme.
pub(super) fn redirect(page: &str) -> Response {
    Redirect::to(&paths::admin(page)).into_response()
}
