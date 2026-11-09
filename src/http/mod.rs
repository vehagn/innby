//! What both listeners share, the public site's and the admin UI's: serving with
//! timeouts and graceful shutdown, security headers, CSRF protection, rate limits,
//! cookies, rendering templates and the stylesheet, gzip where it's safe, and the
//! request's language.

mod compress;
pub mod cookie;
mod csrf;
mod rate_limit;
mod render;

pub use rate_limit::{ClientIp, RateLimiter};
pub use render::{
    render, render_compressed, render_form, render_with, server_error, stylesheet, stylesheet_link,
};

use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::pin,
    time::{Duration, Instant},
};

use anyhow::Context;
use axum::{
    Router,
    extract::{ConnectInfo, FromRequestParts, MatchedPath, State},
    http::{HeaderValue, Request, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::Response,
};
use hyper::{body::Incoming, server::conn::http1};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::graceful::GracefulShutdown,
    service::TowerToHyperService,
};
use sqlx::PgPool;
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};
use tower::ServiceExt;
use tower_http::{set_header::SetResponseHeaderLayer, timeout::TimeoutLayer, trace::TraceLayer};

use crate::{db, i18n::Lang};

pub(crate) const SECURITY_HEADERS: [(header::HeaderName, &str); 6] = [
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
    // An invitation-only event: nothing here belongs in a search engine, and crawlers that
    // fetch a page anyway shouldn't follow its links.
    (
        header::HeaderName::from_static("x-robots-tag"),
        "noindex, nofollow",
    ),
];

/// Sent when the site is served over HTTPS, so browsers never fall back to plain HTTP.
pub(crate) const HSTS: [(header::HeaderName, &str); 1] =
    [(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000")];

/// Requests that take longer are cut off, so slow clients can't hold connections open.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The same for a request's headers, which come before `REQUEST_TIMEOUT` starts.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// The most a request's line and headers may take, rather than hyper's 400 KiB: plenty
/// for the few cookies Innby sets, and little memory per slow or hostile connection.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Pages addressed by a secret link must not be stored by browsers or proxies.
const LINK_HEADERS: [(header::HeaderName, &str); 1] = [(header::CACHE_CONTROL, "no-store")];

/// Serves the app until SIGTERM or Ctrl-C, finishing requests in flight.
///
/// Not `axum::serve`: it gives hyper no timer, and without one hyper doesn't enforce
/// `HEADER_TIMEOUT`.
pub async fn listen(address: SocketAddr, app: Router) -> anyhow::Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("binding {address}"))?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(MAX_HEADER_BYTES);
    let connections = GracefulShutdown::new();
    let mut shutdown = pin!(shutdown_signal());
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(err) => {
                    // Usually out of file descriptors: give connections time to close.
                    tracing::warn!(error = %err, "accepting a connection failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let app = app
            .clone()
            .map_request(move |mut request: Request<Incoming>| {
                request.extensions_mut().insert(ConnectInfo(peer));
                request
            });
        let connection = connections
            .watch(http.serve_connection(TokioIo::new(stream), TowerToHyperService::new(app)));
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::debug!(error = %err, "connection closed");
            }
        });
    }
    connections.shutdown().await;
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

/// What every listener gets: CSRF protection, security headers, a request timeout and
/// request logging (see `log_response`). `https` adds HSTS.
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
    router.layer(middleware::from_fn(log_response)).layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request<_>| {
                // Log the route pattern, never the raw path: RSVP links carry secrets.
                let route = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map_or("unmatched", MatchedPath::as_str);
                tracing::info_span!("request", method = %request.method(), route)
            })
            // `log_response` logs the answer.
            .on_response(()),
    )
}

/// Logs each answer, in the request's span, as tower-http's default does, but the
/// kubelet's probes at debug: they come every few seconds, and buried the requests that
/// matter. A probe that fails is logged at info all the same. `RUST_LOG=info,innby::http=debug`
/// logs them all.
async fn log_response(request: axum::extract::Request, next: Next) -> Response {
    let probe = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|route| is_probe(route.as_str()));
    let started = Instant::now();
    let response = next.run(request).await;
    let latency = format!("{} ms", started.elapsed().as_millis());
    let status = response.status().as_u16();
    if probe && response.status().is_success() {
        tracing::debug!(%latency, status, "finished processing request");
    } else {
        tracing::info!(%latency, status, "finished processing request");
    }
    response
}

/// Whether a route is a probe, at the root or under the admin UI's path.
fn is_probe(route: &str) -> bool {
    route.ends_with(crate::paths::HEALTH) || route.ends_with(crate::paths::READY)
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

pub async fn shutdown_signal() {
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
    use std::sync::{Arc, Mutex};

    use axum::{body::Body, routing::get};
    use tower::ServiceExt;

    use super::*;

    /// What the logs say, at info.
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn probes_are_logged_only_when_they_fail() {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _logging = tracing::subscriber::set_default(subscriber);
        let router = harden(
            Router::new()
                .route(crate::paths::HEALTH, get(|| async { "ok" }))
                .route(
                    crate::paths::READY,
                    get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
                )
                .route(crate::paths::PRIVACY, get(|| async { "notice" })),
            false,
        );
        for path in [
            crate::paths::HEALTH,
            crate::paths::READY,
            crate::paths::PRIVACY,
        ] {
            let request = Request::get(path).body(Body::empty()).unwrap();
            router.clone().oneshot(request).await.unwrap();
        }
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(!logs.contains("/healthz"), "{logs}");
        assert!(
            logs.contains("/readyz") && logs.contains("status=503"),
            "{logs}"
        );
        assert!(
            logs.contains("/privacy") && logs.contains("status=200"),
            "{logs}"
        );
    }

    #[test]
    fn knows_the_probes_under_a_path_too() {
        assert!(is_probe("/healthz") && is_probe("/admin/readyz"));
        assert!(!is_probe("/r/{link}") && !is_probe("/healthz/extra"));
    }
}
