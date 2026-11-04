//! Invitations: the Invitation links tab, which lists every invitation's link and makes
//! new ones in a batch, a line each.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    forms::BatchForm,
    guests::{GuestRow, guest_url},
    page::{AdminState, PageError, load_event, redirect},
};
use crate::{
    db::{
        event::Event,
        rsvp::{self, MAX_PARTY_SIZE, NameKind},
    },
    http::render_form,
    paths::{self, EventLinks},
};

/// Creates an invitation per line, all or none, then shows their links.
pub(super) async fn create(
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
    pub(super) row: GuestRow,
    pub(super) url: String,
}

#[derive(Deserialize)]
pub(super) struct InvitationsQuery {
    pub(super) from: Option<Uuid>,
}

/// Every invitation's link, for sending them out, and the form for new ones.
pub(super) async fn list(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<InvitationsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    invitations_page(&state, event, query.from, String::new(), None).await
}

/// The event's invitations from `from` on, or all of them.
async fn invitations_page(
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
        .filter(|rsvp| rsvp.is_invitation())
        .filter(|rsvp| from.is_none_or(|from| rsvp.id >= from))
        .map(|rsvp| Invitation {
            row: GuestRow::new(rsvp, tz, &[]),
            url: guest_url(&state.config, rsvp),
        })
        .collect();
    let needs_fixing = error.is_some();
    let page = InvitationsPage {
        links: EventLinks::new(event.id),
        event,
        new: from.is_some(),
        invitations,
        names,
        max_party_size: MAX_PARTY_SIZE,
        error,
    };
    Ok(render_form(page, needs_fixing))
}

#[cfg(test)]
mod tests;
