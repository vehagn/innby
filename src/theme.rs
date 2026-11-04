//! The colour scheme: the device's own, unless the guest picks one with `?theme=`. Like
//! the language, links carry the choice, since the site sets no cookies.

use axum::{extract::Request, middleware::Next, response::Response};

use crate::i18n::Messages;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Follows the device's light or dark setting.
    Auto,
    Light,
    Dark,
}

tokio::task_local! {
    static THEME: Theme;
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Auto, Theme::Light, Theme::Dark];

    /// The `?theme=` value, and `<html data-theme>`.
    pub fn code(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_query(query: &str) -> Self {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("theme="))
            .and_then(|code| Theme::ALL.into_iter().find(|theme| theme.code() == code))
            .unwrap_or(Theme::Auto)
    }

    /// The theme of the request being handled. Outside one, such as in mails, links
    /// carry none and the device decides.
    pub fn current() -> Self {
        THEME.try_with(|theme| *theme).unwrap_or(Theme::Auto)
    }

    /// What to add to a localized link's query: nothing for the device's own.
    pub fn param(self) -> &'static str {
        match self {
            Theme::Auto => "",
            Theme::Light => "&theme=light",
            Theme::Dark => "&theme=dark",
        }
    }

    pub fn label(self, t: &Messages) -> &str {
        match self {
            Theme::Auto => &t.theme_auto,
            Theme::Light => &t.theme_light,
            Theme::Dark => &t.theme_dark,
        }
    }

    /// The icon on the theme switch: half a circle, a sun or a moon.
    pub fn icon(self) -> &'static str {
        match self {
            Theme::Auto => {
                r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8"/><path d="M12 4a8 8 0 0 1 0 16z" fill="currentColor"/></svg>"#
            }
            Theme::Light => {
                r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2.5M12 19.5V22M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M2 12h2.5M19.5 12H22M4.9 19.1l1.8-1.8M17.3 6.7l1.8-1.8"/></svg>"#
            }
            Theme::Dark => {
                r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 14.5A8.5 8.5 0 1 1 9.5 4a7 7 0 0 0 10.5 10.5z"/></svg>"#
            }
        }
    }
}

/// Middleware: handles the request with the theme its `?theme=` asks for.
pub async fn scope(request: Request, next: Next) -> Response {
    let theme = request.uri().query().map_or(Theme::Auto, Theme::from_query);
    THEME.scope(theme, next.run(request)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_query() {
        assert_eq!(Theme::from_query("lang=nb&theme=dark"), Theme::Dark);
        assert_eq!(Theme::from_query("theme=light"), Theme::Light);
        assert_eq!(Theme::from_query("theme=purple"), Theme::Auto);
        assert_eq!(Theme::from_query("lang=nb"), Theme::Auto);
        assert_eq!(Theme::current(), Theme::Auto);
    }
}
