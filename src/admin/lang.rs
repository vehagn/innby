//! The admin site's language: the browser's, unless the host picks one with `?lang=`.
//! Pages and links read it from here rather than each handler passing it on. As with the
//! colour scheme, links carry `?lang=` only once the host has picked one.

use axum::{extract::Request, middleware::Next, response::Response};

use crate::i18n::{AdminMessages, Lang};

#[derive(Clone, Copy)]
struct Choice {
    lang: Lang,
    /// Whether `?lang=` picked it, rather than the browser.
    picked: bool,
}

tokio::task_local! {
    static LANG: Choice;
}

/// The language of the admin page being handled.
pub fn current() -> Lang {
    LANG.try_with(|choice| choice.lang).unwrap_or(Lang::DEFAULT)
}

/// The language the host picked, for links to carry.
pub fn picked() -> Option<Lang> {
    LANG.try_with(|choice| choice.picked.then_some(choice.lang))
        .ok()
        .flatten()
}

/// Runs work outside a request, such as a CLI command, in a language.
pub async fn within<T>(lang: Lang, work: impl Future<Output = T>) -> T {
    LANG.scope(
        Choice {
            lang,
            picked: false,
        },
        work,
    )
    .await
}

/// The admin texts in the page's language.
pub fn text() -> &'static AdminMessages {
    &current().messages().admin
}

/// Middleware: handles the request in the language `?lang=` or the browser asks for.
pub async fn scope(lang: Lang, request: Request, next: Next) -> Response {
    let picked = request.uri().query().is_some_and(|query| {
        query
            .split('&')
            .filter_map(|pair| pair.strip_prefix("lang="))
            .any(|code| Lang::from_code(code).is_some())
    });
    LANG.scope(Choice { lang, picked }, next.run(request)).await
}
