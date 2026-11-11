//! A password user's own page, for those an admin made in the admin UI: their e-mail
//! address, for a link to set a new password if they forget it (`auth::reset`), and a
//! new password, which asks for the current one. The config's users and a provider's
//! have none: their logins are set elsewhere.

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    http::header,
    response::{AppendHeaders, IntoResponse, Response},
};
use serde::Deserialize;

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, redirect},
    role::Host,
    users::check_password,
};
use crate::{
    db::{
        admin_user::{self, Taken},
        rsvp::normalize_email,
    },
    http::{ClientIp, render_form, server_error},
    paths,
};

#[derive(Template)]
#[template(path = "admin/account.html")]
struct AccountPage {
    user: String,
    email: String,
    notice: Option<String>,
    error: Option<String>,
}

/// The host, if they have an account page.
fn in_app() -> Result<Host, PageError> {
    let host = Host::current();
    if host.in_app {
        Ok(host)
    } else {
        Err(PageError::Missing)
    }
}

async fn account_page(
    state: &AdminState,
    host: &Host,
    email: Option<String>,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let user = admin_user::find(&state.db, &host.user)
        .await?
        .ok_or(PageError::Missing)?;
    let needs_fixing = error.is_some();
    let page = AccountPage {
        email: email.or(user.email).unwrap_or_default(),
        user: user.name,
        notice,
        error,
    };
    Ok(render_form(page, needs_fixing))
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let host = in_app()?;
    let notice = query.saved.map(|_| lang::text().saved.clone());
    account_page(&state, &host, None, notice, None).await
}

#[derive(Deserialize)]
pub(super) struct EmailForm {
    email: String,
}

/// Sets or clears the host's address.
pub(super) async fn edit(
    State(state): State<AdminState>,
    Form(form): Form<EmailForm>,
) -> Result<Response, PageError> {
    let host = in_app()?;
    let text = lang::text();
    let typed = form.email.trim();
    let email = match typed {
        "" => None,
        typed => match normalize_email(typed) {
            Some(email) => Some(email),
            None => {
                let error = Some(text.error_email.clone());
                return account_page(&state, &host, Some(typed.to_owned()), None, error).await;
            }
        },
    };
    match admin_user::set_email(&state.db, &host.user, email.as_deref()).await? {
        Ok(_) => {}
        Err(Taken::Email | Taken::Name) => {
            let error = Some(text.error_email_taken.clone());
            return account_page(&state, &host, Some(typed.to_owned()), None, error).await;
        }
    }
    tracing::info!(user = &*host.user, "own address changed");
    Ok(redirect(&format!("{}?saved", paths::ACCOUNT)))
}

#[derive(Deserialize)]
pub(super) struct PasswordForm {
    current: String,
    password: String,
}

/// A new password, if they know the current one. It logs them out everywhere, so this
/// browser gets a new session at once.
pub(super) async fn password(
    State(state): State<AdminState>,
    client: ClientIp,
    Form(form): Form<PasswordForm>,
) -> Result<Response, PageError> {
    let host = in_app()?;
    let text = lang::text();
    if let Err(error) = check_password(&form.password) {
        return account_page(&state, &host, None, None, Some(error)).await;
    }
    if !state.auth.knows(client, &host.user, form.current).await {
        let error = Some(text.error_current_password.clone());
        return account_page(&state, &host, None, None, error).await;
    }
    let hash = match state.auth.hash(form.password).await {
        Ok(hash) => hash,
        Err(err) => return Ok(server_error(err)),
    };
    admin_user::set_password(&state.db, &host.user, &hash).await?;
    let user = admin_user::find(&state.db, &host.user)
        .await?
        .ok_or(PageError::Missing)?;
    let cookies = state.auth.start_session(&user.name, user.version).await?;
    tracing::info!(user = &*host.user, "own password changed");
    let cookies = cookies
        .into_iter()
        .map(|cookie| (header::SET_COOKIE, cookie));
    let page = redirect(&format!("{}?saved", paths::ACCOUNT));
    Ok((AppendHeaders(cookies), page).into_response())
}
