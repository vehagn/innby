//! What the admin's pages share: their state, the tabs under the header, loading the
//! event a path names, and the error a page can end in.

use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
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

/// The admin's sections, as tabs under the header: the events and the QR code style,
/// or, within an event, its own pages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Events,
    Qr,
    Guests,
    Invitations,
    Groups,
    Details,
}

pub struct TabLink {
    pub href: String,
    pub label: &'static str,
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
                Tab::Qr,
                paths::admin(paths::QR_STYLE),
                &text.qr_heading,
                Role::is_admin(),
            ),
        ],
        Some(&id) => {
            let links = EventLinks::new(id);
            let edit = Role::can_edit();
            vec![
                (Tab::Guests, links.guests, &text.guests, true),
                (
                    Tab::Invitations,
                    links.invitations,
                    &text.invitation_links,
                    edit,
                ),
                (Tab::Groups, links.groups, &text.group_links, edit),
                (Tab::Details, links.details, &text.details, edit),
            ]
        }
    };
    tabs.into_iter()
        .filter(|(_, _, _, allowed)| *allowed)
        .map(|(tab, href, label, _)| (tab, href, label))
        .map(|(tab, href, label)| TabLink {
            href,
            label,
            current: tab == current,
        })
        .collect()
}

/// The event behind an admin path's `{id}`.
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
