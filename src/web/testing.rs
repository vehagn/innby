//! Helpers for driving the public site's router in tests. The generic HTTP ones are in
//! `crate::testing`, and re-exported here.

use axum::{Router, body::Body, http::Request, response::Response};
use sqlx::PgPool;
use tower::ServiceExt;

pub use crate::testing::{body, escape, form_post, location};
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
    /// The internal API, when a game has a server.
    internal: Option<Router>,
}

impl Site {
    pub fn new(db: PgPool) -> Self {
        Self::with(db, &[])
    }

    /// The site with these settings on top of the tests' own.
    pub fn with(db: PgPool, settings: &[(&str, &str)]) -> Self {
        let config = parse(settings).unwrap();
        let (router, internal) = super::routers(config.clone(), db.clone());
        Self {
            router,
            internal,
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
        // A minute ago, whenever the event is.
        let zone = details.starts_at.timezone();
        details.rsvp_deadline =
            Some(chrono::Utc::now().with_timezone(&zone) - chrono::Duration::minutes(1));
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

    /// A request to the internal API, as a game's server sends it.
    pub async fn send_internal(&self, request: Request<Body>) -> Response {
        let internal = self.internal.clone().expect("a game with a server");
        internal.oneshot(request).await.unwrap()
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

/// The British English texts, which tests ask for with `?lang=en`.
pub fn en() -> &'static Messages {
    Lang::EnGb.messages()
}
