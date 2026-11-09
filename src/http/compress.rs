//! Gzip, for the responses that are safe to compress: the stylesheet and the privacy
//! notice, which hold no secret and nothing a visitor put there.
//!
//! Every other page holds a link, which is a guest's or a group's key, and many also show
//! what someone typed, such as a name signed up through a group link. Compressing both
//! together lets whoever can choose that text and watch the response's size guess the
//! link a few characters at a time (BREACH). Those pages stay uncompressed.

use std::io::Write;

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

/// A response of `content_type` with `body`, or with `gzipped` (or `body` gzipped now)
/// if the request takes gzip. Caches keep both, by the request's `Accept-Encoding`.
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
            None => gzip(&body.into()),
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
