//! A logged-in host, for the admin's tests: requests through the real router with a
//! session cookie of the role the test picks.

use axum::{
    body::Body,
    http::{Request, header},
    response::Response,
};
use sqlx::PgPool;
use tower::ServiceExt;

use super::{auth, role::Role, router};
use crate::{
    config::{AdminConfig, Config, tests::config},
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
        let admin = auth::tests::admin_config();
        Self {
            token: format!("token={}", auth::tests::token(&admin, db.clone(), role)),
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

    /// The path of the tests' event, under which its tabs are.
    pub async fn event(&self) -> String {
        let event = crate::db::event::tests::event(&self.db).await;
        paths::path(paths::EVENT, event.id)
    }
}

/// The `name=value` of each cookie the response sets, and the whole header.
pub fn set_cookies(response: &Response) -> Vec<(String, String)> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            let value = value.to_str().unwrap();
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
