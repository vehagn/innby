//! The host's events. This module is the events page (`admin/events.html`), the list of
//! events, and what making one needs. An event's guests are in `admin::guests`.
//!
//! - `mod.rs`: the events page, and creating an event with its group links.
//! - `new`: the New event tab, for admins: a new event typed, copied from another event,
//!   or read from an event file.
//! - `details`: an event's Details tab: changing it, downloading its file, deleting it.
//! - `form`: the event form, shown again to fix, and checked into an event's `Details`.
//! - `file`: an event as a versioned TOML file (`apiVersion = "innby/v1"`, `kind =
//!   "Event"`), to export and import.

pub(super) mod details;
pub(super) mod file;
mod form;
pub(super) mod new;

use askama::Template;
use axum::{
    extract::{Query, State},
    response::Response,
};
use serde::Deserialize;
use sqlx::PgConnection;

use super::{
    forms::BatchForm,
    lang,
    page::{AdminState, PageError},
    role::Host,
};
use crate::{
    db::{
        event::{self, Details, Event},
        group,
        qr::{self, Style},
        rsvp::{self, Headcount, NameKind},
    },
    http::render,
    i18n::{LangSet, filled},
    paths,
};

/// One event as the list shows it.
struct EventRow {
    path: String,
    title: String,
    when: String,
    headcount: Headcount,
}

#[derive(Template)]
#[template(path = "admin/events.html")]
struct EventsPage {
    events: Vec<EventRow>,
    /// Whether these are only the events that give the host a role, who has none of
    /// their own.
    given_only: bool,
    notice: Option<String>,
    /// For the layout.
    error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    /// An event was just deleted.
    deleted: Option<String>,
}

/// Every event the host may see, the soonest first.
pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, PageError> {
    let notice = query.deleted.map(|_| lang::text().event_deleted.clone());
    events_page(&state, notice).await
}

async fn events_page(state: &AdminState, notice: Option<String>) -> Result<Response, PageError> {
    let lang = lang::current();
    // Without a role of their own, a host sees only the events that give them one.
    let host = Host::current();
    let given_only = host.role.is_none();
    let events = async {
        if given_only {
            event::list_given(&state.db, &host.user).await
        } else {
            event::list(&state.db).await
        }
    };
    let (events, headcounts) = tokio::try_join!(events, rsvp::headcounts(&state.db))?;
    let events = events
        .into_iter()
        .map(|event| EventRow {
            path: paths::admin(&paths::path(paths::EVENT, event.id)),
            title: event.details.title(lang).to_owned(),
            when: lang.format_date(event.details.starts_at),
            headcount: headcounts.get(&event.id).copied().unwrap_or_default(),
        })
        .collect();
    Ok(render(EventsPage {
        events,
        given_only,
        notice,
        error: None,
    }))
}

/// The languages the site offers guests, which are the ones an event's form asks about.
pub(super) async fn site_languages(state: &AdminState) -> Result<LangSet, PageError> {
    Ok(crate::db::languages::site(&state.db).await?.languages)
}

/// The group links in the new event's box, a line each: `Climbing club, 2`, or a name
/// alone for two people a sign-up.
pub(super) fn group_lines(lines: &str) -> Result<Vec<(String, i32)>, String> {
    if lines.trim().is_empty() {
        return Ok(Vec::new());
    }
    let batch = BatchForm {
        form_token: None,
        names: lines.to_owned(),
        max_party_size: 2,
    };
    batch
        .parse(NameKind::Group)
        .map_err(|error| filled(&lang::text().error_event_groups, "error", error))
}

/// Makes an event, its group links, and its QR codes' style if it has one other than
/// the plain one, in the caller's transaction, so it's all or none, noting who made it
/// if a host did.
pub(super) async fn create_event(
    tx: &mut PgConnection,
    details: &Details,
    groups: &[(String, i32)],
    style: Option<&Style>,
    maker: Option<&str>,
) -> sqlx::Result<Event> {
    let event = event::create(&mut *tx, details).await?;
    if let Some(maker) = maker {
        crate::db::access::set_maker(&mut *tx, event.id, maker).await?;
    }
    if let Some(style) = style {
        qr::save(&mut *tx, event.id, style).await?;
    }
    for (name, max_party_size) in groups {
        group::create(&mut *tx, event.id, name, *max_party_size).await?;
    }
    Ok(event)
}
