//! What a host may do in the admin UI. The built-in user is an admin; a single sign-on
//! user gets the highest role their identity provider lists for Innby.
//!
//! - **viewer** sees the events, their headcounts and who's coming. Not the personal
//!   links, e-mail addresses or the guest export: those let you act as a guest.
//! - **editor** runs events that exist: invites guests, makes group links, sends
//!   reminders, downloads the guest list and changes an event's details.
//! - **admin** also creates and deletes events, and styles the QR codes.
//!
//! Routes are grouped by the role they need (see `admin::router`), and pages read the
//! role of the request from here to show only what it may use.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use super::lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Admin,
}

tokio::task_local! {
    static ROLE: Role;
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Editor, Role::Admin];

    /// The name an identity provider lists, as in `innby: [editor]`.
    pub fn name(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }

    /// The role of the request being handled. Outside a login, the least.
    pub fn current() -> Role {
        ROLE.try_with(|role| *role).unwrap_or(Role::Viewer)
    }

    /// Whether the request may change an event's guests, links and details.
    pub fn can_edit() -> bool {
        Role::current() >= Role::Editor
    }

    /// Whether the request may create and delete events and change the QR style.
    pub fn is_admin() -> bool {
        Role::current() >= Role::Admin
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

    /// Handles the request as this role.
    pub async fn scope<T>(self, future: impl Future<Output = T>) -> T {
        ROLE.scope(self, future).await
    }
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
    if Role::current() >= role {
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
        assert_eq!(Role::current(), Role::Viewer);
        Role::Editor
            .scope(async {
                assert!(Role::can_edit());
                assert!(!Role::is_admin());
            })
            .await;
    }
}
