//! The login page, logging in as the built-in user, and logging out.

use std::sync::Arc;

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
};
use chrono::TimeDelta;
use serde::Deserialize;

use super::{Auth, Outcome, REFRESH_TOKEN, TOKEN};
use crate::{
    admin::lang,
    http::{ClientIp, render, render_with, server_error},
    paths,
};

#[derive(Template, Default)]
#[template(path = "admin/login.html")]
struct LoginPage {
    logged_out: bool,
    /// Whether the built-in user may log in.
    password: bool,
    /// The provider's name, when there is one.
    oidc: Option<String>,
    /// The user name typed in, when the login failed.
    user: String,
    error: Option<String>,
}

impl LoginPage {
    fn new(auth: &Auth) -> Self {
        Self {
            password: auth.credentials.is_some(),
            oidc: auth.oidc.as_ref().map(|oidc| oidc.name().to_owned()),
            ..Self::default()
        }
    }
}

/// The login page, with what went wrong.
pub(in crate::admin) fn login_error(auth: &Auth, status: StatusCode, error: String) -> Response {
    let page = LoginPage {
        error: Some(error),
        ..LoginPage::new(auth)
    };
    render_with(status, page)
}

#[derive(Deserialize)]
pub struct LoginQuery {
    logged_out: Option<String>,
}

pub async fn login_page(
    State(auth): State<Arc<Auth>>,
    Query(query): Query<LoginQuery>,
) -> Response {
    render(LoginPage {
        logged_out: query.logged_out.is_some(),
        ..LoginPage::new(&auth)
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    user: String,
    password: String,
}

pub async fn log_in(
    State(auth): State<Arc<Auth>>,
    client: ClientIp,
    Form(form): Form<LoginForm>,
) -> Response {
    let text = lang::text();
    let (status, error) = match auth.check(client, form.user.clone(), form.password).await {
        Outcome::Accepted => {
            return match auth.start_session().await {
                Ok(cookies) => {
                    tracing::info!("host logged in");
                    let cookies = cookies
                        .into_iter()
                        .map(|cookie| (header::SET_COOKIE, cookie));
                    let home = Redirect::to(&paths::admin(paths::HOME));
                    (AppendHeaders(cookies), home).into_response()
                }
                Err(err) => server_error(err),
            };
        }
        Outcome::Rejected => (StatusCode::UNAUTHORIZED, &text.wrong_login),
        Outcome::TooManyAttempts => (StatusCode::TOO_MANY_REQUESTS, &text.too_many_logins),
    };
    let page = LoginPage {
        user: form.user,
        error: Some(error.clone()),
        ..LoginPage::new(&auth)
    };
    render_with(status, page)
}

/// Needs no login, so a browser whose JWT has expired can still forget its refresh token.
pub async fn log_out(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Err(err) = auth.end_session(&headers).await {
        return server_error(err);
    }
    tracing::info!("host logged out");
    let cleared = [TOKEN, REFRESH_TOKEN]
        .map(|name| (header::SET_COOKIE, auth.cookie(name, "", TimeDelta::zero())));
    let page = paths::admin(&format!("{}?logged_out", paths::LOGIN));
    (AppendHeaders(cleared), Redirect::to(&page)).into_response()
}
