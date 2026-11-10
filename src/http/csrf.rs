//! Cross-site request forgery protection without cookies or tokens, in the style of Go's
//! `http.CrossOriginProtection`: browsers say where a request comes from, and we refuse
//! state-changing requests that come from another site.

use axum::{
    extract::Request,
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Rejects unsafe requests that a browser marks as cross-site. The API is for another
/// origin, the games', and a request there needs a token a forged one can't have.
/// `https` is whether the site is served over HTTPS, which its own origin then says.
pub async fn protect(https: bool, request: Request, next: Next) -> Response {
    if request.uri().path().starts_with(crate::paths::API)
        || is_allowed(request.method(), request.headers(), https)
    {
        next.run(request).await
    } else {
        tracing::warn!(method = %request.method(), "cross-origin request rejected");
        (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response()
    }
}

fn is_allowed(method: &Method, headers: &HeaderMap, https: bool) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        // `none` is a user-initiated navigation, such as a bookmark.
        return matches!(site.as_bytes(), b"same-origin" | b"none");
    }
    // Older browsers send only `Origin`: it must be the site's own, the scheme it's
    // served over and the host the request is for. The scheme counts: the same host over
    // plain HTTP is whoever sits between the browser and the site, not the site.
    // Requests without either header don't come from a browser, so a forged form can't
    // produce them.
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let scheme = if https { "https" } else { "http" };
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    let ours = host.map(|host| format!("{scheme}://{host}"));
    origin
        .to_str()
        .is_ok_and(|origin| Some(origin) == ours.as_deref())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const ORIGIN: &str = "https://fest.example.com";

    /// Checks a request for `fest.example.com` with the given extra headers.
    fn allowed(method: Method, headers: &[(&'static str, &'static str)]) -> bool {
        let headers = std::iter::once(("host", "fest.example.com"))
            .chain(headers.iter().copied())
            .map(|(name, value)| (name.parse().unwrap(), HeaderValue::from_static(value)))
            .collect();
        is_allowed(&method, &headers, true)
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
        assert!(!allowed(
            Method::POST,
            &[("origin", "http://fest.example.com.evil")]
        ));
        assert!(allowed(Method::POST, &[]));
    }

    #[test]
    fn the_origin_is_the_scheme_too() {
        // The site over plain HTTP isn't the site: it's anyone on the network.
        assert!(!allowed(
            Method::POST,
            &[("origin", "http://fest.example.com")]
        ));
        let headers = [
            ("host", "localhost:8080"),
            ("origin", "http://localhost:8080"),
        ]
        .map(|(name, value)| (name.parse().unwrap(), HeaderValue::from_static(value)))
        .into_iter()
        .collect();
        assert!(is_allowed(&Method::POST, &headers, false));
        assert!(!is_allowed(&Method::POST, &headers, true));
    }
}
