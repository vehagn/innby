//! Pages from templates, and the stylesheet, which every page links with its hash.

use std::{fmt::Display, sync::LazyLock};

use askama::Template;
use axum::{
    extract::RawQuery,
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use super::compress;
use crate::paths;

const STYLE: &str = include_str!("../../static/style.css");

/// `v=` and a hash of the stylesheet: pages link it with this query, so a changed
/// stylesheet gets a new URL and browsers can keep each one for good.
static STYLE_VERSION: LazyLock<String> =
    LazyLock::new(|| format!("v={}", URL_SAFE_NO_PAD.encode(&Sha256::digest(STYLE)[..6])));

/// The stylesheet, gzipped once, for the browsers that take it, which is all of them.
static STYLE_GZIP: LazyLock<Vec<u8>> = LazyLock::new(|| compress::gzip(STYLE.as_bytes()));

/// The stylesheet's link, for templates.
pub fn stylesheet_link() -> String {
    format!("{}?{}", paths::STYLESHEET, *STYLE_VERSION)
}

pub async fn stylesheet(RawQuery(query): RawQuery, headers: HeaderMap) -> impl IntoResponse {
    let cache = if query.as_deref() == Some(STYLE_VERSION.as_str()) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };
    let css = "text/css; charset=utf-8";
    let response = compress::maybe_gzip(&headers, css, STYLE, Some(&STYLE_GZIP));
    ([(header::CACHE_CONTROL, cache)], response)
}

/// A page that holds no secret and nothing a visitor typed, gzipped if the request takes
/// it; see `compress` for why only such pages are.
pub fn render_compressed(headers: &HeaderMap, template: impl Template) -> Response {
    match template.render() {
        Ok(html) => compress::maybe_gzip(headers, "text/html; charset=utf-8", html, None),
        Err(err) => server_error(err),
    }
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

/// A page with a form on it: 200, or 422 when it shows something to fix, so that tests
/// and tools can tell a refused form from a fresh one.
pub fn render_form(template: impl Template, needs_fixing: bool) -> Response {
    let status = match needs_fixing {
        true => StatusCode::UNPROCESSABLE_ENTITY,
        false => StatusCode::OK,
    };
    render_with(status, template)
}

/// Logs the cause and answers 500 without details.
pub fn server_error(err: impl Display) -> Response {
    tracing::error!(error = %err, "request failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

#[cfg(test)]
mod tests {
    //! The stylesheet and the markup agree, so the look can be changed in CSS alone:
    //! every class a page uses has a rule, every rule's class is used, and no page
    //! styles itself.

    use std::{collections::BTreeSet, fs, path::Path};

    use super::STYLE;

    /// The templates and the code, which both write markup, as (path, text). Tests are
    /// left out, since their markup is made up.
    fn markup() -> Vec<(String, String)> {
        fn walk(dir: &Path, found: &mut Vec<(String, String)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy();
                if path.is_dir() {
                    if name != "e2e" {
                        walk(&path, found);
                    }
                } else if (name.ends_with(".html") || name.ends_with(".rs")) && name != "tests.rs" {
                    let text = fs::read_to_string(&path).unwrap();
                    let code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
                    found.push((path.display().to_string(), code.to_string()));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = Vec::new();
        walk(&root.join("templates"), &mut found);
        walk(&root.join("src"), &mut found);
        found
    }

    /// `text` without anything from `open` to `close`.
    fn without(text: &str, open: &str, close: &str) -> String {
        let mut kept = String::new();
        let mut rest = text;
        while let Some(start) = rest.find(open) {
            kept.push_str(&rest[..start]);
            rest = rest[start..]
                .find(close)
                .map_or("", |end| &rest[start + end + close.len()..]);
        }
        kept + rest
    }

    /// The classes in `class="…"` attributes, leaving out template expressions.
    fn classes_in_markup(text: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for attribute in text.split(r#"class=""#).skip(1) {
            let value = attribute.split('"').next().unwrap_or_default();
            let value = without(&without(value, "{{", "}}"), "{%", "%}");
            found.extend(value.split_whitespace().map(String::from));
        }
        found
    }

    /// The classes the stylesheet's selectors name.
    fn classes_in_css(css: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut prelude = String::new();
        for c in without(css, "/*", "*/").chars() {
            match c {
                '{' => {
                    for name in prelude.split('.').skip(1) {
                        let name: String = name
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                            .collect();
                        if name.starts_with(|c: char| c.is_ascii_alphabetic()) {
                            found.insert(name);
                        }
                    }
                    prelude.clear();
                }
                '}' | ';' => prelude.clear(),
                c => prelude.push(c),
            }
        }
        found
    }

    #[test]
    fn every_class_in_the_markup_has_a_rule() {
        let styled = classes_in_css(STYLE);
        for (path, text) in markup() {
            for class in classes_in_markup(&text) {
                assert!(
                    styled.contains(&class),
                    "{path} uses .{class}, which static/style.css doesn't style"
                );
            }
            assert!(
                !text.contains(r#" style=""#),
                "{path} styles itself; use a class"
            );
        }
    }

    #[test]
    fn every_class_in_the_stylesheet_is_used() {
        let markup = markup();
        let used: BTreeSet<String> = markup
            .iter()
            .flat_map(|(_, text)| classes_in_markup(text))
            .collect();
        for class in classes_in_css(STYLE) {
            // Some are chosen in code, such as an answer's colour.
            let in_code = markup
                .iter()
                .any(|(_, text)| text.contains(&format!(r#""{class}""#)));
            assert!(
                used.contains(&class) || in_code,
                ".{class} in static/style.css is used nowhere"
            );
        }
    }

    #[test]
    fn finds_classes() {
        let css = "/* .not */ a.button:hover, .x .y-z { color: red; } @media (width < 1.5rem) { .q { b: 0.5rem } }";
        assert_eq!(
            classes_in_css(css),
            ["button", "q", "x", "y-z"].map(String::from).into()
        );
        let html = r#"<p class="hint {{ a.b }}{% if c %} d{% endif %}">"#;
        assert_eq!(
            classes_in_markup(html),
            ["d", "hint"].map(String::from).into()
        );
    }
}
