//! The host's events. This module is the events page (`admin/events.html`): the list of
//! events, and the form for a new one, typed, copied from another event, or read from an
//! event file. An event's guests are in `admin::guests`.
//!
//! - `mod.rs`: the events page, and creating an event with its group links.
//! - `details`: an event's Details tab: changing it, downloading its file, deleting it.
//! - `form`: the event form, shown again to fix, and checked into an event's `Details`.
//! - `file`: an event as a versioned TOML file (`apiVersion = "innby/v1"`, `kind =
//!   "Event"`), to export and import.

pub(super) mod details;
pub(super) mod file;
mod form;

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Multipart, Query, State},
    response::Response,
};
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    forms::BatchForm,
    lang,
    page::{AdminState, PageError, load_event, redirect},
    qr as admin_qr,
    role::{Host, Role},
};
use crate::{
    db::{
        event::{self, Details, Event},
        group,
        qr::{self, Icon, Shape, Style},
        rsvp::{self, Headcount, NameKind},
    },
    http::render_form,
    i18n::filled,
    paths,
};
use file::{EventFile, MAX_BYTES};
use form::EventForm;

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
    draft: Draft,
    notice: Option<String>,
    /// For the layout; what goes wrong here is said beside the form it's about.
    error: Option<String>,
}

/// The new event as the page's forms show it: the event form, its group links a line
/// each, and an event file the host pasted, kept to fix. Messages about each show in
/// its own card, which the page opens at.
struct Draft {
    form: EventForm,
    groups: String,
    file: String,
    /// About the new event's form: where it came from, or what to fix.
    notice: Option<String>,
    error: Option<String>,
    /// About reading an event file.
    file_notice: Option<String>,
    file_error: Option<String>,
    /// The QR codes' style of the event it came from, carried in hidden fields.
    qr: Option<Style>,
}

impl Draft {
    fn blank() -> Self {
        Self::new(EventForm::blank(), String::new())
    }

    fn new(form: EventForm, groups: String) -> Self {
        Self {
            form,
            groups,
            file: String::new(),
            notice: None,
            error: None,
            file_notice: None,
            file_error: None,
            qr: None,
        }
    }

    fn form(&self) -> &EventForm {
        &self.form
    }

    fn groups(&self) -> &str {
        &self.groups
    }

    fn file(&self) -> &str {
        &self.file
    }
}

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    /// An event was just deleted.
    deleted: Option<String>,
    /// An event to fill the new event's form from.
    copy: Option<Uuid>,
}

/// Every event the host may see, the soonest first, and for an admin, the forms for a
/// new one: filled in from another event with `?copy=`.
pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, PageError> {
    let text = lang::text();
    if let Some(id) = query.copy.filter(|_| Role::is_admin()) {
        let event = load_event(&state, id).await?;
        let groups = group::list(&state.db, id).await?;
        let style = qr::style(&state.db, id).await?;
        let title = event.details.title(lang::current()).to_owned();
        let (form, groups, style) = EventFile::new(&event.details, &groups, &style)
            .into_form()
            .expect("an event's own file reads back");
        let draft = Draft {
            notice: Some(filled(&text.copied_notice, "title", title)),
            qr: style,
            ..Draft::new(form, groups)
        };
        return events_page(&state, draft, None).await;
    }
    let notice = query.deleted.map(|_| text.event_deleted.clone());
    events_page(&state, Draft::blank(), notice).await
}

async fn events_page(
    state: &AdminState,
    draft: Draft,
    notice: Option<String>,
) -> Result<Response, PageError> {
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
    let needs_fixing = draft.error.is_some() || draft.file_error.is_some();
    let page = EventsPage {
        events,
        given_only,
        draft,
        notice,
        error: None,
    };
    Ok(render_form(page, needs_fixing))
}

/// Creates an event, with any group links, and opens its guests, where invitations
/// start.
pub(super) async fn create(
    State(state): State<AdminState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let mut draft = Draft {
        qr: carried_style(&fields),
        ..Draft::new(
            EventForm::from_fields(&fields),
            fields.get("groups").cloned().unwrap_or_default(),
        )
    };
    let checked = draft.form.details(Utc::now()).and_then(|details| {
        let style = draft.qr.clone().map(admin_qr::check).transpose()?;
        Ok((details, group_lines(&draft.groups)?, style))
    });
    let (details, groups, style) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            draft.error = Some(error);
            return events_page(&state, draft, None).await;
        }
    };
    let event = create_event(&state.db, &details, &groups, style.as_ref()).await?;
    tracing::info!(event = %event.id, groups = groups.len(), "event created");
    let page = format!("{}?created", paths::path(paths::EVENT, event.id));
    Ok(redirect(&page))
}

