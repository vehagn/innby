//! "Remember me on this device": a guest can ask the browser to keep their link in a
//! cookie, so the front page leads them back to their page. It's the site's only cookie,
//! set only when the guest asks for it, and it authorises nothing: every change still
//! needs the link in the address, so the cookie can at most show the way.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;

use super::{
    AppState,
    rsvp::{Guest, Notice, redirect},
};
use crate::{i18n::Lang, janitor::KEEP_AFTER_EVENT};

/// The cookie's name. On https, the `__Host-` prefix makes browsers refuse one that
/// another host or path tries to set.
fn name(https: bool) -> &'static str {
    if https { "__Host-innby" } else { "innby" }
}

/// The link this browser remembers, if any.
pub(super) fn link(headers: &HeaderMap, https: bool) -> Option<&str> {
    let name = name(https);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|link| !link.is_empty())
}

fn cookie(https: bool, value: &str, max_age: i64) -> HeaderValue {
    let secure = if https { "; Secure" } else { "" };
    let cookie = format!(
        "{}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}",
        name(https)
    );
    HeaderValue::from_str(&cookie).expect("a link is plain ASCII")
}

/// Tells the browser to drop the cookie.
pub(super) fn forget_cookie(https: bool) -> HeaderValue {
    cookie(https, "", 0)
}

fn with_cookie(mut response: Response, cookie: HeaderValue) -> Response {
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

/// Remembers the guest's link on this device until their data is deleted, after the
/// event.
pub async fn remember(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let until = state.config.event.starts_at.with_timezone(&Utc) + KEEP_AFTER_EVENT;
    let max_age = (until - Utc::now()).num_seconds().max(0);
    let cookie = cookie(state.config.is_https(), &guest.link, max_age);
    tracing::info!(rsvp = %guest.rsvp.id, "device remembered");
    with_cookie(redirect(&guest.link, lang, Notice::Remembered), cookie)
}

/// Forgets the guest on this device, from their page.
pub async fn forget_here(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let cookie = forget_cookie(state.config.is_https());
    with_cookie(redirect(&guest.link, lang, Notice::Forgotten), cookie)
}

/// Forgets whoever this device remembers, from the front page.
pub async fn forget(State(state): State<AppState>, lang: Lang) -> Response {
    let cookie = forget_cookie(state.config.is_https());
    with_cookie(Redirect::to(&lang.home()).into_response(), cookie)
}

/// Who the front page greets.
pub(super) enum Remembered {
    Nobody,
    Guest(Guest),
    /// A link that no longer works: revoked, or its reply deleted.
    Stale,
}

impl Remembered {
    pub(super) async fn load(state: &AppState, headers: &HeaderMap) -> sqlx::Result<Self> {
        let Some(link) = link(headers, state.config.is_https()) else {
            return Ok(Self::Nobody);
        };
        Ok(match Guest::find(state, link).await? {
            Some(guest) => Self::Guest(guest),
            None => Self::Stale,
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use sqlx::PgPool;

    use super::*;
    use crate::{
        db::rsvp,
        i18n::filled,
        web::testing::{Site, body, en, escape, form_post, location},
    };

    fn set_cookie(response: &Response) -> &str {
        response.headers()[header::SET_COOKIE].to_str().unwrap()
    }

    fn with_cookie(mut request: Request<Body>, cookie: &str) -> Request<Body> {
        let cookie = HeaderValue::from_str(cookie).unwrap();
        request.headers_mut().insert(header::COOKIE, cookie);
        request
    }

    fn get(uri: &str, cookie: &str) -> Request<Body> {
        with_cookie(Request::get(uri).body(Body::empty()).unwrap(), cookie)
    }

    #[sqlx::test]
    async fn the_front_page_leads_a_remembered_guest_back(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 2).await;

        // Nothing is set until the guest asks.
        let response = site.get(&format!("/r/{link}?lang=en")).await;
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(
            body(response)
                .await
                .contains(&escape(&en().remember_device))
        );

        let response = site.post(&format!("/r/{link}/remember?lang=en"), "").await;
        assert_eq!(
            location(&response),
            format!("/r/{link}?lang=en&notice=remembered")
        );
        let set = set_cookie(&response);
        assert!(
            set.starts_with(&format!("__Host-innby={link}; Path=/; Max-Age=")),
            "{set}"
        );
        assert!(set.ends_with("; HttpOnly; SameSite=Lax; Secure"), "{set}");
        let cookie = format!("other=1; __Host-innby={link}");

        let response = site.send(get("/?lang=en", &cookie)).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let html = body(response).await;
        let welcome = escape(&filled(&en().welcome_back, "name", "Kari"));
        assert!(html.contains(&welcome), "{html}");
        assert!(html.contains(&format!(r#"href="/r/{link}?lang=en""#)));
        assert!(html.contains(r#"action="/forget?lang=en""#));
        // Without the cookie, the front page is the same for everyone.
        assert!(!body(site.get("/?lang=en").await).await.contains(&welcome));

        // The guest's page knows, and offers to forget.
        let html = body(site.send(get(&format!("/r/{link}?lang=en"), &cookie)).await).await;
        assert!(html.contains(&escape(&en().remembered_text)));
        assert!(html.contains(&format!(r#"action="/r/{link}/forget?lang=en""#)));
        let response = site.post(&format!("/r/{link}/forget?lang=en"), "").await;
        assert_eq!(
            location(&response),
            format!("/r/{link}?lang=en&notice=forgotten")
        );
        assert!(set_cookie(&response).starts_with("__Host-innby=; Path=/; Max-Age=0;"));

        let response = site.post("/forget?lang=en", "").await;
        assert_eq!(location(&response), "/?lang=en");
        assert!(set_cookie(&response).starts_with("__Host-innby=; Path=/; Max-Age=0;"));

        // A link that stopped working is forgotten.
        rsvp::revoke_link(&site.db, invite.id).await.unwrap();
        let response = site.send(get("/?lang=en", &cookie)).await;
        assert!(set_cookie(&response).contains("Max-Age=0"));
        assert!(!body(response).await.contains(&welcome));
    }

    #[sqlx::test]
    async fn deleting_the_reply_forgets_the_device(db: PgPool) {
        let site = Site::new(db);
        let (_, link) = site.invite("Kari", 1).await;
        let request = form_post(&format!("/r/{link}/delete"), "confirm=yes", "same-origin");
        let response = site
            .send(with_cookie(request, &format!("__Host-innby={link}")))
            .await;
        assert!(set_cookie(&response).contains("Max-Age=0"));
    }

    #[test]
    fn reads_only_its_own_cookie() {
        let headers = |cookie: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
            headers
        };
        assert_eq!(link(&headers("__Host-innby=a.b"), true), Some("a.b"));
        assert_eq!(link(&headers("x=1;innby=a.b"), false), Some("a.b"));
        assert_eq!(link(&headers("innby=a.b"), true), None);
        assert_eq!(link(&headers("innbyx=a.b"), false), None);
        assert_eq!(link(&headers("innby="), false), None);
        assert_eq!(link(&HeaderMap::new(), false), None);
    }
}
