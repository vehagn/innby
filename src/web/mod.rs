mod csrf;
mod email;
mod join;
mod pages;
mod rate_limit;
mod rsvp;
#[cfg(test)]
pub(crate) mod testing;

pub use pages::{render, render_with, server_error, stylesheet, stylesheet_link};
pub use rate_limit::RateLimiter;

use std::{convert::Infallible, net::SocketAddr, pin::pin, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Router,
    extract::{ConnectInfo, FromRef, FromRequestParts, MatchedPath, State},
    http::{HeaderValue, Request, StatusCode, header, request::Parts},
    middleware,
    routing::{get, post},
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
use tower_http::{
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{config::Config, db, i18n::Lang, paths};

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
    // An invitation-only event: nothing here belongs in a search engine, and crawlers that
    // fetch a page anyway shouldn't follow its links.
    (
        header::HeaderName::from_static("x-robots-tag"),
        "noindex, nofollow",
    ),
];

/// Sent when the site is served over HTTPS, so browsers never fall back to plain HTTP.
const HSTS: [(header::HeaderName, &str); 1] =
    [(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000")];

/// Requests that take longer are cut off, so slow clients can't hold connections open.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The same for a request's headers, which come before `REQUEST_TIMEOUT` starts.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);

/// Pages addressed by a secret link must not be stored by browsers or proxies.
const LINK_HEADERS: [(header::HeaderName, &str); 1] = [(header::CACHE_CONTROL, "no-store")];

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
}

impl AppState {
    /// Counts a request that mails an address someone typed in, and says whether it's
    /// within the limits. The shared one is for attackers with many client addresses:
    /// it caps how much mail the site sends for them.
    fn may_send_mail(&self, client: rate_limit::ClientIp) -> bool {
        self.mail_limiter.check(client) && self.mail_limiter_all.check_all()
    }
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
        .header_read_timeout(HEADER_TIMEOUT);
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

pub(crate) fn router(config: Config, db: PgPool) -> Router {
    let https = config.is_https();
    let state = AppState {
        config: Arc::new(config),
        db,
        // Generous enough for a household sharing one address.
        join_limiter: Arc::new(RateLimiter::new(10, HOUR)),
        mail_limiter: Arc::new(RateLimiter::new(5, HOUR)),
        // About one per guest, so a day's worth of everyone adding an address.
        mail_limiter_all: Arc::new(RateLimiter::new(100, DAY)),
    };
    let links = Router::new()
        .route(paths::JOIN, get(join::show).post(join::join))
        .route(paths::JOIN_GUESTS, get(join::guests))
        .route(paths::RSVP, get(rsvp::show).post(rsvp::answer))
        .route(paths::RSVP_GUESTS, get(rsvp::guests))
        .route(paths::RSVP_NAME, post(rsvp::rename))
        .route(paths::RSVP_LISTING, post(rsvp::listing))
        .route(paths::RSVP_EXPORT, get(rsvp::export))
        .route(paths::RSVP_DELETE, post(rsvp::delete))
        .route(paths::RSVP_EMAIL, post(email::add))
        .route(paths::RSVP_EMAIL_REMOVE, post(email::remove))
        .route(
            paths::CONFIRM_EMAIL,
            get(email::confirm_page).post(email::confirm),
        )
        .route(paths::LOST, get(email::lost_page).post(email::lost));
    let router = Router::new()
        .route(paths::HOME, get(pages::index))
        .route(paths::PRIVACY, get(pages::privacy))
        .route(paths::STYLESHEET, get(pages::stylesheet))
        .route(paths::ROBOTS, get(pages::robots))
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(ready))
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
    use axum::{body::Body, response::Response};
    use sqlx::postgres::PgPoolOptions;

    use super::{testing::body, *};

    /// A pool pointing at a closed port, for tests that don't touch the database.
    fn unreachable_db() -> PgPool {
        PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://innby@127.0.0.1:1/innby")
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
        assert!(html.contains(r#"<span class="title-text">Julebord<"#));
        assert!(html.contains("lørdag 12. desember 2026 kl. 19:00"));
        assert!(html.contains(r#"href="?lang=en""#));
    }

    #[tokio::test]
    async fn index_follows_accept_language() {
        let html = body(get("/", Some("en-GB,en;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="en-GB">"#));
        assert!(html.contains(r#"<span class="title-text">Christmas party<"#));
        let starts_at = crate::config::tests::config().event.starts_at;
        assert!(html.contains(&Lang::EnGb.format_date(starts_at)));
        // No English description configured, so the Norwegian one is shown.
        assert!(html.contains("Velkommen!"));
    }

    #[tokio::test]
    async fn offers_every_language() {
        let html = body(get("/", Some("nn-NO,nb;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="nn">"#));
        assert!(html.contains("laurdag 12. desember 2026 kl. 19:00"));
        // The menu offers the others, each in its own language.
        for (code, tag, badge, name) in [
            ("nb", "nb", "NB", "Bokmål"),
            ("en", "en-GB", "GB", "English (Traditional)"),
            ("en-us", "en-US", "US", "English (Simplified)"),
            ("zh-hans", "zh-Hans", "简", "简体中文"),
            ("zh-hant", "zh-Hant", "繁", "繁體中文"),
        ] {
            let link = format!(
                r#"<a href="?lang={code}" hreflang="{tag}" lang="{tag}"><span class="badge" aria-hidden="true">{badge}</span>{name}</a>"#
            );
            assert!(html.contains(&link), "{link}");
        }
        assert!(!html.contains(">Nynorsk</a>"));

        let html = body(get("/", Some("en-US,en;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="en-US">"#));
        assert!(html.contains("Sat, Dec 12, 2026, 7:00 PM"));

        let html = body(get("/", Some("zh-TW,zh;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="zh-Hant">"#));
        assert!(html.contains("2026年12月12日（週六）19:00"));
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
    async fn turns_crawlers_away() {
        let response = get("/robots.txt", None).await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(body(response).await, "User-agent: *\nDisallow: /\n");
    }

    #[tokio::test]
    async fn serves_stylesheet() {
        let response = get("/static/style.css", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=300"
        );

        // Pages link the current version, which may be kept for good.
        let link = stylesheet_link();
        let html = body(get("/", None).await).await;
        assert!(html.contains(&format!(r#"href="{link}""#)), "{link}");
        let response = get(&link, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn privacy_notice_in_every_language() {
        let html = body(get("/privacy", None).await).await;
        assert!(html.contains("<h1>Personvernerklæring</h1>"));
        assert!(html.contains(&format!("Versjon {PRIVACY_VERSION}")));
        assert!(html.contains("mailto:kari@example.com"));
        assert!(html.contains("slettes 30 dager etter festen"));

        let html = body(get("/privacy?lang=nn", None).await).await;
        assert!(html.contains("<h1>Personvernerklæring</h1>"));
        assert!(html.contains("blir sletta 30 dagar etter festen"));

        let html = body(get("/privacy?lang=en-us", None).await).await;
        assert!(html.contains("<h1>Privacy</h1>"));
        assert!(html.contains("deleted after 7 days"));

        let html = body(get("/privacy?lang=zh-hans", None).await).await;
        assert!(html.contains("<h1>隐私声明</h1>"));
        assert!(html.contains("已发送邮件的记录会在 7 天后删除"));
        let html = body(get("/privacy?lang=zh-hant", None).await).await;
        assert!(html.contains("<h1>隱私聲明</h1>"));

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
