//! The public site: the front page, guests' personal and group link pages, e-mail, the
//! event as a calendar file, the privacy notice. `router` lists every route; the shared plumbing is in `crate::http`.

mod calendar;
mod email;
mod games;
mod join;
mod pages;
mod remember;
mod rsvp;
#[cfg(test)]
pub(crate) mod testing;
mod views;
mod you;

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{FromRef, FromRequestParts},
    middleware,
    routing::{get, post},
};
use sqlx::PgPool;

use crate::{
    config::Config,
    http::{self, ClientIp, RateLimiter},
    paths,
};

/// Version of the privacy notice, recorded with each answer.
/// Bump it whenever the notice in `templates/privacy/` changes in substance.
pub const PRIVACY_VERSION: &str = "2026-11-09";

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    db: PgPool,
    join_limiter: Arc<RateLimiter>,
    /// Requests that send mail to an address someone typed in, per client and for all
    /// clients together.
    mail_limiter: Arc<RateLimiter>,
    mail_limiter_all: Arc<RateLimiter>,
    /// Scores from the minigames, per client: a game sends one a round.
    score_limiter: Arc<RateLimiter>,
}

impl AppState {
    /// Counts a request that mails an address someone typed in, and says whether it's
    /// within the limits. The shared one is for attackers with many client addresses:
    /// it caps how much mail the site sends for them.
    fn may_send_mail(&self, client: ClientIp) -> bool {
        self.mail_limiter.check(client) && self.mail_limiter_all.check_all()
    }
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

/// The client, behind the gateway if the config trusts it, for the rate limits.
impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self::from_parts(parts, state.config.trust_forwarded))
    }
}

/// Serves the site, and the internal API for games' servers on a port of its own when a
/// game has a server.
pub async fn serve(config: Config, db: PgPool) -> anyhow::Result<()> {
    let (address, internal_address) = (config.listen, config.internal_listen);
    match routers(config, db) {
        (site, None) => http::listen(address, site).await,
        (site, Some(internal)) => tokio::try_join!(
            http::listen(address, site),
            http::listen(internal_address, internal)
        )
        .map(drop),
    }
}

/// The site alone, for tests that don't need the internal API.
#[cfg(test)]
pub(crate) fn router(config: Config, db: PgPool) -> Router {
    routers(config, db).0
}

/// The site, and the internal API when a game has a server of its own.
pub(crate) fn routers(config: Config, db: PgPool) -> (Router, Option<Router>) {
    let https = config.is_https();
    let state = AppState {
        config: Arc::new(config),
        db,
        // Generous enough for a household sharing one address.
        join_limiter: Arc::new(RateLimiter::new(10, HOUR)),
        mail_limiter: Arc::new(RateLimiter::new(5, HOUR)),
        // About one per guest, so a day's worth of everyone adding an address.
        mail_limiter_all: Arc::new(RateLimiter::new(100, DAY)),
        // A round every six seconds for an hour, for a household on one address.
        score_limiter: Arc::new(RateLimiter::new(600, HOUR)),
    };
    let api = games::router(&state);
    // Not on any route: only the games' servers reach it, inside the cluster.
    let internal = games::internal_router(&state)
        .map(|internal| http::harden(internal.with_state(state.clone()), false));
    let links = Router::new()
        .route(paths::JOIN, get(join::show).post(join::join))
        .route(paths::JOIN_GUESTS, get(join::guests))
        .route(paths::RSVP, get(rsvp::show).post(rsvp::answer))
        .route(paths::RSVP_GUESTS, get(rsvp::guests))
        .route(paths::RSVP_GAMES, get(games::list))
        .route(paths::RSVP_YOU, get(you::show))
        .route(paths::RSVP_NAME, post(you::rename))
        .route(paths::RSVP_LISTING, post(rsvp::listing))
        .route(paths::RSVP_EXPORT, get(you::export))
        .route(paths::RSVP_CALENDAR, get(calendar::show))
        .route(paths::RSVP_DELETE, post(you::delete))
        .route(paths::RSVP_REMEMBER, post(remember::remember))
        .route(paths::RSVP_FORGET, post(remember::forget_here))
        .route(paths::FORGET, post(remember::forget))
        // Greets a guest this device remembers, so no cache may keep it.
        .route(paths::HOME, get(pages::index))
        .route(paths::RSVP_EMAIL, post(email::add))
        .route(paths::RSVP_EMAIL_REMOVE, post(email::remove))
        .route(
            paths::CONFIRM_EMAIL,
            get(email::confirm_page).post(email::confirm),
        )
        .route(paths::LOST, get(email::lost_page).post(email::lost));
    let router = Router::new()
        .route(paths::PRIVACY, get(pages::privacy))
        .route(paths::STYLESHEET, get(http::stylesheet))
        .route(paths::ROBOTS, get(pages::robots))
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(http::ready))
        .merge(http::no_store(links))
        .merge(http::no_store(api.unwrap_or_default()))
        .fallback(pages::not_found)
        .layer(middleware::from_fn(crate::theme::scope))
        .with_state(state);
    (http::harden(router, https), internal)
}

#[cfg(test)]
mod tests;
