//! The event form, on the New event tab for a new event and on an event's Details tab: what
//! the host typed, kept to show again when something needs fixing, and checked into an
//! event's `Details`.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;

use crate::{
    admin::lang,
    db::event::Details,
    i18n::{Lang, Texts, filled},
    janitor::KEEP_AFTER_EVENT,
};

/// The longest title, description and place the form takes.
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 5000;
const MAX_LOCATION: usize = 200;
/// The longest question, and the most choices it may have, each as long as a name.
const MAX_QUESTION: usize = 200;
pub(super) const MAX_CHOICES: usize = 20;
const MAX_CHOICE: usize = 100;

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
    /// The same, or empty for no set end.
    ends_at: String,
    time_zone: String,
    rsvp_deadline: String,
    question: Texts,
    /// A line each.
    choices: Texts,
}

/// The title and description in one of the languages besides Bokmål, for the form.
pub(in crate::admin) struct Translation {
    pub language: &'static str,
    pub title_field: String,
    pub title: String,
    pub description_field: String,
    pub description: String,
    pub question_field: String,
    pub question: String,
    pub choices_field: String,
    pub choices: String,
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
            ends_at: String::new(),
            time_zone: "Europe/Oslo".into(),
            rsvp_deadline: String::new(),
            question: Texts::default(),
            choices: Texts::default(),
        }
    }

    /// A form filled in from an event file's parts.
    pub(super) fn from_parts(
        title: Texts,
        description: Texts,
        location: String,
        starts_at: String,
        ends_at: String,
        time_zone: String,
        rsvp_deadline: String,
    ) -> Self {
        Self {
            title,
            description,
            location,
            starts_at,
            ends_at,
            time_zone,
            rsvp_deadline,
            ..Self::blank()
        }
    }

    /// The form with the host's question and its choices, a line each.
    pub(super) fn with_question(mut self, question: Texts, choices: Texts) -> Self {
        self.question = question;
        self.choices = choices;
        self
    }

    /// A form filled in from an event, to change it.
    pub(super) fn from_details(details: &Details) -> Self {
        Self {
            title: details.title.clone(),
            description: details.description.clone(),
            location: details.location.clone(),
            starts_at: details.starts_at.format(LOCAL_TIME).to_string(),
            ends_at: details
                .ends_at
                .map(|at| at.format(LOCAL_TIME).to_string())
                .unwrap_or_default(),
            time_zone: details.starts_at.timezone().name().to_owned(),
            rsvp_deadline: details
                .rsvp_deadline
                .map(|at| at.format(LOCAL_TIME).to_string())
                .unwrap_or_default(),
            question: details.question.clone(),
            choices: details.choices.clone(),
        }
    }

    /// The form as posted.
    pub(super) fn from_fields(fields: &HashMap<String, String>) -> Self {
        let get = |name: &str| fields.get(name).map_or("", |value| value.trim());
        let mut form = Self {
            location: get("location").to_owned(),
            starts_at: get("starts_at").to_owned(),
            ends_at: get("ends_at").to_owned(),
            time_zone: get("time_zone").to_owned(),
            rsvp_deadline: get("rsvp_deadline").to_owned(),
            ..Self::blank()
        };
        for lang in Lang::ALL {
            form.title.set(lang, get(&field("title", lang)));
            form.description.set(lang, get(&field("description", lang)));
            form.question.set(lang, get(&field("question", lang)));
            form.choices
                .set(lang, &choice_lines(get(&field("choices", lang))));
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

    pub fn ends_at(&self) -> &str {
        &self.ends_at
    }

    pub fn time_zone(&self) -> &str {
        &self.time_zone
    }

    pub fn rsvp_deadline(&self) -> &str {
        &self.rsvp_deadline
    }

    /// The Bokmål question and its choices, a line each.
    pub fn question(&self) -> &str {
        &self.question.nb
    }

    pub fn choices(&self) -> &str {
        &self.choices.nb
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
                question_field: field("question", lang),
                question: self.question.own(lang).to_owned(),
                choices_field: field("choices", lang),
                choices: self.choices.own(lang).to_owned(),
            })
            .collect()
    }

    /// The details the form describes, or what's wrong with them, in the page's
    /// language, `now` being when the host asks.
    pub(in crate::admin) fn details(&self, now: DateTime<Utc>) -> Result<Details, String> {
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
        // Postgres can't store a NUL, and other control characters only come from a
        // crafted request or file. A description keeps its line breaks and tabs.
        let plain = |text: &str| !text.chars().any(char::is_control);
        let lines = |text: &str| {
            !text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        };
        let texts_ok = Lang::ALL
            .into_iter()
            .all(|lang| plain(self.title.own(lang)) && lines(self.description.own(lang)))
            && plain(&self.location);
        if !texts_ok {
            return Err(text.error_event_control.clone());
        }
        self.check_question(&plain)
            .map_err(|()| text.error_event_question.clone())?;
        let tz: Tz = self
            .time_zone
            .parse()
            .map_err(|_| text.error_time_zone.clone())?;
        let starts_at =
            local_time(&self.starts_at, tz).ok_or_else(|| text.error_event_time.clone())?;
        // The janitor deletes an event this long after it starts, so one that started
        // longer ago would go at its next run, with everything made in it meanwhile: an
        // old event file imported without a new date, say.
        if starts_at.with_timezone(&Utc) <= now - KEEP_AFTER_EVENT {
            let days = KEEP_AFTER_EVENT.num_days();
            return Err(filled(&text.error_event_past, "days", days));
        }
        let ends_at = match self.ends_at.as_str() {
            "" => None,
            ends => Some(
                local_time(ends, tz)
                    .filter(|ends| *ends > starts_at)
                    .ok_or_else(|| text.error_event_ends.clone())?,
            ),
        };
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
            ends_at,
            question: self.question.clone(),
            choices: self.choices.clone(),
        })
    }

    /// The host's question: none at all, or a Bokmål question with two choices or more,
    /// and in each other language either nothing of its own or as many choices. Each is
    /// one plain line, the question as long as a title and a choice as a name.
    fn check_question(&self, plain: &dyn Fn(&str) -> bool) -> Result<(), ()> {
        let count = |lang| self.choices.own(lang).lines().count();
        let asked = !self.question.nb.is_empty();
        let all_fit = Lang::ALL.into_iter().all(|lang| {
            let question = self.question.own(lang);
            let choices = self.choices.own(lang);
            question.chars().count() <= MAX_QUESTION
                && plain(question)
                && choices
                    .lines()
                    .all(|line| line.chars().count() <= MAX_CHOICE && plain(line))
                && [0, count(Lang::Nb)].contains(&count(lang))
                && (asked || (question.is_empty() && choices.is_empty()))
        });
        let enough = !asked || (2..=MAX_CHOICES).contains(&count(Lang::Nb));
        if all_fit && enough { Ok(()) } else { Err(()) }
    }
}

