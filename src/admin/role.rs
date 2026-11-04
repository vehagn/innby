//! What a host may do in the admin UI. A user who logs in with a password has the role
//! the config gives them (the built-in user is an admin); a single sign-on user gets the
//! highest role their identity provider lists for Innby. Either may have none.
//!
//! - **viewer** sees the events, their headcounts and who's coming. Not the personal
//!   links, e-mail addresses or the guest export: those let you act as a guest.
//! - **editor** runs events that exist: invites guests, makes group links, sends
//!   reminders, downloads the guest list, and changes an event's details and the style
//!   of its QR codes.
//! - **admin** also creates and deletes events, and gives hosts roles for one event.
//!
//! A host's own role is for every event. An admin can give anyone a role for one event,
//! viewer or editor, by the name they log in with (`db::access`): on that event's pages,
//! and its guests' and groups', they have the higher of the two. A host without a role of
//! their own sees only the events they have one for.
//!
//! Routes are grouped by the role they need (see `admin::router`), and pages read the
//! role of the request from here to show only what it may use.

use std::sync::Arc;

use axum::{
    extract::{RawPathParams, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::lang;
use crate::{
    db::access::{self, EventRole, Scope},
    http::server_error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Admin,
}

impl From<EventRole> for Role {
    fn from(role: EventRole) -> Self {
        match role {
            EventRole::Viewer => Role::Viewer,
            EventRole::Editor => Role::Editor,
        }
    }
}

/// Who a request is from.
#[derive(Debug, Clone)]
pub struct Host {
    /// The name they logged in with, by which an event gives them a role.
    pub user: Arc<str>,
    /// Their role for every event, or on an event's pages, the higher of that and the
    /// event's for them.
    pub role: Option<Role>,
}

tokio::task_local! {
    static HOST: Host;
}

impl Host {
    pub fn new(user: &str, role: Option<Role>) -> Self {
        Self {
            user: Arc::from(user),
            role,
        }
    }

    /// The host of the request being handled. Outside a login, nobody with no role.
    pub fn current() -> Host {
        HOST.try_with(Host::clone)
            .unwrap_or_else(|_| Host::new("", None))
    }

    /// Handles the request as this host.
    pub async fn scope<T>(self, future: impl Future<Output = T>) -> T {
        HOST.scope(self, future).await
    }
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Editor, Role::Admin];

    /// The name an identity provider lists, as in `innby: [editor]`, and the config.
    pub fn name(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }

    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.name() == name)
    }

    /// The role of the request being handled, if it has one.
    pub fn current() -> Option<Role> {
        HOST.try_with(|host| host.role).ok().flatten()
    }

    /// Whether the request may change an event's guests, links and details.
    pub fn can_edit() -> bool {
        Role::current() >= Some(Role::Editor)
    }

    /// Whether the request may create and delete events, and give roles for one.
    pub fn is_admin() -> bool {
        Role::current() >= Some(Role::Admin)
    }

    /// The highest role among a claim's values: a list of names or a single one.
    /// Names Innby doesn't know are ignored, so the same claim can serve other apps.
    pub fn from_claim(claim: &serde_json::Value) -> Option<Role> {
        let names: Vec<&str> = match claim {
            serde_json::Value::String(name) => vec![name.as_str()],
            serde_json::Value::Array(names) => {
                names.iter().filter_map(serde_json::Value::as_str).collect()
            }
            _ => Vec::new(),
        };
        Role::ALL
            .into_iter()
            .rev()
            .find(|role| names.contains(&role.name()))
    }
}

/// Middleware for every page behind the login: on a page about an event, or about one
/// of its guests or groups, the host has the higher of their own role and the one the
/// event gives them. The path's parameter says which it's about (see `paths`).
pub async fn for_event(
    State(db): State<PgPool>,
    params: RawPathParams,
    request: Request,
    next: Next,
) -> Response {
    let host = Host::current();
    let id = |name: &str| {
        params
            .iter()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value.parse::<Uuid>().ok())
    };
    let scope = (id("event").map(Scope::Event))
        .or_else(|| id("guest").map(Scope::Guest))
        .or_else(|| id("group").map(Scope::Group));
    // No event gives more than an editor's role.
    let Some(scope) = scope.filter(|_| host.role < Some(Role::Editor)) else {
        return next.run(request).await;
    };
    match access::role(&db, scope, &host.user).await {
        Ok(given) => {
            let role = host.role.max(given.map(Role::from));
            Host { role, ..host }.scope(next.run(request)).await
        }
        Err(err) => server_error(err),
    }
}

/// Middleware for routes that need a viewer.
pub async fn viewer(request: Request, next: Next) -> Response {
    require(Role::Viewer, request, next).await
}

/// Middleware for routes that need an editor.
pub async fn editor(request: Request, next: Next) -> Response {
    require(Role::Editor, request, next).await
}

/// Middleware for routes that need an admin.
pub async fn admin(request: Request, next: Next) -> Response {
    require(Role::Admin, request, next).await
}

async fn require(role: Role, request: Request, next: Next) -> Response {
    if Role::current() >= Some(role) {
        next.run(request).await
    } else {
        (StatusCode::FORBIDDEN, lang::text().forbidden.as_str()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn takes_the_highest_role_it_knows() {
        assert_eq!(
            Role::from_claim(&json!(["viewer", "admin"])),
            Some(Role::Admin)
        );
        assert_eq!(
            Role::from_claim(&json!(["editor", "owner"])),
            Some(Role::Editor)
        );
        assert_eq!(Role::from_claim(&json!("viewer")), Some(Role::Viewer));
        // Exact names only.
        assert_eq!(Role::from_claim(&json!(["Admin", "admins"])), None);
        assert_eq!(Role::from_claim(&json!([])), None);
        assert_eq!(Role::from_claim(&json!({ "admin": true })), None);
        assert_eq!(Role::from_claim(&json!(null)), None);
    }

    #[tokio::test]
    async fn the_role_lasts_for_the_request() {
        assert_eq!(Role::current(), None);
        Host::new("ola", Some(Role::Editor))
            .scope(async {
                assert!(Role::can_edit());
                assert!(!Role::is_admin());
                assert_eq!(&*Host::current().user, "ola");
            })
            .await;
    }
}
