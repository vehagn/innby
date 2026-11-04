//! The host's events: the list of them, a new one (typed, copied from another, or read
//! from an event file), and each one's details, with its export.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    event_file::{API_VERSION, EventFile, FileError, KIND, MAX_BYTES, file_name},
    invitations::BatchForm,
    lang,
    page::{AdminState, DeleteForm, PageError, SavedQuery, load_event, redirect},
    role::Role,
};
use crate::{
    db::{
        event::{self, Details, Event},
        group,
        rsvp::{self, Headcount, NameKind},
    },
    http::{render, render_with},
    i18n::{Lang, Texts, fill, filled},
    janitor::KEEP_AFTER_EVENT,
    paths::{self, EventLinks},
};

/// The longest title, description and place the form takes.
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 5000;
const MAX_LOCATION: usize = 200;

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
    draft: Draft,
    notice: Option<String>,
    /// For the layout; what goes wrong here is said beside the form it's about.
    error: Option<String>,
}

/// The new event as the page's forms show it: the event form, its group links a line
/// each, and an event file the host pasted, kept to fix. Messages about each show in
/// its own card, which the page opens at.
pub(super) struct Draft {
    form: EventForm,
    groups: String,
    file: String,
    /// About the new event's form: where it came from, or what to fix.
    pub(super) notice: Option<String>,
    pub(super) error: Option<String>,
    /// About reading an event file.
    pub(super) file_notice: Option<String>,
    pub(super) file_error: Option<String>,
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
        }
    }

    pub(super) fn form(&self) -> &EventForm {
        &self.form
    }

    pub(super) fn groups(&self) -> &str {
        &self.groups
    }

    pub(super) fn file(&self) -> &str {
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

/// Every event, the soonest first, and the forms for a new one: filled in from another
/// event with `?copy=`.
pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, PageError> {
    let text = lang::text();
    if let Some(id) = query.copy.filter(|_| Role::is_admin()) {
        let event = load_event(&state, id).await?;
        let groups = group::list(&state.db, id).await?;
        let title = event.details.title(lang::current()).to_owned();
        let (form, groups) = EventFile::new(&event.details, &groups)
            .into_form()
            .expect("an event's own file reads back");
        let draft = Draft {
            notice: Some(filled(&text.copied_notice, "title", title)),
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
    let mut events = Vec::new();
    for event in event::list(&state.db).await? {
        events.push(EventRow {
            path: paths::admin(&paths::path(paths::EVENT, event.id)),
            title: event.details.title(lang).to_owned(),
            when: lang.format_date(event.details.starts_at),
            headcount: rsvp::headcount(&state.db, event.id).await?,
        });
    }
    let status = match (&draft.error, &draft.file_error) {
        (None, None) => StatusCode::OK,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let page = EventsPage {
        events,
        draft,
        notice,
        error: None,
    };
    Ok(render_with(status, page))
}

/// Creates an event, with any group links, and opens its guests, where invitations
/// start.
pub(super) async fn create(
    State(state): State<AdminState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let mut draft = Draft::new(
        EventForm::from_fields(&fields),
        fields.get("groups").cloned().unwrap_or_default(),
    );
    let checked = draft
        .form
        .details()
        .and_then(|details| Ok((details, group_lines(&draft.groups)?)));
    let (details, groups) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            draft.error = Some(error);
            return events_page(&state, draft, None).await;
        }
    };
    let event = create_event(&state.db, &details, &groups).await?;
    tracing::info!(event = %event.id, groups = groups.len(), "event created");
    let page = format!("{}?created", paths::path(paths::EVENT, event.id));
    Ok(redirect(&page))
}

/// The group links in the new event's box, a line each: `Climbing club, 2`, or a name
/// alone for two people a sign-up.
fn group_lines(lines: &str) -> Result<Vec<(String, i32)>, String> {
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

/// Makes an event and its group links, all or none.
pub async fn create_event(
    db: &PgPool,
    details: &Details,
    groups: &[(String, i32)],
) -> sqlx::Result<Event> {
    let mut tx = db.begin().await?;
    let event = event::create(&mut *tx, details).await?;
    for (name, max_party_size) in groups {
        group::create(&mut *tx, event.id, name, *max_party_size).await?;
    }
    tx.commit().await?;
    Ok(event)
}

/// The event and its group links as a file, to keep, copy or import as a new event.
pub(super) async fn export(
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
    let (form, groups) = match read {
        Ok(read) => read,
        Err(err) => return file_error(&state, file, file_error_text(err)).await,
    };
    // Filled in even when something needs fixing, which the form then says.
    let error = form
        .details()
        .and_then(|_| group_lines(&groups).map(drop))
        .err();
    let file_notice = filled(&text.imported_notice, "title", form.title());
    let draft = Draft {
        error,
        file_notice: Some(file_notice),
        ..Draft::new(form, groups)
    };
    events_page(&state, draft, None).await
}

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

fn file_error_text(err: FileError) -> String {
    let text = lang::text();
    match err {
        FileError::Syntax(error) => filled(&text.error_event_file, "error", error),
        FileError::Unversioned => fill(
            &text.error_file_unversioned,
            &[("api", &API_VERSION), ("kind", &KIND)],
        ),
        FileError::Version(found) => fill(
            &text.error_file_version,
            &[("found", &found), ("api", &API_VERSION)],
        ),
        FileError::Kind(found) => filled(&text.error_file_kind, "found", found),
        FileError::Language { field, code } => {
            let codes: Vec<&str> = Lang::ALL.iter().map(|lang| lang.code()).collect();
            fill(
                &text.error_file_language,
                &[
                    ("field", &field),
                    ("code", &code),
                    ("codes", &codes.join(", ")),
                ],
            )
        }
    }
}

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
pub(super) async fn details(
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

pub(super) async fn edit(
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

/// Deletes the event with everything in it.
pub(super) async fn delete(
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

/// `innby event list`: each event's id, start and title, the soonest first.
pub async fn event_list(db: &PgPool) -> anyhow::Result<String> {
    let mut out = String::new();
    for event in event::list(db).await? {
        let details = &event.details;
        out.push_str(&format!(
            "{}  {}  {}\n",
            event.id,
            details.starts_at.format("%Y-%m-%d %H:%M %Z"),
            details.title(Lang::EnGb)
        ));
    }
    Ok(out)
}

/// `innby event export`: the event's file.
pub async fn export_event(db: &PgPool, id: Uuid) -> anyhow::Result<String> {
    let event = event::find(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no event {id}; `innby event list` shows them"))?;
    let groups = group::list(db, id).await?;
    Ok(EventFile::new(&event.details, &groups).to_toml())
}

/// `innby event import`: a new event from a file, checked as the admin's form checks
/// it, in English. Says what was made.
pub async fn import_event(db: &PgPool, file: &str) -> anyhow::Result<String> {
    lang::within(Lang::EnGb, async {
        let (form, groups) = EventFile::parse(file)
            .and_then(EventFile::into_form)
            .map_err(|err| anyhow::anyhow!(file_error_text(err)))?;
        let details = form.details().map_err(anyhow::Error::msg)?;
        let groups = group_lines(&groups).map_err(anyhow::Error::msg)?;
        let event = create_event(db, &details, &groups).await?;
        tracing::info!(event = %event.id, groups = groups.len(), "event imported");
        Ok(format!(
            "Made {} ({}) on {}, with {} group link(s).",
            details.title(Lang::EnGb),
            event.id,
            Lang::EnGb.format_date(details.starts_at),
            groups.len()
        ))
    })
    .await
}

/// The event form as typed, so it can be shown again when something needs fixing.
#[derive(Debug)]
pub(super) struct EventForm {
    title: Texts,
    description: Texts,
    location: String,
    /// `2026-12-12T19:00`, as `<input type="datetime-local">` has it.
    starts_at: String,
    time_zone: String,
    rsvp_deadline: String,
}

/// The title and description in one of the languages besides Bokmål, for the form.
pub(super) struct Translation {
    pub language: &'static str,
    pub title_field: String,
    pub title: String,
    pub description_field: String,
    pub description: String,
}

/// A form field's name for a text in a language: `title` for Bokmål, `title-nn` for
/// Nynorsk and so on.
fn field(base: &str, lang: Lang) -> String {
    match lang {
        Lang::Nb => base.to_owned(),
        lang => format!("{base}-{}", lang.code()),
    }
}

pub(super) const LOCAL_TIME: &str = "%Y-%m-%dT%H:%M";

impl EventForm {
    fn blank() -> Self {
        Self {
            title: Texts::default(),
            description: Texts::default(),
            location: String::new(),
            starts_at: String::new(),
            time_zone: "Europe/Oslo".into(),
            rsvp_deadline: String::new(),
        }
    }

    /// A form filled in from an event file's parts.
    pub(super) fn from_parts(
        title: Texts,
        description: Texts,
        location: String,
        starts_at: String,
        time_zone: String,
        rsvp_deadline: String,
    ) -> Self {
        Self {
            title,
            description,
            location,
            starts_at,
            time_zone,
            rsvp_deadline,
        }
    }

    fn from_details(details: &Details) -> Self {
        Self {
            title: details.title.clone(),
            description: details.description.clone(),
            location: details.location.clone(),
            starts_at: details.starts_at.format(LOCAL_TIME).to_string(),
            time_zone: details.starts_at.timezone().name().to_owned(),
            rsvp_deadline: details
                .rsvp_deadline
                .map(|at| at.format(LOCAL_TIME).to_string())
                .unwrap_or_default(),
        }
    }

    fn from_fields(fields: &HashMap<String, String>) -> Self {
        let get = |name: &str| fields.get(name).map_or("", |value| value.trim());
        let mut form = Self {
            location: get("location").to_owned(),
            starts_at: get("starts_at").to_owned(),
            time_zone: get("time_zone").to_owned(),
            rsvp_deadline: get("rsvp_deadline").to_owned(),
            ..Self::blank()
        };
        for lang in Lang::ALL {
            form.title.set(lang, get(&field("title", lang)));
            form.description.set(lang, get(&field("description", lang)));
        }
        form
    }

    /// The Bokmål title and description, which every other language falls back to.
    pub fn title(&self) -> &str {
        &self.title.nb
    }

    pub fn description(&self) -> &str {
        &self.description.nb
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn starts_at(&self) -> &str {
        &self.starts_at
    }

    pub fn time_zone(&self) -> &str {
        &self.time_zone
    }

    pub fn rsvp_deadline(&self) -> &str {
        &self.rsvp_deadline
    }

    /// The other languages' fields, in the order of the language menu.
    pub fn translations(&self) -> Vec<Translation> {
        Lang::ALL
            .into_iter()
            .filter(|lang| *lang != Lang::Nb)
            .map(|lang| Translation {
                language: lang.name(),
                title_field: field("title", lang),
                title: self.title.own(lang).to_owned(),
                description_field: field("description", lang),
                description: self.description.own(lang).to_owned(),
            })
            .collect()
    }

    /// The details the form describes, or what's wrong with them.
    pub(super) fn details(&self) -> Result<Details, String> {
        let text = lang::text();
        let fits = |value: &str, max: usize| value.chars().count() <= max;
        let title_ok = Lang::ALL
            .into_iter()
            .all(|lang| fits(self.title.own(lang), MAX_TITLE))
            && !self.title.nb.is_empty();
        if !title_ok {
            return Err(text.error_event_title.clone());
        }
        let description_ok = Lang::ALL
            .into_iter()
            .all(|lang| fits(self.description.own(lang), MAX_DESCRIPTION));
        if !description_ok || !fits(&self.location, MAX_LOCATION) {
            return Err(text.error_event_text.clone());
        }
        let tz: Tz = self
            .time_zone
            .parse()
            .map_err(|_| text.error_time_zone.clone())?;
        let starts_at =
            local_time(&self.starts_at, tz).ok_or_else(|| text.error_event_time.clone())?;
        let rsvp_deadline = match self.rsvp_deadline.as_str() {
            "" => None,
            deadline => {
                Some(local_time(deadline, tz).ok_or_else(|| text.error_event_deadline.clone())?)
            }
        };
        Ok(Details {
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            starts_at,
            rsvp_deadline,
        })
    }
}

/// A time as `<input type="datetime-local">` sends it, in the event's time zone. A time
/// that the clocks skip, or pass twice, gets the earlier reading.
fn local_time(value: &str, tz: Tz) -> Option<DateTime<Tz>> {
    let value = value.get(..16)?;
    NaiveDateTime::parse_from_str(value, LOCAL_TIME)
        .ok()?
        .and_local_timezone(tz)
        .earliest()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn reads_and_checks_the_event_form() {
        let form = EventForm::from_fields(&fields(&[
            ("title", " Julebord "),
            ("title-en", "Christmas party"),
            ("title-nn", ""),
            ("description", "Velkommen!"),
            ("location", "Storgata 1, Oslo"),
            ("starts_at", "2026-12-12T19:00"),
            ("time_zone", "Europe/Oslo"),
            ("rsvp_deadline", ""),
        ]));
        assert_eq!(form.details().unwrap(), crate::db::event::tests::details());
        // And back into the form it came from.
        let again = EventForm::from_details(&form.details().unwrap());
        assert_eq!(again.starts_at(), "2026-12-12T19:00");
        assert_eq!(again.translations()[1].title, "Christmas party");

        for (field, value) in [
            ("title", ""),
            ("starts_at", "next Friday"),
            ("time_zone", "Europe/Atlantis"),
            ("rsvp_deadline", "soon"),
        ] {
            let mut bad = fields(&[
                ("title", "Julebord"),
                ("starts_at", "2026-12-12T19:00"),
                ("time_zone", "Europe/Oslo"),
            ]);
            bad.insert(field.into(), value.into());
            assert!(EventForm::from_fields(&bad).details().is_err(), "{field}");
        }
    }

    #[sqlx::test]
    async fn the_cli_lists_exports_and_imports_events(db: PgPool) {
        let event = crate::db::event::tests::event(&db).await;
        group::create(&db, event.id, "Naboer", 4).await.unwrap();
        assert!(
            event_list(&db)
                .await
                .unwrap()
                .contains(&event.id.to_string())
        );

        let file = export_event(&db, event.id).await.unwrap();
        let made = import_event(&db, &file).await.unwrap();
        assert!(made.starts_with("Made Christmas party"), "{made}");
        assert!(made.ends_with("with 1 group link(s)."), "{made}");
        let events = event::list(&db).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].details, event.details);
        assert_eq!(
            group::list(&db, events[1].id).await.unwrap()[0].name,
            "Naboer"
        );

        // Mistakes come back in English, and make nothing.
        let err = import_event(&db, &file.replace("Naboer", "<b>"))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("Group links: Line 1"), "{err}");
        assert!(export_event(&db, Uuid::nil()).await.is_err());
        assert_eq!(event::list(&db).await.unwrap().len(), 2);
    }
}
