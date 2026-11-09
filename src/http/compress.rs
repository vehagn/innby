//! Gzip, for the responses that are safe to compress: the stylesheet and the privacy
//! notice, which hold no secret and nothing a visitor put there.
//!
//! Every other page holds a link, which is a guest's or a group's key, and many also show
//! what someone typed, such as a name signed up through a group link. Compressing both
//! together lets whoever can choose that text and watch the response's size guess the
//! link a few characters at a time (BREACH). Those pages stay uncompressed.

use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    io::Write,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use flate2::{Compression, write::GzEncoder};

/// Whether the request's `Accept-Encoding` takes gzip, which every browser's does, unless
/// it gives it a quality of 0.
pub fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let refused = parts.any(|param| {
                param
                    .strip_prefix("q=")
                    .is_some_and(|q| q.parse::<f32>().is_ok_and(|q| q == 0.0))
            });
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
}

/// `bytes`, gzipped as tightly as gzip goes.
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 3), Compression::best());
    encoder.write_all(bytes).expect("writing to memory");
    encoder.finish().expect("writing to memory")
}

/// The pages gzipped so far, by a hash of what they hold. Those that may be gzipped are
/// the same for everyone in a language and colour scheme, so there are a few dozen at
/// most; past that, it starts again.
static GZIPPED: LazyLock<Mutex<Gzipped>> = LazyLock::new(Mutex::default);
const MOST_GZIPPED: usize = 64;

#[derive(Default)]
struct Gzipped {
    /// Keyed afresh on each start, so nobody can make two pages share a key.
    hasher: RandomState,
    pages: HashMap<u64, Arc<[u8]>>,
}
/// `body`, gzipped, from the cache or now. Gzipping a page as tightly as gzip goes
/// costs more than rendering it, so it's done once per page.
fn gzip_cached(body: &[u8]) -> Arc<[u8]> {
    let key = {
        let cache = GZIPPED.lock().expect("gzip cache");
        let key = cache.hasher.hash_one(body);
        if let Some(gzipped) = cache.pages.get(&key) {
            return Arc::clone(gzipped);
        }
        key
    };
    let gzipped: Arc<[u8]> = gzip(body).into();
    let mut cache = GZIPPED.lock().expect("gzip cache");
    if cache.pages.len() >= MOST_GZIPPED {
        cache.pages.clear();
    }
    cache.pages.insert(key, Arc::clone(&gzipped));
    gzipped
}

/// A response of `content_type` with `body`, or with `gzipped` (or `body` gzipped, once
/// for each different body) if the request takes gzip. Caches keep both, by the
/// request's `Accept-Encoding`.
pub fn maybe_gzip(
    headers: &HeaderMap,
    content_type: &'static str,
    body: impl Into<Vec<u8>>,
    gzipped: Option<&[u8]>,
) -> Response {
    let vary = (header::VARY, HeaderValue::from_static("accept-encoding"));
    let content_type = (header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if accepts_gzip(headers) {
        let body = match gzipped {
            Some(gzipped) => gzipped.to_vec(),
            None => gzip_cached(&body.into()).to_vec(),
        };
        let encoding = (header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        return ([vary, content_type, encoding], body).into_response();
    }
    ([vary, content_type], body.into()).into_response()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use flate2::read::GzDecoder;

    use super::*;

    fn with(encoding: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, encoding.parse().unwrap());
        headers
    }

    #[test]
    fn reads_whether_gzip_is_taken() {
        assert!(accepts_gzip(&with("gzip, deflate, br, zstd")));
        assert!(accepts_gzip(&with("br;q=1.0, gzip;q=0.8")));
        assert!(accepts_gzip(&with("*")));
        assert!(!accepts_gzip(&with("gzip;q=0, br")));
        assert!(!accepts_gzip(&with("identity")));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn gzips_each_page_once() {
        let page = "<p>The same page for everyone.</p>".repeat(50);
        let first = gzip_cached(page.as_bytes());
        assert!(Arc::ptr_eq(&first, &gzip_cached(page.as_bytes())));
        let other = gzip_cached(b"<p>Another page.</p>");
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn gzips_what_reads_back() {
        let text = "body { color: red; }\n".repeat(100);
        let packed = gzip(text.as_bytes());
        assert!(packed.len() < text.len() / 10, "{}", packed.len());
        let mut unpacked = String::new();
        GzDecoder::new(&packed[..])
            .read_to_string(&mut unpacked)
            .unwrap();
        assert_eq!(unpacked, text);
    }
}
