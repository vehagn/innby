//! Cookies: reading the ones a request sent, and setting new ones. Every cookie Innby
//! sets is `HttpOnly` and `SameSite=Lax`, and `Secure` over HTTPS.

use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};

/// Each `name=value` pair in the request's `Cookie` headers.
pub fn all(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
}

/// The value of the cookie with this name, if the request sent one.
pub fn get<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    all(headers).find_map(|(cookie, value)| (cookie == name).then_some(value))
}

/// A `Set-Cookie` value for the paths under `path`, kept for `max_age` seconds; zero
/// drops it.
pub fn set(name: &str, value: &str, path: &str, max_age: i64, https: bool) -> HeaderValue {
    let secure = if https { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}")
        .try_into()
        .expect("cookie names and values are plain ASCII")
}

/// The response, with a cookie set.
pub fn with(mut response: Response, cookie: HeaderValue) -> Response {
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_cookie_header() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            "theme=dark; refresh_token=r".parse().unwrap(),
        );
        headers.append(header::COOKIE, "token=t".parse().unwrap());
        assert_eq!(get(&headers, "token"), Some("t"));
        assert_eq!(get(&headers, "refresh_token"), Some("r"));
        assert_eq!(get(&headers, "them"), None);
        assert_eq!(all(&headers).count(), 3);
    }

    #[test]
    fn sets_http_only_cookies_secure_over_https() {
        assert_eq!(
            set("token", "t", "/admin", 60, true),
            "token=t; Path=/admin; Max-Age=60; HttpOnly; SameSite=Lax; Secure"
        );
        assert_eq!(
            set("token", "", "/", 0, false),
            "token=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
        );
    }
}