/// Choices as typed, a line each, trimmed, without the empty lines.
fn choice_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
            ("starts_at", "2054-12-12T19:00"),
            ("time_zone", "Europe/Oslo"),
            ("rsvp_deadline", ""),
        ]));
        assert_eq!(
            form.details(Utc::now()).unwrap(),
            crate::db::event::tests::details()
        );
        // And back into the form it came from.
        let again = EventForm::from_details(&form.details(Utc::now()).unwrap());
        assert_eq!(again.starts_at(), "2054-12-12T19:00");
        assert_eq!(again.translations()[1].title, "Christmas party");

        for (field, value) in [
            ("title", ""),
            ("title", "Jule\u{0}bord"),
            ("title-en", "Christmas\u{1}party"),
            ("location", "Storgata\n1"),
            ("description", "Velkommen\u{0}!"),
            ("starts_at", "next Friday"),
            ("time_zone", "Europe/Atlantis"),
            ("rsvp_deadline", "soon"),
            ("ends_at", "late"),
            // An end must come after the start.
            ("ends_at", "2054-12-12T19:00"),
            ("ends_at", "2054-12-12T18:00"),
        ] {
            let mut bad = fields(&[
                ("title", "Julebord"),
                ("starts_at", "2054-12-12T19:00"),
                ("time_zone", "Europe/Oslo"),
            ]);
            bad.insert(field.into(), value.into());
            assert!(
                EventForm::from_fields(&bad).details(Utc::now()).is_err(),
                "{field}"
            );
        }
        // A description keeps its line breaks and tabs.
        let lines = EventForm::from_fields(&fields(&[
            ("title", "Julebord"),
            ("description", "Velkommen!\r\n\tKl. 19"),
            ("starts_at", "2054-12-12T19:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        assert!(lines.details(Utc::now()).is_ok());

        // An end, after midnight, goes into the event and back into the form.
        let ending = EventForm::from_fields(&fields(&[
            ("title", "Julebord"),
            ("starts_at", "2054-12-12T19:00"),
            ("ends_at", "2054-12-13T01:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        let details = ending.details(Utc::now()).unwrap();
        assert_eq!(
            details.ends_at.unwrap().to_rfc3339(),
            "2054-12-13T01:00:00+01:00"
        );
        assert_eq!(
            EventForm::from_details(&details).ends_at(),
            "2054-12-13T01:00"
        );
    }

    #[test]
    fn checks_the_hosts_question() {
        let form = |extra: &[(&str, &str)]| {
            let mut all = fields(&[
                ("title", "Sommerfest"),
                ("starts_at", "2054-06-20T15:00"),
                ("time_zone", "Europe/Oslo"),
            ]);
            all.extend(fields(extra));
            EventForm::from_fields(&all).details(Utc::now())
        };
        // None at all is fine, and so is a question with choices, trimmed a line each.
        assert!(form(&[]).unwrap().question(Lang::Nb).is_none());
        let asked = form(&[
            ("question", "Hva tar du med?"),
            ("choices", " Salat \r\n\nDessert\n"),
            ("question-en", "What will you bring?"),
            ("choices-en", "Salad\nDessert"),
        ])
        .unwrap();
        assert_eq!(asked.choices.nb, "Salat\nDessert");
        assert_eq!(
            asked.question(Lang::EnGb),
            Some(("What will you bring?", vec!["Salad", "Dessert"]))
        );
        // Nynorsk has none of its own, so it falls back to Bokmål.
        assert_eq!(asked.question(Lang::Nn).unwrap().1, ["Salat", "Dessert"]);

        let many = (0..=MAX_CHOICES)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let long = "x".repeat(MAX_CHOICE + 1);
        for bad in [
            vec![("question", "Hva tar du med?")],
            vec![("question", "Hva tar du med?"), ("choices", "Salat")],
            vec![("choices", "Salat\nDessert")],
            vec![("question", "Hva?"), ("choices", many.as_str())],
            vec![("question", "Hva?"), ("choices", &format!("Salat\n{long}"))],
            vec![("question", "Hva?\u{1}"), ("choices", "Salat\nDessert")],
            vec![
                ("question", "Hva tar du med?"),
                ("choices", "Salat\nDessert"),
                ("choices-en", "Salad\nDessert\nDrinks"),
            ],
        ] {
            assert!(form(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn refuses_a_start_the_janitor_would_delete_at_once() {
        let event = EventForm::from_fields(&fields(&[
            ("title", "Julebord"),
            ("starts_at", "2054-12-12T19:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        let at = |at: &str| at.parse::<DateTime<Utc>>().unwrap();
        // 19:00 in Oslo is 18:00 UTC; the janitor deletes it 30 days on.
        assert!(event.details(at("2054-12-01T12:00:00Z")).is_ok());
        assert!(event.details(at("2055-01-11T17:59:00Z")).is_ok());
        let error = event.details(at("2055-01-11T18:00:00Z")).unwrap_err();
        assert!(error.contains("30"), "{error}");
    }
}
