mod pages;

use std::{convert::Infallible, sync::Arc};

use anyhow::Context;
use axum::{
    Router,
    extract::{FromRequestParts, MatchedPath},
    http::{HeaderValue, Request, header, request::Parts},
    routing::get,
};
use tokio::signal::unix::{SignalKind, signal};
use tower_http::{
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{config::Config, i18n::Lang};

const SECURITY_HEADERS: [(header::HeaderName, &str); 3] = [
    (
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; \
         frame-ancestors 'none'; base-uri 'none'",
    ),
    (header::REFERRER_POLICY, "no-referrer"),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
}

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    axum::serve(listener, router(config))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn router(config: Config) -> Router {
    let state = AppState {
        config: Arc::new(config),
    };
    let router = Router::new()
        .route("/", get(pages::index))
        .route("/static/style.css", get(pages::stylesheet))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
        .with_state(state);

    SECURITY_HEADERS
        .into_iter()
        .fold(router, |router, (name, value)| {
            router.layer(SetResponseHeaderLayer::if_not_present(
                name,
                HeaderValue::from_static(value),
            ))
        })
        .layer(
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

async fn shutdown_signal() {
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
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::Response,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn get(uri: &str, accept_language: Option<&str>) -> Response {
        let mut request = Request::get(uri);
        if let Some(value) = accept_language {
            request = request.header(header::ACCEPT_LANGUAGE, value);
        }
        router(crate::config::tests::config())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_endpoints_respond() {
        for uri in ["/healthz", "/readyz"] {
            assert_eq!(get(uri, None).await.status(), StatusCode::OK);
        }
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
        for (name, value) in SECURITY_HEADERS {
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
    async fn unknown_route_is_not_found() {
        assert_eq!(get("/nope", None).await.status(), StatusCode::NOT_FOUND);
    }
}
