//! The event form, on the events page for a new event and on an event's Details tab: what
//! the host typed, kept to show again when something needs fixing, and checked into an
//! event's `Details`.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;

use crate::{
    admin::lang,
    db::event::Details,
    i18n::{Lang, Texts},
};

/// The longest title, description and place the form takes.
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 5000;
const MAX_LOCATION: usize = 200;

/// A time as `<input type="datetime-local">` has it, and event files too:
/// `2026-12-12T19:00`.
pub(super) const LOCAL_TIME: &str = "%Y-%m-%dT%H:%M";

/// The event form as typed.
#[derive(Debug)]
pub(in crate::admin) struct EventForm {
    title: Texts,
    description: Texts,
    location: String,
    /// In `LOCAL_TIME`, in `time_zone`.
    starts_at: String,
    time_zone: String,
    rsvp_deadline: String,
}

/// The title and description in one of the languages besides Bokmål, for the form.
pub(in crate::admin) struct Translation {
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

impl EventForm {
    pub(super) fn blank() -> Self {
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

    /// A form filled in from an event, to change it.
    pub(super) fn from_details(details: &Details) -> Self {
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

    /// The form as posted.
    pub(super) fn from_fields(fields: &HashMap<String, String>) -> Self {
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

    /// The details the form describes, or what's wrong with them, in the page's
    /// language.
    pub(in crate::admin) fn details(&self) -> Result<Details, String> {
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

/// A time in `LOCAL_TIME`, in the event's time zone. A time that the clocks skip, or
/// pass twice, gets the earlier reading.
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
