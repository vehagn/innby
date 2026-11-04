//! Helpers for tests of either listener: sending forms and reading what comes back.

use axum::{
    body::Body,
    http::{Request, header},
    response::Response,
};
use http_body_util::BodyExt;

/// A form post from this `Sec-Fetch-Site`, as a browser sends it.
pub fn form_post(uri: &str, form: &str, fetch_site: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("sec-fetch-site", fetch_site)
        .body(Body::from(form.to_owned()))
        .unwrap()
}

pub async fn body(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Where a redirect goes.
pub fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

/// Every `Set-Cookie` header of the response.
pub fn set_cookies(response: &Response) -> Vec<&str> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect()
}

/// A text as a template writes it into HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&#38;")
        .replace('<', "&#60;")
        .replace('>', "&#62;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}
