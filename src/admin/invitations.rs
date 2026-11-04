//! Invitations: the Invitation links tab, which lists every invitation's link and makes
//! new ones in a batch, a line each.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use uuid::Uuid;

use super::guests::{Row, guest_url};
use super::{
    lang,
    page::{AdminState, PageError, load_event, redirect},
};
use crate::{
    db::{
        event::Event,
        rsvp::{self, MAX_PARTY_SIZE, NameKind, Source},
    },
    http::render_with,
    i18n::fill,
    paths::{self, EventLinks},
};

/// A name and a party size: an invitation's, or a group's for each sign-up.
#[derive(Deserialize)]
pub(super) struct InviteForm {
    pub(super) name: String,
    pub(super) max_party_size: i32,
}

impl InviteForm {
    pub(super) fn validate(&self, kind: NameKind) -> Result<String, String> {
        validate(&self.name, self.max_party_size, kind)
    }
}

pub(super) fn validate(name: &str, max_party_size: i32, kind: NameKind) -> Result<String, String> {
    let text = lang::text();
    if !(1..=MAX_PARTY_SIZE).contains(&max_party_size) {
        return Err(fill(&text.error_party_size, &[("max", &MAX_PARTY_SIZE)]));
    }
    let error = match kind {
        NameKind::Person => &text.error_name,
        NameKind::Group => &text.error_group_name,
    };
    rsvp::normalize_name(name, kind).ok_or_else(|| error.clone())
}

/// Several names at once, one per line.
#[derive(Deserialize)]
pub(super) struct BatchForm {
    pub(super) names: String,
    /// For lines that don't give their own.
    pub(super) max_party_size: i32,
}

impl BatchForm {
    /// Each line's name and party size. A line can end in a comma and its own party size;
    /// blank lines are skipped. One invalid line fails the whole batch.
    pub(super) fn parse(&self, kind: NameKind) -> Result<Vec<(String, i32)>, String> {
        let mut entries = Vec::new();
        for (index, line) in self.names.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, max_party_size) = line
                .rsplit_once(',')
                .and_then(|(name, size)| Some((name, size.trim().parse().ok()?)))
                .unwrap_or((line, self.max_party_size));
            let name = validate(name, max_party_size, kind).map_err(|error| {
                let line = index + 1;
                fill(&lang::text().error_line, &[("n", &line), ("error", &error)])
            })?;
            entries.push((name, max_party_size));
        }
        if entries.is_empty() {
            return Err(lang::text().error_no_names.clone());
        }
        Ok(entries)
    }
}

/// Creates an invitation per line, all or none, then shows their links.
pub(super) async fn create_invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<BatchForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let entries = match form.parse(NameKind::Person) {
        Ok(entries) => entries,
        Err(error) => {
            return invitations_page(&state, event, None, form.names, Some(error)).await;
        }
    };
    let mut tx = state.db.begin().await?;
    let mut ids = Vec::new();
    for (name, max_party_size) in &entries {
        ids.push(
            rsvp::create_invite(&mut *tx, id, name, *max_party_size)
                .await?
                .id,
        );
    }
    tx.commit().await?;
    tracing::info!(count = ids.len(), event = %id, "invitations created");
    // The ids are UUIDv7, so the new ones are those from the first on.
    let page = format!(
        "{}?from={}",
        paths::path(paths::EVENT_INVITATIONS, id),
        ids[0]
    );
    Ok(redirect(&page))
}

#[derive(Template)]
#[template(path = "admin/invitations.html")]
pub(super) struct InvitationsPage {
    pub(super) event: Event,
    pub(super) links: EventLinks,
    /// Only the invitations just created.
    pub(super) new: bool,
    pub(super) invitations: Vec<Invitation>,
    /// The batch form's text, kept when a line needs fixing.
    pub(super) names: String,
    pub(super) max_party_size: i32,
    pub(super) error: Option<String>,
}

pub(super) struct Invitation {
    pub(super) row: Row,
    pub(super) url: String,
}

#[derive(Deserialize)]
pub(super) struct InvitationsQuery {
    pub(super) from: Option<Uuid>,
}

/// Every invitation's link, for sending them out, and the form for new ones.
pub(super) async fn invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<InvitationsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    invitations_page(&state, event, query.from, String::new(), None).await
}

/// The event's invitations from `from` on, or all of them.
pub(super) async fn invitations_page(
    state: &AdminState,
    event: Event,
    from: Option<Uuid>,
    names: String,
    error: Option<String>,
) -> Result<Response, PageError> {
    let rsvps = rsvp::list(&state.db, event.id).await?;
    let tz = event.details.starts_at.timezone();
    let invitations = rsvps
        .iter()
        .filter(|rsvp| rsvp.source == Source::Invite)
        .filter(|rsvp| from.is_none_or(|from| rsvp.id >= from))
        .map(|rsvp| Invitation {
            row: Row::new(rsvp, tz, &[]),
            url: guest_url(&state.config, rsvp),
        })
        .collect();
    let status = match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    };
    Ok(render_with(
        status,
        InvitationsPage {
            links: EventLinks::new(event.id),
            event,
            new: from.is_some(),
            invitations,
            names,
            max_party_size: MAX_PARTY_SIZE,
            error,
        },
    ))
}

#[cfg(test)]
mod tests;
