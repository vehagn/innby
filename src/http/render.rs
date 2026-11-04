//! Pages from templates, and the stylesheet, which every page links with its hash.

use std::{fmt::Display, sync::LazyLock};

use askama::Template;
use axum::{
    extract::RawQuery,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::paths;

const STYLE: &str = include_str!("../../static/style.css");

/// `v=` and a hash of the stylesheet: pages link it with this query, so a changed
/// stylesheet gets a new URL and browsers can keep each one for good.
static STYLE_VERSION: LazyLock<String> =
    LazyLock::new(|| format!("v={}", URL_SAFE_NO_PAD.encode(&Sha256::digest(STYLE)[..6])));

/// The stylesheet's link, for templates.
pub fn stylesheet_link() -> String {
    format!("{}?{}", paths::STYLESHEET, *STYLE_VERSION)
}

pub async fn stylesheet(RawQuery(query): RawQuery) -> impl IntoResponse {
    let cache = if query.as_deref() == Some(STYLE_VERSION.as_str()) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, cache),
        ],
        STYLE,
    )
}

pub fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => server_error(err),
    }
}

pub fn render_with(status: StatusCode, template: impl Template) -> Response {
    let mut response = render(template);
    if response.status().is_success() {
        *response.status_mut() = status;
    }
    response
}

/// Logs the cause and answers 500 without details.
pub fn server_error(err: impl Display) -> Response {
    tracing::error!(error = %err, "request failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}
