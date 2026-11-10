//! An event's Access tab: who may see or run this event alone, by the name they log in
//! with. Only admins give and take this; anyone with a role of their own already has it
//! for every event (see `role`).

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, load_event, saved},
};
use crate::{
    db::{
        access::{self, Access, EventRole, MAX_USER_LEN},
        event::Event,
    },
    http::render_form,
    i18n::fill,
    paths::{self, EventLinks},
};

#[derive(Template)]
#[template(path = "admin/access.html")]
struct AccessPage {
    event: Event,
    links: EventLinks,
    hosts: Vec<Access>,
    /// The form's, kept when the name needs fixing.
    user: String,
    role: EventRole,
    notice: Option<String>,
    error: Option<String>,
}

async fn access_page(
    state: &AdminState,
    event: Event,
    form: AccessForm,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let hosts = access::list(&state.db, event.id).await?;
    let needs_fixing = error.is_some();
    let page = AccessPage {
        links: EventLinks::new(event.id),
        event,
        hosts,
        user: form.user,
        role: form.role,
        notice,
        error,
    };
    Ok(render_form(page, needs_fixing))
}

/// A name, and the role to give it for the event.
#[derive(Deserialize)]
pub(super) struct AccessForm {
    user: String,
    role: EventRole,
}

impl AccessForm {
    fn blank() -> Self {
        Self {
            user: String::new(),
            role: EventRole::Viewer,
        }
    }

    /// The name, trimmed, or what's wrong with it. A name is kept as typed otherwise,
    /// since a login matches it exactly.
    fn user(&self) -> Result<&str, String> {
        let user = self.user.trim();
        if user.is_empty() || user.chars().count() > MAX_USER_LEN || user.contains(char::is_control)
        {
            let text = &lang::text().error_user_name;
            return Err(fill(text, &[("max", &MAX_USER_LEN)]));
        }
        Ok(user)
    }
}

/// The event's hosts with a role for it alone, and the form to give one.
pub(super) async fn list(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let notice = query.saved.map(|_| lang::text().saved.clone());
    access_page(&state, event, AccessForm::blank(), notice, None).await
}

/// Gives a host a role for the event, in place of the one they had for it.
pub(super) async fn create(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<AccessForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let user = match form.user() {
        Ok(user) => user.to_owned(),
        Err(error) => return access_page(&state, event, form, None, Some(error)).await,
    };
    access::give(&state.db, event.id, &user, form.role).await?;
    tracing::info!(event = %event.id, role = form.role.code(), "event access given");
    Ok(saved(paths::EVENT_ACCESS, event.id))
}

#[derive(Deserialize)]
pub(super) struct RemoveForm {
    user: String,
}

/// Takes a host's role for the event away.
pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<RemoveForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    if access::take(&state.db, event.id, &form.user).await? {
        tracing::info!(event = %event.id, "event access taken");
    }
    Ok(saved(paths::EVENT_ACCESS, event.id))
}
