mod csrf;
mod email;
mod join;
mod pages;
mod rate_limit;
mod rsvp;
#[cfg(test)]
mod testing;

pub use pages::{render, render_with, server_error, stylesheet};

use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Router,
    extract::{FromRef, FromRequestParts, MatchedPath, State},
    http::{HeaderValue, Request, StatusCode, header, request::Parts},
    middleware,
    routing::{get, post},
};
use sqlx::PgPool;
use tokio::signal::unix::{SignalKind, signal};
use tower_http::{
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{config::Config, db, i18n::Lang};

/// Version of the privacy notice, recorded with each answer.
/// Bump it whenever the notice in `templates/privacy/` changes in substance.
pub const PRIVACY_VERSION: &str = "2026-11-02";

const SECURITY_HEADERS: [(header::HeaderName, &str); 6] = [
    (
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; \
         frame-ancestors 'none'; base-uri 'none'",
    ),
    (header::REFERRER_POLICY, "no-referrer"),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    // No other site may hold a window handle to ours or embed its responses.
    (
        header::HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    ),
    (
        header::HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    ),
    // An invitation-only event: nothing here belongs in a search engine.
    (header::HeaderName::from_static("x-robots-tag"), "noindex"),
];

/// Sent when the site is served over HTTPS, so browsers never fall back to plain HTTP.
const HSTS: [(header::HeaderName, &str); 1] =
    [(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000")];

/// Requests that take longer are cut off, so slow clients can't hold connections open.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Pages addressed by a secret link must not be stored by browsers or proxies.
const LINK_HEADERS: [(header::HeaderName, &str); 1] = [(header::CACHE_CONTROL, "no-store")];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    db: PgPool,
    join_limiter: Arc<rate_limit::RateLimiter>,
    /// Requests that send mail to an address someone typed in.
    mail_limiter: Arc<rate_limit::RateLimiter>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

pub async fn serve(config: Config, db: PgPool) -> anyhow::Result<()> {
    let address = config.listen;
    listen(address, router(config, db)).await
}

/// Serves the app until SIGTERM or Ctrl-C, finishing requests in flight.
pub async fn listen(address: SocketAddr, app: Router) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("binding {address}"))?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

fn with_headers<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    headers: &[(header::HeaderName, &'static str)],
) -> Router<S> {
    headers.iter().fold(router, |router, (name, value)| {
        router.layer(SetResponseHeaderLayer::overriding(
            name.clone(),
            HeaderValue::from_static(value),
        ))
    })
}

/// For pages that show secret links: browsers and proxies must not keep them.
pub fn no_store<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    with_headers(router, &LINK_HEADERS)
}

pub(crate) fn router(config: Config, db: PgPool) -> Router {
    let https = config.base_url.starts_with("https://");
    let state = AppState {
        config: Arc::new(config),
        db,
        // Generous enough for a household sharing one address.
        join_limiter: Arc::new(rate_limit::RateLimiter::new(
            10,
            Duration::from_secs(60 * 60),
        )),
        mail_limiter: Arc::new(rate_limit::RateLimiter::new(
            5,
            Duration::from_secs(60 * 60),
        )),
    };
    let links = Router::new()
        .route("/join/{code}", get(join::show).post(join::join))
        .route("/r/{link}", get(rsvp::show).post(rsvp::answer))
        .route("/r/{link}/name", post(rsvp::rename))
        .route("/r/{link}/export", get(rsvp::export))
        .route("/r/{link}/delete", post(rsvp::delete))
        .route("/r/{link}/email", post(email::add))
        .route("/r/{link}/email/remove", post(email::remove))
        .route("/e/{token}", get(email::confirm_page).post(email::confirm))
        .route("/lost", get(email::lost_page).post(email::lost));
    let router = Router::new()
        .route("/", get(pages::index))
        .route("/privacy", get(pages::privacy))
        .route("/static/style.css", get(pages::stylesheet))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .merge(no_store(links))
        .fallback(pages::not_found)
        .with_state(state);
    harden(router, https)
}

/// What every listener gets: CSRF protection, security headers, a request timeout and
/// request logging. `https` adds HSTS.
pub fn harden(router: Router, https: bool) -> Router {
    let router =
        router
            .layer(middleware::from_fn(csrf::protect))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                REQUEST_TIMEOUT,
            ));
    let router = with_headers(router, &SECURITY_HEADERS);
    let router = if https {
        with_headers(router, &HSTS)
    } else {
        router
    };
    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request<_>| {
                // Log the route pattern, never the raw path: RSVP links carry secrets.
                let route = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map_or("unmatched", MatchedPath::as_str);
                tracing::info_span!("request", method = %request.method(), route)
            })
            .on_response(DefaultOnResponse::new().level(Level::INFO)),
    )
}

