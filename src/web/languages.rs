//! The languages the site offers guests: those its admin picked on the admin UI's
//! Languages tab (`db::languages`), read from the database and kept for a few seconds,
//! since every page asks. Each request is handled with them on offer (`i18n::Offer`),
//! which the `Lang` a handler asks for and the language menu go by; an event's pages
//! narrow the offer further, to the event's own languages.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use sqlx::PgPool;

use crate::{db, i18n::Offer};

/// How long a read is kept. The admin UI is another process, so a change there shows
/// here within this; tests read every time, so they see their own changes at once.
const KEEP: Duration = Duration::from_secs(if cfg!(test) { 0 } else { 10 });

pub(super) struct SiteLanguages {
    db: PgPool,
    /// The last read, and when.
    kept: Mutex<Option<(Instant, Offer)>>,
}

impl SiteLanguages {
    pub(super) fn new(db: PgPool) -> Self {
        Self {
            db,
            kept: Mutex::default(),
        }
    }

    /// What the site offers now, or did when last read.
    async fn offer(&self) -> Offer {
        let kept = *self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((read, offer)) = kept
            && read.elapsed() < KEEP
        {
            return offer;
        }
        let offer = match db::languages::site(&self.db).await {
            Ok(offer) => offer,
            // The page will say what it must about the database; until it's back, what
            // was offered still is.
            Err(err) => {
                tracing::warn!(error = %err, "reading the site's languages failed");
                kept.map_or_else(Offer::everything, |(_, offer)| offer)
            }
        };
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *kept = Some((Instant::now(), offer));
        offer
    }
}

/// Middleware: handles the request with the site's languages on offer.
pub(super) async fn scope(
    State(site): State<Arc<SiteLanguages>>,
    request: Request,
    next: Next,
) -> Response {
    site.offer().await.scope(next.run(request)).await
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use sqlx::PgPool;

    use crate::{
        db::{event, languages, rsvp},
        i18n::{Lang, LangSet, Offer},
        web::testing::{Site, body},
    };

    /// The page at `uri`, for a browser that asks for `accept`.
    async fn page(site: &Site, uri: &str, accept: &str) -> String {
        let request = Request::get(uri).header(header::ACCEPT_LANGUAGE, accept);
        body(site.send(request.body(Body::empty()).unwrap()).await).await
    }

    fn set(langs: &[Lang]) -> LangSet {
        langs.iter().copied().collect()
    }

    /// Whether the page's language menu offers this language.
    fn offers(html: &str, code: &str) -> bool {
        html.contains(&format!(r#"<a href="?lang={code}" hreflang="#))
    }

    #[sqlx::test]
    async fn the_site_offers_only_the_languages_its_admin_picked(db: PgPool) {
        let site = Site::new(db);
        // Every language there's a file for, until an admin picks.
        let html = page(&site, "/", "zh-CN").await;
        assert!(html.contains(r#"<html lang="zh-Hans" "#), "{html}");
        assert!(offers(&html, "en-gb") && offers(&html, "nn"), "{html}");

        // Bokmål and plain English, and Bokmål for anyone else.
        let picked = Offer::new(set(&[Lang::NB, Lang::EN]), Lang::NB);
        languages::save_site(&site.db, picked).await.unwrap();
        let html = page(&site, "/", "zh-CN").await;
        assert!(html.contains(r#"<html lang="nb" "#), "{html}");
        assert!(offers(&html, "en"), "{html}");
        for gone in ["nn", "en-gb", "en-us", "zh-hans", "zh-hant"] {
            assert!(!offers(&html, gone), "{gone}: {html}");
        }
        // A language that isn't offered isn't to be had by asking for it, either way.
        let html = page(&site, "/privacy?lang=zh-hans", "zh-CN").await;
        assert!(html.contains(r#"<html lang="nb" "#), "{html}");
        // A British browser gets the plain English that is offered.
        let html = page(&site, "/", "en-GB,en;q=0.9").await;
        assert!(html.contains(r#"<html lang="en" "#), "{html}");
        assert!(offers(&html, "nb") && !offers(&html, "en"), "{html}");
    }

    #[sqlx::test]
    async fn an_event_narrows_the_languages_of_its_own_pages(db: PgPool) {
        let site = Site::new(db);
        let (invite, link) = site.invite("Kari", 1).await;
        let guest = format!("/r/{link}");
        // The event, written in Bokmål, is offered in English too, and nothing else.
        let party = site.event().await;
        let mut details = party.details.clone();
        details.offered = set(&[Lang::EN]);
        event::update(&site.db, party.id, &details).await.unwrap();

        // Its own language is the default, and always offered.
        let html = page(&site, &guest, "zh-CN,zh;q=0.9").await;
        assert!(html.contains(r#"<html lang="nb" "#), "{html}");
        assert!(offers(&html, "en"), "{html}");
        for gone in ["nn", "en-gb", "zh-hans"] {
            assert!(!offers(&html, gone), "{gone}: {html}");
        }
        let html = page(&site, &format!("{guest}/guests?lang=nn"), "nn").await;
        assert!(html.contains(r#"<html lang="nb" "#), "{html}");
        let html = page(&site, &format!("{guest}/you"), "en-GB").await;
        assert!(html.contains(r#"<html lang="en" "#), "{html}");
        // The rest of the site still has every language.
        let html = page(&site, "/", "zh-CN").await;
        assert!(html.contains(r#"<html lang="zh-Hans" "#), "{html}");

        // An answer is kept in the language of the page it was given on, not the one
        // asked for, so the guest's mail is in a language the event has.
        site.post(
            &format!("{guest}?lang=zh-hans"),
            "party_size=1&response=attending",
        )
        .await;
        let answered = rsvp::find(&site.db, invite.id).await.unwrap().unwrap();
        assert_eq!(answered.lang, Lang::NB);

        // The site's languages bound the event's: with English gone from the site,
        // the event is left with its own.
        let only = Offer::new(set(&[Lang::NB, Lang::NN]), Lang::NN);
        languages::save_site(&site.db, only).await.unwrap();
        let html = page(&site, &guest, "en").await;
        assert!(html.contains(r#"<html lang="nb" "#), "{html}");
        assert!(!offers(&html, "en") && !offers(&html, "nn"), "{html}");
    }

    #[sqlx::test]
    async fn a_group_sign_up_page_is_in_its_events_languages(db: PgPool) {
        let site = Site::new(db);
        let party = site.event().await;
        let mut details = party.details.clone();
        details.offered = set(&[Lang::NB, Lang::EN]);
        event::update(&site.db, party.id, &details).await.unwrap();
        let club = crate::db::group::create(&site.db, party.id, "Climbing club", 2)
            .await
            .unwrap();
        let join = format!("/join/{}", club.link(&site.config.link_key));

        let html = page(&site, &join, "zh-TW, en;q=0.5").await;
        assert!(html.contains(r#"<html lang="en" "#), "{html}");
        assert!(offers(&html, "nb") && !offers(&html, "zh-hant"), "{html}");
        // A link that doesn't work has no event, so its page is in the site's languages.
        let html = page(&site, "/join/nope", "zh-TW").await;
        assert!(html.contains(r#"<html lang="zh-Hant" "#), "{html}");
    }
}
