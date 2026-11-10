//! The New event tab, for admins (`admin/new_event.html`): the form for a new event,
//! typed, copied from another event with `?copy=`, or read from an event file, and
//! creating it. It's a tab of its own so the events page is just the list.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Multipart, Query, State},
    response::Response,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    create_event,
    file::{EventFile, MAX_BYTES},
    form::EventForm,
    group_lines,
};
use crate::{
    admin::{
        forms::{commit_once, form_token, sent_before},
        lang,
        page::{AdminState, PageError, load_event, redirect},
        qr as admin_qr,
    },
    db::{
        group,
        qr::{self, Icon, Shape, Style},
    },
    http::render_form,
    i18n::filled,
    paths,
};

#[derive(Template)]
#[template(path = "admin/new_event.html")]
struct NewEventPage {
    draft: Draft,
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
pub(in crate::admin) struct NewEventQuery {
    /// An event to fill the new event's form from.
    copy: Option<Uuid>,
}

/// The forms for a new event: blank, or filled in from another event with `?copy=`.
pub(in crate::admin) async fn show(
    State(state): State<AdminState>,
    Query(query): Query<NewEventQuery>,
) -> Result<Response, PageError> {
    let Some(id) = query.copy else {
        return Ok(page(Draft::blank()));
    };
    let event = load_event(&state, id).await?;
    let groups = group::list(&state.db, id).await?;
    let style = qr::style(&state.db, id).await?;
    let title = event.details.title(lang::current()).to_owned();
    let (form, groups, style) = EventFile::new(&event.details, &groups, &style)
        .into_form()
        .expect("an event's own file reads back");
    Ok(page(Draft {
        notice: Some(filled(&lang::text().copied_notice, "title", title)),
        qr: style,
        ..Draft::new(form, groups)
    }))
}

fn page(draft: Draft) -> Response {
    let needs_fixing = draft.error.is_some() || draft.file_error.is_some();
    render_form(NewEventPage { draft, error: None }, needs_fixing)
}

/// Creates an event, with any group links, and opens its guests, where invitations
/// start.
pub(in crate::admin) async fn create(
    State(state): State<AdminState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let token = form_token(fields.get("form_token").map(String::as_str));
    if let Some(first) = sent_before(&state.db, token).await? {
        return Ok(first);
    }
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
            return Ok(page(draft));
        }
    };
    let mut tx = state.db.begin().await?;
    let event = create_event(&mut tx, &details, &groups, style.as_ref()).await?;
    let page = format!("{}?created", paths::path(paths::EVENT, event.id));
    if let Some(first) = commit_once(&state.db, tx, token, &page, paths::HOME).await? {
        return Ok(first);
    }
    tracing::info!(event = %event.id, groups = groups.len(), "event created");
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

/// Reads an event file, uploaded or pasted, into the form for a new event, for the host
/// to check and create. Nothing is made yet.
pub(in crate::admin) async fn import(mut multipart: Multipart) -> Result<Response, PageError> {
    let text = lang::text();
    let (mut pasted, mut uploaded) = (String::new(), None);
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return Ok(file_error(String::new(), text.error_no_file.clone())),
        };
        let name = field.name().unwrap_or_default().to_owned();
        let Ok(bytes) = field.bytes().await else {
            return Ok(file_error(String::new(), text.error_no_file.clone()));
        };
        if bytes.len() > MAX_BYTES {
            return Ok(file_error(String::new(), text.error_file_too_large.clone()));
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
                return Ok(file_error(String::new(), text.error_file_not_text.clone()));
            }
        },
        None => pasted,
    };
    if file.trim().is_empty() {
        return Ok(file_error(file, text.error_no_file.clone()));
    }
    let read = EventFile::parse(&file).and_then(EventFile::into_form);
    let (form, groups, style) = match read {
        Ok(read) => read,
        Err(err) => return Ok(file_error(file, err.message())),
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
    Ok(page(draft))
}

/// The page with the file kept to fix, and what's wrong with it.
fn file_error(file: String, error: String) -> Response {
    let draft = Draft {
        file,
        file_error: Some(error),
        ..Draft::blank()
    };
    page(draft)
}
