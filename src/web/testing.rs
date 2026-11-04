//! Helpers for driving the public site's router in tests.

use axum::{
    Router,
    body::Body,
    http::{Request, header},
    response::Response,
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

use crate::{
    config::{Config, tests::parse},
    db::{
        event::Event,
        rsvp::{self, Rsvp},
    },
    i18n::{Lang, Messages},
    link::Purpose,
};

/// The public site on a throwaway database. One router per site, so state such as the
/// rate limiter lives across requests.
pub struct Site {
    pub db: PgPool,
    pub config: Config,
    router: Router,
}

impl Site {
    pub fn new(db: PgPool) -> Self {
        let config = parse(&[]).unwrap();
        Self {
            router: super::router(config.clone(), db.clone()),
            db,
            config,
        }
    }

    /// The event the tests invite guests to.
    pub async fn event(&self) -> Event {
        crate::db::event::tests::event(&self.db).await
    }

    /// Closes answers to the event, as a deadline that has passed does.
    pub async fn close_answers(&self) {
        let event = self.event().await;
        let mut details = event.details;
        details.rsvp_deadline = Some(details.starts_at - chrono::Duration::days(3650));
        crate::db::event::update(&self.db, event.id, &details)
            .await
            .unwrap();
    }

    pub async fn invite(&self, name: &str, max_party_size: i32) -> (Rsvp, String) {
        let event = self.event().await;
        let rsvp = rsvp::create_invite(&self.db, event.id, name, max_party_size)
            .await
            .unwrap();
        let link = self
            .config
            .link_key
            .link(Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
        (rsvp, link)
    }

    pub async fn send(&self, request: Request<Body>) -> Response {
        self.router.clone().oneshot(request).await.unwrap()
    }

    pub async fn get(&self, uri: &str) -> Response {
        self.send(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    /// A same-origin form post, as a browser sends it.
    pub async fn post(&self, uri: &str, form: &str) -> Response {
        self.send(form_post(uri, form, "same-origin")).await
    }

    pub async fn find(&self, rsvp: &Rsvp) -> Option<Rsvp> {
        rsvp::find(&self.db, rsvp.id).await.unwrap()
    }
}

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

/// The British English texts, which tests ask for with `?lang=en`.
pub fn en() -> &'static Messages {
    Lang::EnGb.messages()
}

/// A text as a template writes it into HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&#38;")
        .replace('<', "&#60;")
        .replace('>', "&#62;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

pub fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}
