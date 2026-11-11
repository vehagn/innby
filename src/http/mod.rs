//! What both listeners share, the public site's and the admin UI's: serving with
//! timeouts and graceful shutdown, security headers, CSRF protection, rate limits,
//! cookies, rendering templates and the stylesheet, the icon, gzip where it's safe, and
//! the request's language.

mod compress;
pub mod cookie;
mod csrf;
pub mod icon;
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

use crate::{
    db,
    i18n::{Asked, Lang, Offer},
};

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
    let router = router
        .layer(middleware::from_fn(move |request, next| {
            csrf::protect(https, request, next)
        }))
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
    if quiet(probe, response.status()) {
        tracing::debug!(%latency, status, "finished processing request");
    } else {
        tracing::info!(%latency, status, "finished processing request");
    }
    response
}

/// Whether an answer is logged at debug rather than info: a probe's that succeeded.
fn quiet(probe: bool, status: StatusCode) -> bool {
    probe && status.is_success()
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

/// What the request asks for in the way of language, for a page that must find its
/// event before it knows which languages it's offered in.
impl<S: Send + Sync> FromRequestParts<S> for Asked {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Asked::from_parts(parts))
    }
}

/// The page language, of those the request is offered (`i18n::Offer`): an explicit
/// `?lang=` wins over the browser's `Accept-Language`. On an event's pages the event
/// narrows the offer once it's known, so those go by `web::rsvp::Guest::lang` instead.
impl<S: Send + Sync> FromRequestParts<S> for Lang {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Asked::from_parts(parts).among(Offer::current()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Which answers are quiet, rather than the log lines themselves: tracing remembers
    // per call site whether anyone listens, and tests in other threads change that, so a
    // test that reads the logs fails now and then.
    #[test]
    fn probes_are_logged_only_when_they_fail() {
        assert!(quiet(true, StatusCode::OK));
        assert!(!quiet(true, StatusCode::SERVICE_UNAVAILABLE));
        assert!(!quiet(false, StatusCode::OK));
        assert!(!quiet(false, StatusCode::NOT_FOUND));
    }

    #[test]
    fn knows_the_probes_under_a_path_too() {
        assert!(is_probe("/healthz") && is_probe("/admin/readyz"));
        assert!(!is_probe("/r/{link}") && !is_probe("/healthz/extra"));
    }
}
