//! Cross-site request forgery protection without cookies or tokens, in the style of Go's
//! `http.CrossOriginProtection`: browsers say where a request comes from, and we refuse
//! state-changing requests that come from another site.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Rejects unsafe requests that a browser marks as cross-site. `origin` is the site's own
/// origin, used for older browsers that send `Origin` but not `Sec-Fetch-Site`.
pub async fn protect(State(origin): State<Arc<str>>, request: Request, next: Next) -> Response {
    if is_allowed(request.method(), request.headers(), &origin) {
        next.run(request).await
    } else {
        tracing::warn!(method = %request.method(), "cross-origin request rejected");
        (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response()
    }
}

fn is_allowed(method: &Method, headers: &HeaderMap, origin: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        // `none` is a user-initiated navigation, such as a bookmark.
        return matches!(site.as_bytes(), b"same-origin" | b"none");
    }
    // Requests without either header don't come from a browser, so a forged form can't
    // produce them.
    headers
        .get(header::ORIGIN)
        .is_none_or(|value| value.as_bytes() == origin.as_bytes())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const ORIGIN: &str = "https://fest.example.com";

    fn allowed(method: Method, headers: &[(&'static str, &'static str)]) -> bool {
        let headers = headers
            .iter()
            .map(|(name, value)| (name.parse().unwrap(), HeaderValue::from_static(value)))
            .collect();
        is_allowed(&method, &headers, ORIGIN)
    }

    #[test]
    fn safe_methods_always_pass() {
        assert!(allowed(Method::GET, &[("sec-fetch-site", "cross-site")]));
        assert!(allowed(Method::HEAD, &[("origin", "https://evil.example")]));
    }

    #[test]
    fn fetch_metadata_decides_when_present() {
        assert!(allowed(Method::POST, &[("sec-fetch-site", "same-origin")]));
        assert!(allowed(Method::POST, &[("sec-fetch-site", "none")]));
        assert!(!allowed(Method::POST, &[("sec-fetch-site", "same-site")]));
        assert!(!allowed(
            Method::POST,
            &[("sec-fetch-site", "cross-site"), ("origin", ORIGIN)]
        ));
    }

    #[test]
    fn falls_back_to_origin() {
        assert!(allowed(Method::POST, &[("origin", ORIGIN)]));
        assert!(!allowed(
            Method::POST,
            &[("origin", "https://evil.example")]
        ));
        assert!(!allowed(Method::DELETE, &[("origin", "null")]));
        assert!(allowed(Method::POST, &[]));
    }
}
