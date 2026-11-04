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
    guests::{Row, local},
    invitations::{BatchForm, InviteForm},
    lang,
    page::{AdminState, DeleteForm, PageError, SavedQuery, load_event},
    qr::{self as admin_qr, qr_file},
};
use crate::{
    config::Config,
    db::{
        event::Event,
        group::{self, Group},
        qr,
        rsvp::{self, MAX_PARTY_SIZE, Rsvp},
    },
    http::{render, render_with},
    i18n::count,
    link::Purpose,
    paths::{self, AdminLinks, EventLinks},
};

/// One group as the group links page lists it.
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

#[derive(Template)]
#[template(path = "admin/groups.html")]
struct GroupsPage {
    event: Event,
    links: EventLinks,
    groups: Vec<GroupRow>,
    /// The batch form's text, kept when a line needs fixing.
    names: String,
    max_party_size: i32,
    /// False once the answer deadline has passed, which closes the group links too.
    accepts_answers: bool,
    notice: Option<String>,
    error: Option<String>,
}

async fn groups_page(
    state: &AdminState,
    event: Event,
    names: String,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let (groups, rsvps) = tokio::try_join!(
        group::list(&state.db, event.id),
        rsvp::list(&state.db, event.id)
    )?;
    let status = match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    };
    let page = GroupsPage {
        links: EventLinks::new(event.id),
        accepts_answers: event.details.accepts_answers(Utc::now()),
        event,
        groups: groups
            .iter()
            .map(|group| GroupRow::new(&state.config, group, &rsvps))
            .collect(),
        names,
        max_party_size: MAX_PARTY_SIZE,
        notice,
        error,
    };
    Ok(render_with(status, page))
}

#[derive(serde::Deserialize)]
pub(super) struct GroupsQuery {
    /// How many groups were just created.
    created: Option<usize>,
}

/// The event's group links, and the form for new ones.
pub(super) async fn list(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<GroupsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let text = lang::text();
    let notice = query
        .created
        .map(|n| count(n, &text.groups_created_one, &text.groups_created_other));
    groups_page(&state, event, String::new(), notice, None).await
}

/// Creates a group per line for the event, all or none.
pub(super) async fn create(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<BatchForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let entries = match form.parse() {
        Ok(entries) => entries,
        Err(error) => return groups_page(&state, event, form.names, None, Some(error)).await,
    };
    let mut tx = state.db.begin().await?;
    for (name, max_party_size) in &entries {
        group::create(&mut *tx, id, name, *max_party_size).await?;
    }
    tx.commit().await?;
    tracing::info!(count = entries.len(), event = %id, "groups created");
    let page = format!(
        "{}?created={}",
        paths::path(paths::EVENT_GROUPS, id),
        entries.len()
    );
    Ok(Redirect::to(&paths::admin(&page)).into_response())
}

#[derive(Template)]
#[template(path = "admin/group.html")]
struct GroupPage {
    event: Event,
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
    error: Option<String>,
}

impl GroupPage {
    async fn load(state: &AdminState, id: Uuid) -> Result<Self, PageError> {
        let group = group::find(&state.db, id)
            .await?
            .ok_or(PageError::Missing)?;
        let event = load_event(state, group.event_id).await?;
        let (rsvps, style) =
            tokio::try_join!(rsvp::list(&state.db, group.event_id), qr::style(&state.db),)?;
        let tz = event.details.starts_at.timezone();
        let url = group_url(&state.config, &group);
        Ok(Self {
            members: rsvps
                .iter()
                .filter(|rsvp| rsvp.group_id == Some(id))
                .map(|rsvp| Row::new(rsvp, tz, std::slice::from_ref(&group)))
                .collect(),
            qr: admin_qr::svg(&url, &style, None),
            url,
            links: AdminLinks::group(group.id),
            created: local(group.created_at, tz),
            group,
            max_party_size: MAX_PARTY_SIZE,
            accepts_answers: event.details.accepts_answers(Utc::now()),
            event,
            saved: false,
            error: None,
        })
    }

