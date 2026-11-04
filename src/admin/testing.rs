//! A logged-in host, for the admin's tests: requests through the real router with a
//! session cookie of the role the test picks.

use axum::{
    body::Body,
    http::{Request, header},
    response::Response,
};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{auth, role::Role, router};
use crate::{
    config::{AdminConfig, Config, tests::config},
    db::rsvp::{self, Rsvp},
    paths,
};

pub struct Admin {
    pub db: PgPool,
    pub config: Config,
    pub admin: AdminConfig,
    /// The `token` cookie of a login.
    pub token: String,
}

impl Admin {
    pub fn new(db: PgPool) -> Self {
        Self::as_role(db, Role::Admin)
    }

    /// A host with this role, as single sign-on would log them in.
    pub fn as_role(db: PgPool, role: Role) -> Self {
        Self::as_user(db, "admin", Some(role))
    }

    /// A host by this name, with this role of their own, or none.
    pub fn as_user(db: PgPool, user: &str, role: Option<Role>) -> Self {
        let admin = auth::tests::admin_config();
        let token = auth::tests::token(&admin, db.clone(), user, role);
        Self {
            token: format!("token={token}"),
            db,
            config: config(),
            admin,
        }
    }

    pub async fn send(&self, request: axum::http::request::Builder, body: &str) -> Response {
        // The tests read the British texts.
        let request = request
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT_LANGUAGE, "en-GB")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(body.to_owned()))
            .unwrap();
        router(self.config.clone(), &self.admin, self.db.clone())
            .oneshot(request)
            .await
            .unwrap()
    }

    pub fn logged_in(&self, request: axum::http::request::Builder) -> axum::http::request::Builder {
        request.header(header::COOKIE, &self.token)
    }

    pub async fn get(&self, uri: &str) -> Response {
        self.send(self.logged_in(Request::get(uri)), "").await
    }

    pub async fn post(&self, uri: &str, form: &str) -> Response {
        self.send(self.logged_in(Request::post(uri)), form).await
    }

    /// The path of the tests' event, under which its tabs are. The event is made the
    /// first time it's asked for.
    pub async fn event(&self) -> String {
        paths::path(paths::EVENT, self.event_id().await)
    }

    pub async fn event_id(&self) -> Uuid {
        crate::db::event::tests::event(&self.db).await.id
    }

    /// An invitation to the tests' event.
    pub async fn invite(&self, name: &str, max_party_size: i32) -> Rsvp {
        let event = self.event_id().await;
        rsvp::create_invite(&self.db, event, name, max_party_size)
            .await
            .unwrap()
    }

    /// The tests' event's guests, oldest first.
    pub async fn guests(&self) -> Vec<Rsvp> {
        rsvp::list(&self.db, self.event_id().await).await.unwrap()
    }
}

/// The `name=value` of each cookie the response sets, and the whole header.
pub fn set_cookies(response: &Response) -> Vec<(String, String)> {
    crate::testing::set_cookies(response)
        .into_iter()
        .map(|value| {
            (
                value.split(';').next().unwrap().to_owned(),
                value.to_owned(),
            )
        })
        .collect()
}

/// The `value` of every read-only link field on the page.
pub fn links(html: &str) -> Vec<&str> {
    html.split(r#"readonly value=""#)
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect()
}
