//! A password user's own page, for those an admin made in the admin UI: their e-mail
//! address, for a link to set a new password if they forget it (`auth::reset`), and a
//! new password, which asks for the current one. The config's users and a provider's
//! have none: their logins are set elsewhere.

use std::sync::Arc;

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    http::header,
    response::{AppendHeaders, IntoResponse, Response},
};
use serde::Deserialize;

use super::{
    auth::jwt::PasswordIdentity,
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, redirect},
    role::Host,
    users::check_password,
};
use crate::{
    db::{
        admin_user::{self, AccountCredential, Taken},
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

/// The host whose page it is: their name, and the account they're logged in to. A change
/// is written only to that account with that password still, so one that finds it gone,
/// or its password changed since the request began, changes nothing: the login is over
/// by then, and the browser goes to log in again, as its next request would have.
struct Account {
    user: Arc<str>,
    credential: AccountCredential,
}

impl Account {
    /// The host's, if they have an account page.
    fn current() -> Result<Self, PageError> {
        let host = Host::current();
        let credential = host.credential.ok_or(PageError::Missing)?;
        Ok(Self {
            user: host.user,
            credential,
        })
    }
}

async fn account_page(
    state: &AdminState,
    account: &Account,
    email: Option<String>,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let user = admin_user::find(&state.db, &account.user)
        .await?
        .filter(|user| user.id == account.credential.id)
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
    let account = Account::current()?;
    let notice = query.saved.map(|_| lang::text().saved.clone());
    account_page(&state, &account, None, notice, None).await
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
    let account = Account::current()?;
    let text = lang::text();
    let typed = form.email.trim();
    let email = match typed {
        "" => None,
        typed => match normalize_email(typed) {
            Some(email) => Some(email),
            None => {
                let error = Some(text.error_email.clone());
                return account_page(&state, &account, Some(typed.to_owned()), None, error).await;
            }
        },
    };
    let set = admin_user::set_email_for_account(&state.db, account.credential, email.as_deref());
    match set.await? {
        Ok(true) => Ok(redirect(&format!("{}?saved", paths::ACCOUNT))),
        Ok(false) => Ok(state.auth.to_login().await),
        Err(Taken::Email | Taken::Name) => {
            let error = Some(text.error_email_taken.clone());
            account_page(&state, &account, Some(typed.to_owned()), None, error).await
        }
    }
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
    let account = Account::current()?;
    let text = lang::text();
    if let Err(error) = check_password(&form.password) {
        return account_page(&state, &account, None, None, Some(error)).await;
    }
    // The password they know must be this account's, as they're logged in to it.
    let verified = state
        .auth
        .verify_password(client, &account.user, form.current)
        .await;
    if verified != Some(PasswordIdentity::Stored(account.credential)) {
        let error = Some(text.error_current_password.clone());
        return account_page(&state, &account, None, None, error).await;
    }
    let hash = match state.auth.hash(form.password).await {
        Ok(hash) => hash,
        Err(err) => return Ok(server_error(err)),
    };
    let changed = admin_user::change_password(&state.db, account.credential, &hash);
    let Some(credential) = changed.await? else {
        return Ok(state.auth.to_login().await);
    };
    let cookies = state
        .auth
        .start_session(&account.user, PasswordIdentity::Stored(credential))
        .await?;
    tracing::info!("own password changed");
    let cookies = cookies
        .into_iter()
        .map(|cookie| (header::SET_COOKIE, cookie));
    let page = redirect(&format!("{}?saved", paths::ACCOUNT));
    Ok((AppendHeaders(cookies), page).into_response())
}
