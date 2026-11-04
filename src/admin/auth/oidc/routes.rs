//! `/login/oidc`, which sends the browser to the provider, and the callback it comes back
//! to.

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
};
use chrono::{TimeDelta, Utc};

use super::{Callback, FLOW_COOKIE, FLOW_LIFETIME, Flow, LoginError, Provider};
use crate::{
    admin::{
        auth::{self, Auth},
        lang,
    },
    http::cookie,
    i18n::fill,
    paths,
};

/// `/login/oidc`: off to the provider.
pub async fn start(State(auth): State<Arc<Auth>>) -> Response {
    let Some(provider) = auth.oidc() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match provider.start(Utc::now()).await {
        Ok((url, flow)) => {
            let sealed = auth.seal(
                FLOW_COOKIE,
                &serde_json::to_vec(&flow).expect("flows serialize"),
            );
            let cookie = auth.cookie(FLOW_COOKIE, &sealed, FLOW_LIFETIME);
            (
                AppendHeaders([(header::SET_COOKIE, cookie)]),
                Redirect::to(url.as_str()),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!("OIDC login couldn't start: {err:#}");
            failed(&auth, provider)
        }
    }
}

/// `/login/oidc/callback`: back from the provider.
pub async fn callback(
    State(auth): State<Arc<Auth>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(provider) = auth.oidc() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let field = |name: &str| query.get(name).cloned();
    let callback = Callback {
        code: field("code"),
        state: field("state"),
        iss: field("iss"),
        error: field("error"),
        error_description: field("error_description"),
    };
    let clear = auth.cookie(FLOW_COOKIE, "", TimeDelta::zero());
    let flow = cookie::get(&headers, FLOW_COOKIE)
        .and_then(|sealed| auth.open(FLOW_COOKIE, sealed))
        .and_then(|json| serde_json::from_slice::<Flow>(&json).ok());
    let result = match &flow {
        Some(flow) => provider.finish(flow, &callback, Utc::now()).await,
        None => Err(LoginError::Expired),
    };
    let text = lang::text();
    let mut response = match result {
        Ok(login) => {
            let cookie = auth.start_oidc_session(&login);
            tracing::info!(
                user = login.user,
                role = login.role.name(),
                "host logged in through OIDC"
            );
            let home = Redirect::to(&paths::admin(paths::HOME));
            (AppendHeaders([(header::SET_COOKIE, cookie)]), home).into_response()
        }
        Err(LoginError::Expired) => {
            tracing::warn!("OIDC callback without a login in progress, or a stale one");
            auth::login_error(&auth, StatusCode::BAD_REQUEST, text.login_expired.clone())
        }
        Err(LoginError::Cancelled) => auth::login_error(
            &auth,
            StatusCode::UNAUTHORIZED,
            text.login_cancelled.clone(),
        ),
        Err(LoginError::NoRole { user }) => {
            tracing::warn!(user, "OIDC login without an Innby role");
            let message = fill(
                &text.no_role,
                &[("name", &provider.name()), ("user", &user)],
            );
            auth::login_error(&auth, StatusCode::FORBIDDEN, message)
        }
        Err(LoginError::Failed(err)) => {
            tracing::error!("OIDC login failed: {err:#}");
            failed(&auth, provider)
        }
    };
    response.headers_mut().append(header::SET_COOKIE, clear);
    response
}

fn failed(auth: &Auth, provider: &Provider) -> Response {
    let message = fill(&lang::text().login_failed, &[("name", &provider.name())]);
    auth::login_error(auth, StatusCode::BAD_GATEWAY, message)
}
