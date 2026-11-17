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

use crate::{cause::Cause, db, i18n::Offer};

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
                tracing::warn!(error = %Cause::from(&err), "reading the site's languages failed");
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
mod tests;
