//! A new password by mail, for the password users made in the admin UI who have an
//! e-mail address: asking for a link on the login page (`/login/forgot`), and setting a
//! password from it (`/password/{user}/{token}`), which an admin's new user gets too.
//!
//! The link is signed with the link key, as guests' are, over the user, their address,
//! their password's version and when it stops working (`link::PasswordLink`), so it
//! works once, until the address changes, and not past the hour (a week for a new user).
//! Nothing secret is stored: the mailer signs it as the mail goes out. Asking never says
//! whether there's such a user, and is limited per client address and for everyone.

use std::sync::Arc;

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;

use super::Auth;
use crate::{
    admin::{lang, users::check_password},
    db::{admin_user, outbox},
    http::{ClientIp, render, render_with, server_error},
    link::PasswordLink,
    paths,
};

#[derive(Template, Default)]
#[template(path = "admin/forgot.html")]
struct ForgotPage {
    typed: String,
    sent: bool,
    error: Option<String>,
}

pub async fn forgot_page() -> Response {
    render(ForgotPage::default())
}

#[derive(Deserialize)]
pub struct ForgotForm {
    user: String,
}

/// Queues the link for the user with this name or address, if they have an address, and
/// says the same whether or not they do.
pub async fn forgot(
    State(auth): State<Arc<Auth>>,
    client: ClientIp,
    Form(form): Form<ForgotForm>,
) -> Response {
    let typed = form.user.trim().to_owned();
    if !auth.resets.check(client) || !auth.all_resets.check_all() {
        tracing::warn!("password link limit hit");
        let page = ForgotPage {
            error: Some(lang::text().too_many_resets.clone()),
            typed,
            ..ForgotPage::default()
        };
        return render_with(StatusCode::TOO_MANY_REQUESTS, page);
    }
    let found = match admin_user::find_by_name_or_email(&auth.db, &typed).await {
        Ok(found) => found.filter(|user| user.email.is_some()),
        Err(err) => return server_error(err),
    };
    if let Some(user) = found {
        let kind = outbox::Kind::PasswordReset;
        if let Err(err) =
            outbox::enqueue_for_host(&auth.db, &user.name, kind, lang::current()).await
        {
            return server_error(err);
        }
        tracing::info!(user = user.name, "password link asked for");
    }
    render(ForgotPage {
        sent: true,
        ..ForgotPage::default()
    })
}

#[derive(Template)]
#[template(path = "admin/password.html")]
struct PasswordPage {
    user: String,
    /// Where the form posts: the link itself.
    action: String,
    /// Whether the link doesn't work, so there's no form, but a way to a new one.
    dead: bool,
    error: Option<String>,
}

impl PasswordPage {
    fn new(path: &LinkPath) -> Self {
        let link = paths::path(paths::PASSWORD, &path.user).replace("{token}", &path.token);
        Self {
            user: path.user.clone(),
            action: paths::admin(&link),
            dead: false,
            error: None,
        }
    }
}

/// The page a link that doesn't work leads to.
fn dead_link(path: &LinkPath) -> Response {
    let page = PasswordPage {
        dead: true,
        ..PasswordPage::new(path)
    };
    render_with(StatusCode::GONE, page)
}

#[derive(Deserialize)]
pub struct LinkPath {
    user: String,
    token: String,
}

/// Whether the link is the user's as they are now, and not too old.
async fn valid(db: &PgPool, auth: &Auth, path: &LinkPath) -> sqlx::Result<bool> {
    let Ok(link) = path.token.parse::<PasswordLink>() else {
        return Ok(false);
    };
    if link.expires < Utc::now() {
        return Ok(false);
    }
    let Some(user) = admin_user::find(db, &path.user).await? else {
        return Ok(false);
    };
    let Some(email) = &user.email else {
        return Ok(false);
    };
    Ok(auth
        .link_key
        .verify_password(&link, &user.name, email, user.version))
}

pub async fn password_page(State(auth): State<Arc<Auth>>, Path(path): Path<LinkPath>) -> Response {
    match valid(&auth.db, &auth, &path).await {
        Ok(true) => render(PasswordPage::new(&path)),
        Ok(false) => dead_link(&path),
        Err(err) => server_error(err),
    }
}

#[derive(Deserialize)]
pub struct PasswordForm {
    password: String,
}

/// Sets the password, which bumps its version, so the link works no more and every
/// session ends; then on to the login.
pub async fn set_password(
    State(auth): State<Arc<Auth>>,
    Path(path): Path<LinkPath>,
    Form(form): Form<PasswordForm>,
) -> Response {
    match valid(&auth.db, &auth, &path).await {
        Ok(true) => {}
        Ok(false) => return dead_link(&path),
        Err(err) => return server_error(err),
    }
    if let Err(error) = check_password(&form.password) {
        let page = PasswordPage {
            error: Some(error),
            ..PasswordPage::new(&path)
        };
        return render_with(StatusCode::UNPROCESSABLE_ENTITY, page);
    }
    let hash = match auth.hash(form.password).await {
        Ok(hash) => hash,
        Err(err) => return server_error(err),
    };
    match admin_user::set_password(&auth.db, &path.user, &hash).await {
        Ok(_) => {}
        Err(err) => return server_error(err),
    }
    tracing::info!(user = path.user, "password set from a link");
    let login = paths::admin(&format!("{}?password_set", paths::LOGIN));
    Redirect::to(&login).into_response()
}
