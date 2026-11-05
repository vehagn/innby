//! `innby admin`: the host's UI for events, their invitations, group links and
//! headcount, in the site's languages. Served on its own listener, at the root of its
//! own hostname or under a path of the site's (`INNBY_ADMIN_URL`), behind a login.
//!
//! - `mod.rs`: `router`, which lists every page and the role it needs. Handlers are
//!   named for what they do: `list`, `show`, `create`, `edit`, `revoke`, `delete`.
//! - `page`: what pages share: state, tabs, errors. `forms`: what their forms share.
//! - `events`, `guests`, `invitations`, `groups`, `qr`, `games`, `access`: the pages,
//!   one module per tab.
//! - `cli`: the host's commands: `innby invite` and `innby event`.
//! - `auth`: logging in, with a password or through an OIDC provider, and sessions.
//! - `role`: what each role may do, for every event or for one. `lang`: the page's
//!   language.

mod access;
pub mod auth;
pub mod cli;
mod events;
mod forms;
mod games;
mod groups;
mod guests;
mod invitations;
pub mod lang;
mod page;
mod qr;
pub mod role;
#[cfg(test)]
pub(crate) mod testing;

pub use page::{Tab, tabs};

use std::sync::Arc;

use axum::{
    Router, middleware,
    response::Redirect,
    routing::{get, post},
};
use sqlx::PgPool;

use crate::{
    config::{AdminConfig, Config},
    http, paths,
};
use page::AdminState;

pub async fn serve(config: Config, admin: AdminConfig, db: PgPool) -> anyhow::Result<()> {
    let address = admin.listen;
    http::listen(address, router(config, &admin, db)).await
}

tokio::task_local! {
    static PREFIX: Arc<str>;
    static GAMES: bool;
}

/// Whether the config lists any minigames, for the Games tab.
pub fn has_games() -> bool {
    GAMES.try_with(|games| *games).unwrap_or(false)
}

/// The path the admin UI is under, such as `/admin`, or empty at the root of its own
/// hostname. Links read it from here, as they read the language.
pub fn prefix() -> Arc<str> {
    PREFIX
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::from(""))
}

pub(crate) fn router(config: Config, admin_config: &AdminConfig, db: PgPool) -> Router {
    // The admin hostname is served the same way as the public one.
    let https = config.is_https();
    let auth = Arc::new(auth::Auth::new(admin_config, https, db.clone()));
    let state = AdminState {
        config: Arc::new(config),
        db,
        auth: Arc::clone(&auth),
    };
    // Each path needs one role for all its methods; see `role` for what each may do.
    // The list of events is for anyone logged in, and shows each the events they may see.
    let anyone = Router::new().route(paths::HOME, get(events::list));
    let viewer = Router::new()
        .route(paths::EVENT, get(guests::list))
        .route_layer(middleware::from_fn(role::viewer));
    let editor = Router::new()
        .route(
            paths::EVENT_INVITATIONS,
            get(invitations::list).post(invitations::create),
        )
        .route(paths::EVENT_GROUPS, get(groups::list).post(groups::create))
        .route(
            paths::EVENT_DETAILS,
            get(events::details::show).post(events::details::edit),
        )
        .route(paths::EVENT_CSV, get(guests::csv))
        .route(paths::EVENT_EXPORT, get(events::details::export))
        .route(paths::EVENT_REMINDERS, post(guests::remind))
        .route(paths::GROUP, get(groups::show).post(groups::edit))
        .route(paths::GROUP_REVOKE, post(groups::revoke))
        .route(paths::GROUP_DELETE, post(groups::delete))
        .route(paths::GUEST, get(guests::show).post(guests::edit))
        .route(paths::GUEST_REVOKE, post(guests::revoke))
        .route(paths::GUEST_DELETE, post(guests::delete))
        .route(paths::GUEST_QR, get(guests::qr))
        .route(paths::GROUP_QR, get(groups::qr))
        .route(paths::EVENT_QR, get(qr::show).post(qr::save))
        .route(paths::EVENT_GAMES, get(games::show).post(games::edit))
        .route_layer(middleware::from_fn(role::editor));
    let admin = Router::new()
        .route(paths::EVENTS, post(events::create))
        .route(paths::EVENTS_IMPORT, post(events::import))
        .route(paths::EVENT_DELETE, post(events::details::delete))
        .route(paths::EVENT_ACCESS, get(access::list).post(access::create))
        .route(paths::EVENT_ACCESS_REMOVE, post(access::delete))
        .route_layer(middleware::from_fn(role::admin));
    let mut pages = anyone
        .merge(viewer)
        .merge(editor)
        .merge(admin)
        // On an event's pages, before the checks above, the role it gives the host.
        .route_layer(middleware::from_fn_with_state(
            state.db.clone(),
            role::for_event,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth),
            auth::require_login,
        ))
        .route(paths::LOGOUT, post(auth::log_out));
    pages = if !admin_config.users.is_empty() {
        pages.route(paths::LOGIN, get(auth::login_page).post(auth::log_in))
    } else {
        pages.route(paths::LOGIN, get(auth::login_page))
    };
    if admin_config.oidc.is_some() {
        pages = pages
            .route(paths::LOGIN_OIDC, get(auth::oidc::start))
            .route(paths::LOGIN_OIDC_CALLBACK, get(auth::oidc::callback));
    }
    let prefix: Arc<str> = Arc::from(admin_config.path.as_str());
    let games = !state.config.games.list.is_empty();
    let scoped = Arc::clone(&prefix);
    let router: Router = http::no_store(pages)
        // Probes don't log in, and the login page has its style before anyone does.
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(http::ready))
        .route(paths::STYLESHEET, get(http::stylesheet))
        .layer(middleware::from_fn(crate::theme::scope))
        .layer(middleware::from_fn(lang::scope))
        .layer(middleware::from_fn(
            move |request, next: middleware::Next| {
                PREFIX.scope(Arc::clone(&scoped), GAMES.scope(games, next.run(request)))
            },
        ))
        .with_state(state.clone());
    let router = if prefix.is_empty() {
        router
    } else {
        // Under a path of the site's hostname, as `/admin`. The probes stay at the
        // root, where the kubelet asks.
        let home = paths::admin_under(&prefix, paths::HOME);
        Router::<AdminState>::new()
            .route(paths::HEALTH, get(|| async { "ok" }))
            .route(paths::READY, get(http::ready))
            .with_state(state)
            .nest(&prefix, router)
            .route(
                &format!("{prefix}/"),
                get(move || async move { Redirect::permanent(&home) }),
            )
    };
    http::harden(router, https)
}

#[cfg(test)]
mod tests;