/// The QR codes' style the form carries from the event it came from, if any.
fn carried_style(fields: &HashMap<String, String>) -> Option<Style> {
    let get = |name: &str| fields.get(name).map(String::as_str);
    Some(Style {
        shape: Shape::from_code(get("qr_shape")?)?,
        color: get("qr_color")?.to_owned(),
        icon: Icon::from_code(get("qr_icon").unwrap_or("none"))?,
        emoji: get("qr_emoji").unwrap_or_default().to_owned(),
    })
}

/// The group links in the new event's box, a line each: `Climbing club, 2`, or a name
/// alone for two people a sign-up.
pub(super) fn group_lines(lines: &str) -> Result<Vec<(String, i32)>, String> {
    if lines.trim().is_empty() {
        return Ok(Vec::new());
    }
    let batch = BatchForm {
        names: lines.to_owned(),
        max_party_size: 2,
    };
    batch
        .parse(NameKind::Group)
        .map_err(|error| filled(&lang::text().error_event_groups, "error", error))
}

/// Makes an event, its group links, and its QR codes' style if it has one other than
/// the plain one, all or none.
pub(super) async fn create_event(
    db: &PgPool,
    details: &Details,
    groups: &[(String, i32)],
    style: Option<&Style>,
) -> sqlx::Result<Event> {
    let mut tx = db.begin().await?;
    let event = event::create(&mut *tx, details).await?;
    if let Some(style) = style {
        qr::save(&mut *tx, event.id, style).await?;
    }
    for (name, max_party_size) in groups {
        group::create(&mut *tx, event.id, name, *max_party_size).await?;
    }
    tx.commit().await?;
    Ok(event)
}

/// Reads an event file, uploaded or pasted, into the form for a new event, for the host
/// to check and create. Nothing is made yet.
pub(super) async fn import(
    State(state): State<AdminState>,
    mut multipart: Multipart,
) -> Result<Response, PageError> {
    let text = lang::text();
    let (mut pasted, mut uploaded) = (String::new(), None);
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return file_error(&state, String::new(), text.error_no_file.clone()).await,
        };
        let name = field.name().unwrap_or_default().to_owned();
        let Ok(bytes) = field.bytes().await else {
            return file_error(&state, String::new(), text.error_no_file.clone()).await;
        };
        if bytes.len() > MAX_BYTES {
            return file_error(&state, String::new(), text.error_file_too_large.clone()).await;
        }
        match name.as_str() {
            "file" if !bytes.is_empty() => uploaded = Some(bytes),
            "text" => pasted = String::from_utf8_lossy(&bytes).into_owned(),
            _ => {}
        }
    }
    let file = match uploaded {
        Some(bytes) => match String::from_utf8(bytes.to_vec()) {
            Ok(file) => file,
            Err(_) => {
                return file_error(&state, String::new(), text.error_file_not_text.clone()).await;
            }
        },
        None => pasted,
    };
    if file.trim().is_empty() {
        return file_error(&state, file, text.error_no_file.clone()).await;
    }
    let read = EventFile::parse(&file).and_then(EventFile::into_form);
    let (form, groups, style) = match read {
        Ok(read) => read,
        Err(err) => return file_error(&state, file, err.message()).await,
    };
    // Filled in even when something needs fixing, which the form then says.
    let error = form
        .details(Utc::now())
        .and_then(|_| group_lines(&groups).map(drop))
        .and_then(|()| style.clone().map(admin_qr::check).transpose().map(drop))
        .err();
    let file_notice = filled(&text.imported_notice, "title", form.title());
    let draft = Draft {
        error,
        file_notice: Some(file_notice),
        qr: style,
        ..Draft::new(form, groups)
    };
    events_page(&state, draft, None).await
}

/// The events page with the file kept to fix, and what's wrong with it.
async fn file_error(
    state: &AdminState,
    file: String,
    error: String,
) -> Result<Response, PageError> {
    let draft = Draft {
        file,
        file_error: Some(error),
        ..Draft::blank()
    };
    events_page(state, draft, None).await
}
