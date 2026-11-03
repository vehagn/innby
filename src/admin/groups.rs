//! Group links: a sign-up link per group of people, for its group chat or a poster.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use uuid::Uuid;

use super::{
    AdminState, BatchForm, DeleteForm, Feedback, InviteForm, PageError, Row, ShowQuery, index_page,
    local, qr_svg,
};
use crate::{
    config::Config,
    db::{
        group::{self, Group},
        rsvp::{self, MAX_PARTY_SIZE, Rsvp},
    },
    link::Purpose,
    paths::{self, AdminLinks},
    web::{render, render_with, server_error},
};

/// One group as the index page lists it.
pub(super) struct GroupRow {
    /// The group's admin page.
    pub path: String,
    pub name: String,
    pub max_party_size: i32,
    pub sign_ups: usize,
    pub url: String,
}

impl GroupRow {
    pub(super) fn new(config: &Config, group: &Group, rsvps: &[Rsvp]) -> Self {
        Self {
            path: paths::admin(&paths::path(paths::GROUP, group.id)),
            name: group.name.clone(),
            max_party_size: group.max_party_size,
            sign_ups: rsvps
                .iter()
                .filter(|rsvp| rsvp.group_id == Some(group.id))
                .count(),
            url: group_url(config, group),
        }
    }
}

fn group_url(config: &Config, group: &Group) -> String {
    let link = config
        .link_key
        .link(Purpose::Group, group.id, group.link_nonce);
    config.url(&paths::path(paths::JOIN, link))
}

/// Creates a group per line, all or none.
pub(super) async fn create(
    State(state): State<AdminState>,
    Form(form): Form<BatchForm>,
) -> Response {
    let entries = match form.parse() {
        Ok(entries) => entries,
        Err(error) => {
            let feedback = Feedback {
                groups: form.names,
                ..Feedback::default()
            };
            return index_page(&state, Some(error), feedback).await;
        }
    };
    let created = async {
        let mut tx = state.db.begin().await?;
        for (name, max_party_size) in &entries {
            group::create(&mut *tx, name, *max_party_size).await?;
        }
        tx.commit().await
    }
    .await;
    match created {
        Ok(()) => {
            tracing::info!(count = entries.len(), "groups created");
            let index = format!("{}?groups={}#groups", paths::HOME, entries.len());
            Redirect::to(&paths::admin(&index)).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "admin/group.html")]
struct GroupPage {
    group: Group,
    links: AdminLinks,
    created: String,
    url: String,
    qr: String,
    /// Who signed up through the link.
    members: Vec<Row>,
    max_party_size: i32,
    accepts_answers: bool,
    saved: bool,
    error: Option<&'static str>,
}

impl GroupPage {
    async fn load(state: &AdminState, id: Uuid) -> Result<Self, PageError> {
        let (group, rsvps) = tokio::try_join!(group::find(&state.db, id), rsvp::list(&state.db))?;
        let group = group.ok_or(PageError::Missing)?;
        let tz = state.config.event.starts_at.timezone();
        let url = group_url(&state.config, &group);
        Ok(Self {
            members: rsvps
                .iter()
                .filter(|rsvp| rsvp.group_id == Some(id))
                .map(|rsvp| Row::new(rsvp, tz, std::slice::from_ref(&group)))
                .collect(),
            qr: qr_svg(&url),
            url,
            links: AdminLinks::group(group.id),
            created: local(group.created_at, tz),
            group,
            max_party_size: MAX_PARTY_SIZE,
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
            saved: false,
            error: None,
        })
    }

    async fn render_error(
        state: &AdminState,
        id: Uuid,
        error: &'static str,
    ) -> Result<Response, PageError> {
        let mut page = Self::load(state, id).await?;
        page.error = Some(error);
        Ok(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
    }
}

fn saved_redirect(id: Uuid) -> Response {
    let page = format!("{}?saved", paths::path(paths::GROUP, id));
    Redirect::to(&paths::admin(&page)).into_response()
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ShowQuery>,
) -> Result<Response, PageError> {
    let mut page = GroupPage::load(&state, id).await?;
    page.saved = query.saved.is_some();
    Ok(render(page))
}

/// Renames the group, or changes how many people new sign-ups may cover.
pub(super) async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<InviteForm>,
) -> Result<Response, PageError> {
    let name = match form.validate() {
        Ok(name) => name,
        Err(error) => return GroupPage::render_error(&state, id, error).await,
    };
    if !group::update(&state.db, id, &name, form.max_party_size).await? {
        return Err(PageError::Missing);
    }
    Ok(saved_redirect(id))
}

/// Replaces the group's link, for when it has spread too far. Those who signed up keep
/// their own links.
pub(super) async fn revoke(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    group::revoke_link(&state.db, id)
        .await?
        .ok_or(PageError::Missing)?;
    tracing::info!(group = %id, "group link revoked");
    Ok(saved_redirect(id))
}

/// Deletes a group nobody has signed up through.
pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    if form.confirm.is_none() {
        return GroupPage::render_error(&state, id, "Tick the box to confirm the deletion.").await;
    }
    if !group::delete_unused(&state.db, id).await? {
        return match group::find(&state.db, id).await? {
            Some(_) => {
                GroupPage::render_error(
                    &state,
                    id,
                    "People have signed up through this group, so it can't be deleted.",
                )
                .await
            }
            None => Err(PageError::Missing),
        };
    }
    tracing::info!(group = %id, "group deleted");
    Ok(Redirect::to(&paths::admin(&format!("{}#groups", paths::HOME))).into_response())
}
