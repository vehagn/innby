//! The public site: the front page, guests' personal and group link pages, e-mail, the
//! privacy notice. `router` lists every route; the shared plumbing is in `crate::http`.

mod email;
mod join;
mod pages;
mod remember;
mod rsvp;
#[cfg(test)]
pub(crate) mod testing;
mod views;

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
pub const PRIVACY_VERSION: &str = "2026-11-04";

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

pub async fn serve(config: Config, db: PgPool) -> anyhow::Result<()> {
    let address = config.listen;
    http::listen(address, router(config, db)).await
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
        .fallback(pages::not_found)
        .layer(middleware::from_fn(crate::theme::scope))
        .with_state(state);
    http::harden(router, https)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, response::Response};

    use axum::http::{Request, StatusCode, header};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::{
        testing::{body, escape},
        *,
    };
    use crate::{
        http::{HSTS, SECURITY_HEADERS, stylesheet_link},
        i18n::Lang,
    };

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
    async fn index_is_about_innby_in_norwegian() {
        let html = body(get("/", None).await).await;
        assert!(html.contains(r#"<html lang="nb" "#));
        assert!(html.contains(r#"<span class="title-text">Innby<"#));
        assert!(html.contains(&escape(&Lang::Nb.messages().site_intro)));
        // No event on the front page: that's on the invitation's.
        assert!(!html.contains("Julebord"));
        assert!(html.contains(r#"href="?lang=en""#));
    }

    #[tokio::test]
    async fn index_follows_accept_language() {
        let html = body(get("/", Some("en-GB,en;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="en-GB" "#));
        assert!(html.contains(&escape(&Lang::EnGb.messages().site_intro)));
    }

    #[tokio::test]
    async fn offers_every_language() {
        let html = body(get("/", Some("nn-NO,nb;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="nn" "#));
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
        assert!(html.contains(r#"<html lang="en-US" "#));

        let html = body(get("/", Some("zh-TW,zh;q=0.9")).await).await;
        assert!(html.contains(r#"<html lang="zh-Hant" "#));
    }

    #[tokio::test]
    async fn query_parameter_overrides_accept_language() {
        let html = body(get("/?lang=nb", Some("en")).await).await;
        assert!(html.contains(r#"<html lang="nb" "#));
    }

    #[tokio::test]
    async fn links_carry_the_colour_scheme() {
        let html = body(get("/", None).await).await;
        assert!(html.contains(r#"data-theme="auto""#));
        assert!(html.contains(r#"href="/privacy?lang=nb""#));
        assert!(html.contains(r#"href="?lang=nb" title="Som enheten" aria-current="true""#));
        assert!(html.contains(r#"href="?lang=nb&#38;theme=dark" title="Mørkt""#));

        let html = body(get("/?lang=en&theme=dark", None).await).await;
        assert!(html.contains(r#"data-theme="dark""#));
        assert!(html.contains(r#"href="/privacy?lang=en&#38;theme=dark""#));
        assert!(html.contains(r#"href="?lang=nb&#38;theme=dark" hreflang="nb""#));
        assert!(html.contains(r#"href="?lang=en" title="As one&#39;s device prefers">"#));

        let html = body(get("/?theme=purple", None).await).await;
        assert!(html.contains(r#"data-theme="auto""#));
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
        assert!(html.contains("Kari Nordmann hosts the events on this site"));
        assert!(html.contains("deleted after 7 days"));
    }

    #[tokio::test]
    async fn unknown_route_is_not_found() {
        assert_eq!(get("/nope", None).await.status(), StatusCode::NOT_FOUND);
    }
}
