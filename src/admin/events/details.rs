//! An event's Details tab (`admin/details.html`): its title, description, place and
//! times to change, the event as a file to download, and deleting it.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{
    file::{EventFile, file_name},
    form::EventForm,
};
use crate::{
    admin::{
        forms::{DeleteForm, SavedQuery},
        lang,
        page::{AdminState, PageError, load_event, redirect},
    },
    db::{
        event::{self, Event},
        group,
    },
    http::{render, render_with},
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
    saved: bool,
    error: Option<String>,
}

impl DetailsPage {
    fn new(event: Event, form: EventForm) -> Self {
        Self {
            links: EventLinks::new(event.id),
            event,
            form,
            keep_after_event_days: KEEP_AFTER_EVENT.num_days(),
            saved: false,
            error: None,
        }
    }

    fn render_error(mut self, error: String) -> Response {
        self.error = Some(error);
        render_with(StatusCode::UNPROCESSABLE_ENTITY, self)
    }
}

/// The event's details, to change or to delete it.
pub(in crate::admin) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let form = EventForm::from_details(&event.details);
    Ok(render(DetailsPage {
        saved: query.saved.is_some(),
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
    let details = match form.details() {
        Ok(details) => details,
        Err(error) => return Ok(DetailsPage::new(event, form).render_error(error)),
    };
    event::update(&state.db, id, &details)
        .await?
        .ok_or(PageError::Missing)?;
    tracing::info!(event = %id, "event changed");
    let page = format!("{}?saved", paths::path(paths::EVENT_DETAILS, id));
    Ok(redirect(&page))
}

/// The event and its group links as a file, to keep, copy or import as a new event.
pub(in crate::admin) async fn export(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let groups = group::list(&state.db, id).await?;
    let file = EventFile::new(&event.details, &groups).to_toml();
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