/// Ready to take traffic only while the database answers.
pub async fn ready(State(db): State<PgPool>) -> StatusCode {
    match db::ping(&db).await {
        Ok(()) => StatusCode::OK,
        Err(err) => {
            tracing::warn!(error = %err, "database unavailable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub(crate) async fn shutdown_signal() {
    let mut terminate = signal(SignalKind::terminate()).expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down");
}

/// The page language: an explicit `?lang=` wins over the browser's `Accept-Language`.
impl<S: Send + Sync> FromRequestParts<S> for Lang {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let from_query = parts
            .uri
            .query()
            .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("lang=")))
            .and_then(Lang::from_code);
        let from_header = || {
            parts
                .headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|value| value.to_str().ok())
                .and_then(Lang::negotiate)
        };
        Ok(from_query.or_else(from_header).unwrap_or(Lang::DEFAULT))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{body::Body, http::Request, response::Response};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    /// A pool pointing at a closed port, for tests that don't touch the database.
    fn unreachable_db() -> PgPool {
        PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://fest@127.0.0.1:1/fest")
            .unwrap()
    }

    async fn request(db: PgPool, uri: &str, accept_language: Option<&str>) -> Response {
        let mut request = Request::get(uri);
        if let Some(value) = accept_language {
            request = request.header(header::ACCEPT_LANGUAGE, value);
        }
        router(crate::config::tests::config(), db)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn get(uri: &str, accept_language: Option<&str>) -> Response {
        request(unreachable_db(), uri, accept_language).await
    }

    async fn body(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn healthy_without_database() {
        assert_eq!(get("/healthz", None).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn not_ready_without_database() {
        assert_eq!(
            get("/readyz", None).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[sqlx::test]
    async fn ready_with_database(pool: PgPool) {
        assert_eq!(
            request(pool, "/readyz", None).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn index_defaults_to_norwegian() {
        let html = body(get("/", None).await).await;
        assert!(html.contains(r#"<html lang="nb">"#));
        assert!(html.contains("<h1>Julebord</h1>"));
        assert!(html.contains("lørdag 12. desember 2026 kl. 19:00"));
        assert!(html.contains(r#"href="?lang=en""#));
    }

    #[tokio::test]
    async fn index_follows_accept_language() {
        let html = body(get("/", Some("en-GB,en;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="en">"#));
        assert!(html.contains("<h1>Christmas party</h1>"));
        assert!(html.contains("Saturday 12 December 2026, 19:00"));
        // No English description configured, so the Norwegian one is shown.
        assert!(html.contains("Velkommen!"));
    }

    #[tokio::test]
    async fn query_parameter_overrides_accept_language() {
        let html = body(get("/?lang=nb", Some("en")).await).await;
        assert!(html.contains(r#"<html lang="nb">"#));
    }

    #[tokio::test]
    async fn responses_carry_security_headers() {
        let response = get("/", None).await;
        for (name, value) in SECURITY_HEADERS.into_iter().chain(HSTS) {
            assert_eq!(response.headers()[&name], value, "{name}");
        }
    }

    #[tokio::test]
    async fn serves_stylesheet() {
        let response = get("/static/style.css", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn privacy_notice_in_both_languages() {
        let html = body(get("/privacy", None).await).await;
        assert!(html.contains("<h1>Personvernerklæring</h1>"));
        assert!(html.contains(&format!("Versjon {PRIVACY_VERSION}")));
        assert!(html.contains("mailto:kari@example.com"));
        assert!(html.contains("slettes 30 dager etter festen"));

        let html = body(get("/privacy?lang=en", None).await).await;
        assert!(html.contains("<h1>Privacy notice</h1>"));
        assert!(html.contains("Kari Nordmann is hosting the event"));
        assert!(html.contains("deleted after 7 days"));
    }

    #[tokio::test]
    async fn unknown_route_is_not_found() {
        assert_eq!(get("/nope", None).await.status(), StatusCode::NOT_FOUND);
    }
}