    async fn render_error(
        state: &AdminState,
        id: Uuid,
        error: String,
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
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let mut page = GroupPage::load(&state, id).await?;
    page.saved = query.saved.is_some();
    Ok(render(page))
}

/// The group's QR code as a file, for a poster.
pub(super) async fn qr(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let (group, style) = tokio::try_join!(group::find(&state.db, id), qr::style(&state.db))?;
    let group = group.ok_or(PageError::Missing)?;
    Ok(qr_file(
        &group_url(&state.config, &group),
        &style,
        &group.name,
    ))
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
        let error = lang::text().error_confirm_delete.clone();
        return GroupPage::render_error(&state, id, error).await;
    }
    let group = group::find(&state.db, id)
        .await?
        .ok_or(PageError::Missing)?;
    if !group::delete_unused(&state.db, id).await? {
        let error = lang::text().error_group_in_use.clone();
        return GroupPage::render_error(&state, id, error).await;
    }
    tracing::info!(group = %id, "group deleted");
    let groups = paths::path(paths::EVENT_GROUPS, group.event_id);
    Ok(Redirect::to(&paths::admin(&groups)).into_response())
}

#[cfg(test)]
mod tests {

    use sqlx::PgPool;

    use super::*;
    use crate::{
        admin::testing::{Admin, links},
        web::testing::{body, location},
    };

    #[sqlx::test]
    async fn creates_and_manages_groups(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let response = admin
            .post(
                &format!("{e}/groups"),
                "names=Climbing+club%0D%0ANeighbours%2C+4&max_party_size=2",
            )
            .await;
        assert_eq!(location(&response), format!("{e}/groups?created=2"));
        let html = body(admin.get(&format!("{e}/groups?created=2")).await).await;
        assert!(html.contains("2 groups created."));
        let [climbing, neighbours] = group::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap()
        .try_into()
        .unwrap();
        assert_eq!(
            (neighbours.name.as_str(), neighbours.max_party_size),
            ("Neighbours", 4)
        );
        let url = format!(
            "https://fest.example.com/join/{}",
            admin.config.link_key.link(Purpose::Group, climbing.id, 0)
        );
        assert!(links(&html).contains(&url.as_str()));

        let page = format!("/groups/{}", climbing.id);
        let html = body(admin.get(&page).await).await;
        assert!(html.contains("<h1>Climbing club</h1>"));
        assert!(html.contains(&format!(r#"value="{url}""#)));
        assert!(html.contains("<svg"));

        // Someone signs up; the group shows them and can't be deleted any more.
        let sign_up = rsvp::SignUp {
            group: &climbing,
            name: "Per Hansen",
            party_size: 1,
            name_display: rsvp::NameDisplay::Partial,
            lang: crate::i18n::Lang::Nb,
        };
        rsvp::create_open(&admin.db, &sign_up, crate::web::PRIVACY_VERSION)
            .await
            .unwrap();
        let html = body(admin.get(&page).await).await;
        assert!(html.contains("Per Hansen"));
        assert!(html.contains("first name and initial"));
        let html = body(admin.get(&e).await).await;
        assert!(html.contains("sign-up: Climbing club"));
        let response = admin.post(&format!("{page}/delete"), "confirm=yes").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("can&#39;t be deleted"));

        admin
            .post(&page, "name=Climbing+friends&max_party_size=3")
            .await;
        admin.post(&format!("{page}/revoke"), "").await;
        let edited = group::find(&admin.db, climbing.id).await.unwrap().unwrap();
        assert_eq!(
            (
                edited.name.as_str(),
                edited.max_party_size,
                edited.link_nonce
            ),
            ("Climbing friends", 3, 1)
        );

        let response = admin
            .post(&format!("/groups/{}/delete", neighbours.id), "confirm=yes")
            .await;
        assert_eq!(location(&response), format!("{e}/groups"));
        assert!(
            group::find(&admin.db, neighbours.id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
