//! An event's Details tab (`admin/details.html`): its title, description, place and
//! times to change, the event as a file to download, and deleting it. A new start, end
//! or place is mailed to every guest with a confirmed address.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    file::{EventFile, file_name},
    form::EventForm,
};
use crate::{
    admin::{
        forms::DeleteForm,
        lang,
        page::{AdminState, PageError, load_event, redirect},
    },
    db::{
        event::{self, Details, Event},
        group, outbox, qr, rsvp,
    },
    http::{render, render_with},
    i18n::count,
    janitor::KEEP_AFTER_EVENT,
    paths::{self, EventLinks},
};

#[derive(Template)]
#[template(path = "admin/details.html")]
struct DetailsPage {
    event: Event,
    links: EventLinks,
    form: EventForm,
    keep_after_event_days: i64,
    /// "Saved", and how many guests are told of a new time or place.
    notice: Option<String>,
    error: Option<String>,
}

impl DetailsPage {
    fn new(event: Event, form: EventForm) -> Self {
        Self {
            links: EventLinks::new(event.id),
            event,
            form,
            keep_after_event_days: KEEP_AFTER_EVENT.num_days(),
            notice: None,
            error: None,
        }
    }

    fn render_error(mut self, error: String) -> Response {
        self.error = Some(error);
        render_with(StatusCode::UNPROCESSABLE_ENTITY, self)
    }
}

#[derive(Deserialize)]
pub(in crate::admin) struct DetailsQuery {
    saved: Option<String>,
    /// How many guests a change of time or place is mailed to.
    told: Option<usize>,
}

/// The event's details, to change or to delete it.
pub(in crate::admin) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<DetailsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let form = EventForm::from_details(&event.details);
    let text = lang::text();
    let notice = match (query.saved, query.told) {
        (_, Some(n)) => Some(count(
            n,
            &text.change_notices_one,
            &text.change_notices_other,
        )),
        (Some(_), None) => Some(text.saved.clone()),
        (None, None) => None,
    };
    Ok(render(DetailsPage {
        notice,
        ..DetailsPage::new(event, form)
    }))
}

pub(in crate::admin) async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let form = EventForm::from_fields(&fields);
    let details = match form.details(Utc::now()) {
        Ok(details) => details,
        Err(error) => return Ok(DetailsPage::new(event, form).render_error(error)),
    };
    let mut tx = state.db.begin().await?;
    let updated = event::update(&mut *tx, id, &details)
        .await?
        .ok_or(PageError::Missing)?;
    // A new start, end or place goes to everyone with an address, in the same
    // transaction, so a change is never saved without its mails.
    // A pick is a place in the list, so a changed list would give it another meaning.
    let choices = |details: &Details| details.choices.own(details.lang).to_owned();
    if choices(&updated.details) != choices(&event.details)
        || updated
            .details
            .question
            .own(updated.details.lang)
            .is_empty()
    {
        rsvp::clear_choices(&mut *tx, id).await?;
    }
    let told = if updated.changed_at != event.changed_at {
        Some(outbox::queue_change_notices(&mut *tx, id).await?)
    } else {
        None
    };
    tx.commit().await?;
    tracing::info!(event = %id, ?told, "event changed");
    let base = paths::path(paths::EVENT_DETAILS, id);
    Ok(redirect(&match told {
        Some(n) => format!("{base}?told={n}"),
        None => format!("{base}?saved"),
    }))
}

/// The event and its group links as a file, to keep, copy or import as a new event.
pub(in crate::admin) async fn export(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let groups = group::list(&state.db, id).await?;
    let style = qr::style(&state.db, id).await?;
    let file = EventFile::new(&event.details, &groups, &style).to_toml();
    let disposition = format!(r#"attachment; filename="{}""#, file_name(&event.details));
    let headers = [
        (
            header::CONTENT_TYPE,
            "application/toml; charset=utf-8".to_owned(),
        ),
        (header::CONTENT_DISPOSITION, disposition),
    ];
    Ok((headers, file).into_response())
}

/// Deletes the event with everything in it.
pub(in crate::admin) async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    if form.confirm.is_none() {
        let page = DetailsPage::new(event.clone(), EventForm::from_details(&event.details));
        return Ok(page.render_error(lang::text().error_confirm_delete.clone()));
    }
    event::delete(&state.db, id).await?;
    tracing::info!(event = %id, "event deleted");
    Ok(redirect(&format!("{}?deleted", paths::HOME)))
}
