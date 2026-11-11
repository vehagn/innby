use std::io::Read;

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
async fn index_is_about_innby_in_the_base_language() {
    // A browser that asks for no language gets English, the base.
    let html = body(get("/", None).await).await;
    assert!(html.contains(r#"<html lang="en" "#));
    assert!(html.contains(r#"<span class="title-text">Innby<"#));
    assert!(html.contains(&escape(&Lang::EN.messages().site_intro)));
    // No event on the front page: that's on the invitation's.
    assert!(!html.contains("Julebord") && !html.contains("Christmas"));
    assert!(html.contains(r#"href="?lang=nb""#));
}

#[tokio::test]
async fn every_page_has_the_banner_home() {
    // In the page's language and colour scheme.
    let html = body(get("/privacy?lang=nn&theme=dark", None).await).await;
    assert!(
        html.contains(r#"<a class="brand" href="/?lang=nn&#38;theme=dark">Innby</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn index_follows_accept_language() {
    let html = body(get("/", Some("nb-NO,nb;q=0.9")).await).await;
    assert!(html.contains(r#"<html lang="nb" "#));
    assert!(html.contains(&escape(&Lang::NB.messages().site_intro)));
    // The traditional and simplified variants answer British and American browsers.
    let html = body(get("/", Some("en-GB,en;q=0.9")).await).await;
    assert!(html.contains(r#"<html lang="en-GB" "#));
    assert!(html.contains(&escape(&Lang::EN_GB.messages().site_intro)));
    let html = body(get("/", Some("en-AU,en;q=0.9")).await).await;
    assert!(html.contains(r#"<html lang="en" "#));
}

#[tokio::test]
async fn offers_every_language() {
    let html = body(get("/", Some("nn-NO,nb;q=0.9")).await).await;
    assert!(html.contains(r#"<html lang="nn" "#));
    // The menu offers the others, each in its own language.
    for (code, tag, badge, name) in [
        ("nb", "nb", "NB", "Bokmål"),
        ("en", "en", "EN", "English"),
        ("en-gb", "en-GB", "GB", "English (Traditional)"),
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
    let html = body(get("/", Some("nb")).await).await;
    assert!(html.contains(r#"data-theme="auto""#));
    assert!(html.contains(r#"href="/privacy?lang=nb""#));
    assert!(html.contains(r#"href="?lang=nb" title="Som enheten" aria-current="true""#));
    assert!(html.contains(r#"href="?lang=nb&#38;theme=dark" title="Mørkt""#));

    let html = body(get("/?lang=en-gb&theme=dark", None).await).await;
    assert!(html.contains(r#"data-theme="dark""#));
    assert!(html.contains(r#"href="/privacy?lang=en-gb&#38;theme=dark""#));
    assert!(html.contains(r#"href="?lang=nb&#38;theme=dark" hreflang="nb""#));
    assert!(html.contains(r#"href="?lang=en-gb" title="As one&#39;s device prefers">"#));

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

/// What a browser gets for `uri`, asking for gzip.
async fn gzipped(db: PgPool, uri: &str) -> Response {
    let request = Request::get(uri)
        .header(header::ACCEPT_ENCODING, "gzip, deflate, br")
        .body(Body::empty())
        .unwrap();
    router(crate::config::tests::config(), db)
        .oneshot(request)
        .await
        .unwrap()
}

#[tokio::test]
async fn gzips_the_stylesheet_and_the_privacy_notice() {
    for uri in ["/static/style.css", "/privacy?lang=en"] {
        let response = gzipped(unreachable_db(), uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            response.headers()[header::CONTENT_ENCODING],
            "gzip",
            "{uri}"
        );
        assert_eq!(response.headers()[header::VARY], "accept-encoding", "{uri}");
        let packed = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut text = String::new();
        flate2::read::GzDecoder::new(&packed[..])
            .read_to_string(&mut text)
            .unwrap();
        assert!(
            text.len() > packed.len() * 2,
            "{uri}: {} from {}",
            packed.len(),
            text.len()
        );
    }
    // Not for a browser that doesn't ask.
    let response = get("/static/style.css", None).await;
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
}

#[sqlx::test]
async fn never_gzips_a_page_that_holds_a_link(db: PgPool) {
    let site = testing::Site::new(db.clone());
    let (_, link) = site.invite("Kari", 2).await;
    for uri in [
        format!("/r/{link}"),
        format!("/r/{link}/guests"),
        "/".into(),
    ] {
        let response = gzipped(db.clone(), &uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert!(
            response.headers().get(header::CONTENT_ENCODING).is_none(),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn privacy_notice_in_every_language() {
    let html = body(get("/privacy", Some("nb")).await).await;
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

    // English, the base, and so what a browser that asks for no language gets.
    for page in ["/privacy?lang=en", "/privacy"] {
        let html = body(get(page, None).await).await;
        assert!(html.contains("<h1>Privacy notice</h1>"));
        assert!(html.contains("Kari Nordmann hosts the events on this site"));
        assert!(html.contains("deleted after 7 days"));
    }
    let html = body(get("/privacy?lang=en-gb", None).await).await;
    assert!(
        html.contains("what is needed to plan the occasion"),
        "{html}"
    );
}

#[tokio::test]
async fn unknown_route_is_not_found() {
    assert_eq!(get("/nope", None).await.status(), StatusCode::NOT_FOUND);
    // Nor kept: a path under a guest's link has the link in it.
    let response = get("/r/some.link/nope", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}
