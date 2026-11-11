//! The site's icon, an open door with confetti, for browser tabs, bookmarks and home
//! screens. `static/favicon.svg` is the drawing; `just icons` draws the other two from
//! it. Browsers ask for `/favicon.ico` and iPhones for `/apple-touch-icon.png` whether a
//! page links them or not, so both sites answer at those paths too.

use axum::{
    http::header,
    response::{IntoResponse, Response},
};

/// A day: an icon seldom changes, and its URL doesn't when it does.
const CACHE: &str = "public, max-age=86400";

fn icon(content_type: &'static str, bytes: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE),
        ],
        bytes,
    )
        .into_response()
}

/// The drawing itself, for the browsers that take SVG icons, which is most.
pub async fn svg() -> Response {
    icon("image/svg+xml", include_bytes!("../../static/favicon.svg"))
}

/// For the browsers that don't, and for those that ask before reading the page.
pub async fn ico() -> Response {
    icon(
        "image/vnd.microsoft.icon",
        include_bytes!("../../static/favicon.ico"),
    )
}

/// For an iPhone's home screen.
pub async fn apple_touch() -> Response {
    icon(
        "image/png",
        include_bytes!("../../static/apple-touch-icon.png"),
    )
}
