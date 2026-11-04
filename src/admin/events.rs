//! The host's events: the list of them, a new one, and each one's details.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    lang,
    page::{AdminState, DeleteForm, PageError, SavedQuery, load_event, redirect},
};
use crate::{
    db::{
        event::{self, Details, Event},
        rsvp::{self, Headcount},
    },
    http::{render, render_with},
    i18n::{Lang, Texts},
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
    form: EventForm,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    /// An event was just deleted.
    deleted: Option<String>,
}

/// Every event, the soonest first, and the form for a new one.
pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, PageError> {
    let notice = query.deleted.map(|_| lang::text().event_deleted.clone());
    events_page(&state, EventForm::blank(), notice, None).await
}

async fn events_page(
    state: &AdminState,
    form: EventForm,
    notice: Option<String>,
    error: Option<String>,
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
    let status = error_status(&error);
    let page = EventsPage {
        events,
        form,
        notice,
        error,
    };
    Ok(render_with(status, page))
}

fn error_status(error: &Option<String>) -> StatusCode {
    match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    }
}

/// Creates an event and opens its guests, where invitations start.
pub(super) async fn create(
    State(state): State<AdminState>,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let form = EventForm::from_fields(&fields);
    let details = match form.details() {
        Ok(details) => details,
        Err(error) => return events_page(&state, form, None, Some(error)).await,
    };
    let event = event::create(&state.db, &details).await?;
    tracing::info!(event = %event.id, "event created");
    let page = format!("{}?created", paths::path(paths::EVENT, event.id));
    Ok(redirect(&page))
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

/// The event form as typed, so it can be shown again when something needs fixing.
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

const LOCAL_TIME: &str = "%Y-%m-%dT%H:%M";

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
                language: &lang.messages().language_name,
                title_field: field("title", lang),
                title: self.title.own(lang).to_owned(),
                description_field: field("description", lang),
                description: self.description.own(lang).to_owned(),
            })
            .collect()
    }

    /// The details the form describes, or what's wrong with them.
    fn details(&self) -> Result<Details, String> {
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
}
