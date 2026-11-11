//! What the admin's pages share: their state, the tabs under the header, the trail of
//! where a page is, loading the event a path names, how a moment shows in a table, and
//! the error a page can end in.

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
    icons::{self, TabLink},
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
/// event, the users and the site's languages, or, within an event, its own pages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Events,
    NewEvent,
    Users,
    Languages,
    Guests,
    Invitations,
    Groups,
    Qr,
    Games,
    Board,
    Details,
    Access,
}

impl Tab {
    /// A line drawing beside the tab's name, so the tabs can be told apart at a glance.
    fn icon(self) -> &'static str {
        match self {
            Tab::Events => icons::CALENDAR,
            Tab::NewEvent => icons::CALENDAR_PLUS,
            Tab::Users => icons::PERSON,
            Tab::Languages => icons::GLOBE,
            Tab::Guests => icons::PERSON_TICK,
            Tab::Invitations => icons::ENVELOPE,
            Tab::Groups => icons::PEOPLE,
            Tab::Qr => icons::QR_CODE,
            Tab::Games => icons::GAMEPAD,
            Tab::Board => icons::SPEECH,
            Tab::Details => icons::PENCIL,
            Tab::Access => icons::KEY,
        }
    }
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
                Role::can_create(),
            ),
            (
                Tab::Users,
                paths::admin(paths::USERS),
                &text.users,
                Role::is_admin(),
            ),
            (
                Tab::Languages,
                paths::admin(paths::LANGUAGES),
                &text.languages,
                Role::is_admin(),
            ),
        ],
        Some(&id) => {
            let links = EventLinks::new(id);
            let (organise, edit, manage) = (
                Role::can_organise(),
                Role::can_edit(),
                Role::can_manage_event(),
            );
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
                (Tab::Board, links.board, &text.board, organise),
                (Tab::Details, links.details, &text.details, edit),
                (Tab::Access, links.access, &text.access, manage),
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

/// One step of the trail above a page's tabs, from the top down to the page: a link
/// to each level above, and the page's own level without one.
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

impl Crumb {
    fn up(label: impl Into<String>, href: String) -> Self {
        Self {
            label: label.into(),
            href: Some(href),
        }
    }

    fn here(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

/// The events, the top of every event's trail.
fn events_crumb() -> Crumb {
    Crumb::up(&lang::text().events, paths::admin(paths::HOME))
}

/// Where an event's own pages are: in the events.
pub fn event_trail(event: &Event) -> Vec<Crumb> {
    vec![
        events_crumb(),
        Crumb::here(event.details.title(lang::current())),
    ]
}

/// Where a guest's page is: in their event, whose Guests tab lists them.
pub fn guest_trail(event: &Event, name: &str) -> Vec<Crumb> {
    let title = event.details.title(lang::current());
    vec![
        events_crumb(),
        Crumb::up(title, EventLinks::new(event.id).guests),
        Crumb::here(name),
    ]
}

/// Where a group's page is: in its event's Group links.
pub fn group_trail(event: &Event, name: &str) -> Vec<Crumb> {
    let (text, links) = (lang::text(), EventLinks::new(event.id));
    let title = event.details.title(lang::current());
    vec![
        events_crumb(),
        Crumb::up(title, links.guests),
        Crumb::up(&text.group_links, links.groups),
        Crumb::here(name),
    ]
}

/// Where a password user's page is: in the users.
pub fn user_trail(name: &str) -> Vec<Crumb> {
    let users = Crumb::up(&lang::text().users, paths::admin(paths::USERS));
    vec![users, Crumb::here(name)]
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

/// Back to the page a change was made on, `pattern` filled with `id`, with `?saved`
/// for it to say so (see `forms::SavedQuery`).
pub(super) fn saved(pattern: &str, id: impl std::fmt::Display) -> Response {
    redirect(&format!("{}?saved", paths::path(pattern, id)))
}
