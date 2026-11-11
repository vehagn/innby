//! The event form, on the New event tab for a new event and on an event's Details tab: what
//! the host typed, kept to show again when something needs fixing, and checked into an
//! event's `Details`.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;

use crate::{
    admin::lang,
    db::event::Details,
    i18n::{Lang, LangSet, Texts, filled},
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

/// A date and time as the form keeps one, and event files too:
/// `2026-12-12T19:00`.
pub(super) const LOCAL_TIME: &str = "%Y-%m-%dT%H:%M";

/// The event form as typed.
#[derive(Debug)]
pub(in crate::admin) struct EventForm {
    /// The language the event is written in: the form's first fields are in it, and
    /// the other languages fall back to it.
    lang: Lang,
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
    /// The languages the host ticked for the event's pages: none for all the site's.
    offered: LangSet,
    /// The languages the site offers, which are the ones the form asks about.
    site: LangSet,
}

/// A language the event's pages may be offered in, for the form's ticks.
pub(in crate::admin) struct OfferedChoice {
    pub code: &'static str,
    pub name: &'static str,
    pub checked: bool,
}

/// The texts in one of the languages besides the event's own, for the form.
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

/// A form field's name for a text in a language: `title` for the event's own language,
/// `title-nn` for Nynorsk and so on.
fn field(base: &str, lang: Lang, main: Lang) -> String {
    if lang == main {
        base.to_owned()
    } else {
        format!("{base}-{}", lang.code())
    }
}

/// One choice in the form's list of languages.
pub(in crate::admin) struct LanguageChoice {
    pub code: &'static str,
    pub name: &'static str,
    pub selected: bool,
}

impl EventForm {
    /// A form with nothing in it, in the host's own language.
    pub(super) fn blank() -> Self {
        Self {
            lang: lang::current(),
            title: Texts::default(),
            description: Texts::default(),
            location: String::new(),
            starts_at: String::new(),
            ends_at: String::new(),
            time_zone: "Europe/Oslo".into(),
            rsvp_deadline: String::new(),
            question: Texts::default(),
            choices: Texts::default(),
            offered: LangSet::default(),
            site: LangSet::all(),
        }
    }

    /// The form for a site that offers these languages: it asks about them alone, and
    /// about any other the event already has a text in, so that saving keeps it.
    pub(in crate::admin) fn on_site(mut self, site: LangSet) -> Self {
        self.site = site;
        self
    }

    /// The same, for a form that's already somewhere.
    pub(super) fn set_site(&mut self, site: LangSet) {
        self.site = site;
    }

    /// The form with the languages the event's pages are offered in.
    pub(super) fn offered_in(mut self, offered: LangSet) -> Self {
        self.offered = offered;
        self
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

    /// The form in this language: its first fields are in it.
    pub(super) fn in_language(mut self, lang: Lang) -> Self {
        self.lang = lang;
        self
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
            lang: details.lang,
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
            offered: details.offered,
            ..Self::blank()
        }
    }

    /// The form as posted. Without a language, as from an older page, it's the base.
    pub(super) fn from_fields(fields: &HashMap<String, String>) -> Self {
        let get = |name: &str| fields.get(name).map_or("", |value| value.trim());
        let main = Lang::by_code(get("lang")).unwrap_or_else(Lang::base);
        // A date and a time of day come in a field each, `{name}_date` and
        // `{name}_time`. A time that can't be read stays as typed, to show again.
        let when = |name: &str| {
            let date = get(&format!("{name}_date"));
            let time = get(&format!("{name}_time"));
            if date.is_empty() && time.is_empty() {
                return String::new();
            }
            match time_of_day(time) {
                Some(time) => format!("{date}T{}", time.format("%H:%M")),
                None => format!("{date}T{time}"),
            }
        };
        let mut form = Self {
            lang: main,
            location: get("location").to_owned(),
            starts_at: when("starts_at"),
            ends_at: when("ends_at"),
            time_zone: get("time_zone").to_owned(),
            rsvp_deadline: when("rsvp_deadline"),
            ..Self::blank()
        };
        for &lang in Lang::all() {
            let field = |base| get(&field(base, lang, main)).to_owned();
            form.title.set(lang, &field("title"));
            form.description.set(lang, &field("description"));
            form.question.set(lang, &field("question"));
            form.choices.set(lang, &choice_lines(&field("choices")));
            if !get(&format!("offer-{}", lang.code())).is_empty() {
                form.offered.insert(lang);
            }
        }
        form
    }

    /// The title and description in the event's language, which the others fall back to.
    pub fn title(&self) -> &str {
        self.title.own(self.lang)
    }

    pub fn description(&self) -> &str {
        self.description.own(self.lang)
    }

    /// Whether the form asks about a language: the site offers it, or the event is
    /// written in it or has a text in it already.
    fn asks_about(&self, lang: Lang) -> bool {
        let written = [
            &self.title,
            &self.description,
            &self.question,
            &self.choices,
        ];
        self.site.contains(lang)
            || lang == self.lang
            || written.iter().any(|texts| !texts.own(lang).is_empty())
    }

    /// The languages the event may be written in, with its own chosen.
    pub fn languages(&self) -> Vec<LanguageChoice> {
        Lang::all()
            .iter()
            .copied()
            .filter(|lang| self.site.contains(*lang) || *lang == self.lang)
            .map(|lang| LanguageChoice {
                code: lang.code(),
                name: lang.name(),
                selected: lang == self.lang,
            })
            .collect()
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    /// When it starts, as the form's two fields have it: the date and the time of day.
    pub fn starts_at(&self) -> (&str, &str) {
        date_and_time(&self.starts_at)
    }

    pub fn ends_at(&self) -> (&str, &str) {
        date_and_time(&self.ends_at)
    }

    pub fn time_zone(&self) -> &str {
        &self.time_zone
    }

    pub fn rsvp_deadline(&self) -> (&str, &str) {
        date_and_time(&self.rsvp_deadline)
    }

    /// The question and its choices, a line each, in the event's language.
    pub fn question(&self) -> &str {
        self.question.own(self.lang)
    }

    pub fn choices(&self) -> &str {
        self.choices.own(self.lang)
    }

    /// The languages the event's pages may be offered in, those the site offers, with
    /// the host's ticked.
    pub fn offered(&self) -> Vec<OfferedChoice> {
        self.site
            .iter()
            .map(|lang| OfferedChoice {
                code: lang.code(),
                name: lang.name(),
                checked: self.offered.contains(lang),
            })
            .collect()
    }

    /// Whether the host has ticked any, which the form then opens at.
    pub fn narrowed(&self) -> bool {
        !self.offered.is_empty()
    }

    /// The other languages' fields, in the order of the language menu.
    pub fn translations(&self) -> Vec<Translation> {
        Lang::all()
            .iter()
            .copied()
            .filter(|lang| *lang != self.lang && self.asks_about(*lang))
            .map(|lang| Translation {
                language: lang.name(),
                title_field: field("title", lang, self.lang),
                title: self.title.own(lang).to_owned(),
                description_field: field("description", lang, self.lang),
                description: self.description.own(lang).to_owned(),
                question_field: field("question", lang, self.lang),
                question: self.question.own(lang).to_owned(),
                choices_field: field("choices", lang, self.lang),
                choices: self.choices.own(lang).to_owned(),
            })
            .collect()
    }

    /// The details the form describes, or what's wrong with them, in the page's
    /// language, `now` being when the host asks.
    pub(in crate::admin) fn details(&self, now: DateTime<Utc>) -> Result<Details, String> {
        let text = lang::text();
        let fits = |value: &str, max: usize| value.chars().count() <= max;
        let title_ok = Lang::all()
            .iter()
            .copied()
            .all(|lang| fits(self.title.own(lang), MAX_TITLE))
            && !self.title.own(self.lang).is_empty();
        if !title_ok {
            return Err(text.error_event_title.clone());
        }
        let description_ok = Lang::all()
            .iter()
            .copied()
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
        let texts_ok = Lang::all()
            .iter()
            .copied()
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
            lang: self.lang,
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            starts_at,
            rsvp_deadline,
            ends_at,
            question: self.question.clone(),
            choices: self.choices.clone(),
            offered: self.offered,
        })
    }

    /// The host's question: none at all, or one in the event's language with two choices
    /// or more, and in each other language either nothing of its own or as many choices. Each is
    /// one plain line, the question as long as a title and a choice as a name.
    fn check_question(&self, plain: &dyn Fn(&str) -> bool) -> Result<(), ()> {
        let count = |lang| self.choices.own(lang).lines().count();
        let main = self.lang;
        let asked = !self.question.own(main).is_empty();
        let all_fit = Lang::all().iter().copied().all(|lang| {
            let question = self.question.own(lang);
            let choices = self.choices.own(lang);
            question.chars().count() <= MAX_QUESTION
                && plain(question)
                && choices
                    .lines()
                    .all(|line| line.chars().count() <= MAX_CHOICE && plain(line))
                && [0, count(main)].contains(&count(lang))
                && (asked || (question.is_empty() && choices.is_empty()))
        });
        let enough = !asked || (2..=MAX_CHOICES).contains(&count(main));
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

/// A `LOCAL_TIME` as the form's two fields show it: its date, and its time of day.
fn date_and_time(when: &str) -> (&str, &str) {
    when.split_once('T').unwrap_or((when, ""))
}

/// A time of day as a host types it. The form asks for the 24-hour clock, `19:00`, and
/// takes `19.00`, `1900` and `19` for it too; a time with am or pm, `7 pm` or
/// `7:30 p.m.`, is read as that, for those who'd rather write it so. A browser's own
/// time field isn't used: it asks for am or pm wherever the device's settings do,
/// whatever the page says.
fn time_of_day(typed: &str) -> Option<NaiveTime> {
    let typed: String = typed
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let typed = typed.replace("a.m.", "am").replace("p.m.", "pm");
    let (clock, afternoon) = match (typed.strip_suffix("am"), typed.strip_suffix("pm")) {
        (Some(clock), _) => (clock, Some(false)),
        (_, Some(clock)) => (clock, Some(true)),
        (None, None) => (typed.as_str(), None),
    };
    if clock.is_empty()
        || !clock
            .chars()
            .all(|c| c.is_ascii_digit() || ":.".contains(c))
    {
        return None;
    }
    let (hour, minute) = match clock.split_once([':', '.']) {
        Some((hour, minute)) if minute.len() == 2 => (hour, minute),
        Some(_) => return None,
        // `1900`, or the hour alone.
        None if clock.len() > 2 => clock.split_at(clock.len() - 2),
        None => (clock, "00"),
    };
    let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
    let hour = match afternoon {
        None => hour,
        Some(_) if !(1..=12).contains(&hour) => return None,
        Some(afternoon) => hour % 12 + if afternoon { 12 } else { 0 },
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
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

    /// The form's fields, with each time (`starts_at`, `ends_at`, `rsvp_deadline`)
    /// given as one `LOCAL_TIME` and sent as the form sends it, a date and a time.
    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut fields = HashMap::new();
        for (key, value) in pairs {
            if ["starts_at", "ends_at", "rsvp_deadline"].contains(key) {
                let (date, time) = date_and_time(value);
                fields.insert(format!("{key}_date"), date.to_owned());
                fields.insert(format!("{key}_time"), time.to_owned());
            } else {
                fields.insert(key.to_string(), value.to_string());
            }
        }
        fields
    }

    #[test]
    fn reads_a_time_on_either_clock() {
        let time = |typed| time_of_day(typed).map(|time| time.format("%H:%M").to_string());
        for (typed, read) in [
            ("19:00", "19:00"),
            ("7:05", "07:05"),
            ("19.30", "19:30"),
            ("1930", "19:30"),
            ("19", "19:00"),
            ("00:00", "00:00"),
            ("7 pm", "19:00"),
            ("7:30PM", "19:30"),
            ("7.30 p.m.", "19:30"),
            ("12 am", "00:00"),
            ("12:15 pm", "12:15"),
            ("11 AM", "11:00"),
        ] {
            assert_eq!(time(typed).as_deref(), Some(read), "{typed}");
        }
        for typed in [
            "", "late", "24:00", "19:60", "13 pm", "0 am", "7:5", "pm", "-7",
        ] {
            assert_eq!(time(typed), None, "{typed}");
        }
    }

    #[test]
    fn takes_a_time_typed_with_am_or_pm_and_shows_it_on_the_24_hour_clock() {
        let mut sent = fields(&[
            ("title", "Julebord"),
            ("starts_at", "2054-12-12T7 pm"),
            ("time_zone", "Europe/Oslo"),
        ]);
        let form = EventForm::from_fields(&sent);
        assert_eq!(form.starts_at(), ("2054-12-12", "19:00"));
        assert!(form.details(Utc::now()).is_ok());
        // One that can't be read stays as typed, to fix.
        sent.insert("starts_at_time".into(), "after dinner".into());
        let form = EventForm::from_fields(&sent);
        assert_eq!(form.starts_at(), ("2054-12-12", "after dinner"));
        assert!(form.details(Utc::now()).is_err());
    }

    #[test]
    fn reads_and_checks_the_event_form() {
        let form = EventForm::from_fields(&fields(&[
            ("lang", "nb"),
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
        assert_eq!(again.starts_at(), ("2054-12-12", "19:00"));
        let english = again.translations();
        let english = english.iter().find(|other| other.language == "English");
        assert_eq!(english.unwrap().title, "Christmas party");

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
            // The last of a name counts.
            let bad = fields(&[
                ("lang", "nb"),
                ("title", "Julebord"),
                ("starts_at", "2054-12-12T19:00"),
                ("time_zone", "Europe/Oslo"),
                (field, value),
            ]);
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
            ("2054-12-13", "01:00")
        );
    }

    #[test]
    fn an_event_may_be_written_in_any_language() {
        // In English, with Bokmål among the others: the plain fields are English.
        let form = EventForm::from_fields(&fields(&[
            ("lang", "en"),
            ("title", "Summer party"),
            ("title-nb", "Sommerfest"),
            ("starts_at", "2054-06-20T15:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        let details = form.details(Utc::now()).unwrap();
        assert_eq!(details.lang, Lang::EN);
        assert_eq!(details.title(Lang::EN), "Summer party");
        assert_eq!(details.title(Lang::NB), "Sommerfest");
        // Chinese has none of its own, and falls back in the end to the event's English.
        assert_eq!(details.title(Lang::ZH_HANS), "Summer party");
        let again = EventForm::from_details(&details);
        assert_eq!(again.title(), "Summer party");
        assert!(
            again
                .languages()
                .iter()
                .any(|l| l.code == "en" && l.selected)
        );
        assert!(
            again
                .translations()
                .iter()
                .all(|t| t.title_field != "title-en")
        );

        // It needs a title in its own language; one in Bokmål alone isn't enough.
        let missing = EventForm::from_fields(&fields(&[
            ("lang", "en"),
            ("title-nb", "Sommerfest"),
            ("starts_at", "2054-06-20T15:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        assert!(missing.details(Utc::now()).is_err());
    }

    #[test]
    fn asks_about_the_sites_languages_and_keeps_the_hosts_ticks() {
        // A site that offers Bokmål and English, and an event with a Chinese title
        // from before it stopped offering Chinese.
        let site: LangSet = [Lang::NB, Lang::EN].into_iter().collect();
        let form = EventForm::from_fields(&fields(&[
            ("lang", "nb"),
            ("title", "Sommerfest"),
            ("title-zh-hans", "夏日派对"),
            ("starts_at", "2054-06-20T15:00"),
            ("time_zone", "Europe/Oslo"),
            ("offer-nb", "yes"),
            ("offer-en", "yes"),
        ]))
        .on_site(site);
        let details = form.details(Utc::now()).unwrap();
        assert_eq!(details.offered, site);

        let again = EventForm::from_details(&details).on_site(site);
        assert!(again.narrowed());
        let ticks: Vec<_> = again
            .offered()
            .iter()
            .map(|choice| (choice.code, choice.checked))
            .collect();
        assert_eq!(ticks, [("nb", true), ("en", true)]);
        // It asks for texts in the site's other language, and in the one the event
        // already has a text in, so that saving doesn't lose it.
        let asked: Vec<_> = again
            .translations()
            .iter()
            .map(|other| other.title_field.clone())
            .collect();
        assert_eq!(asked, ["title-en", "title-zh-hans"]);
        // The event may be written in a language of the site's, or the one it is in.
        let written: Vec<_> = again.languages().iter().map(|l| l.code).collect();
        assert_eq!(written, ["nb", "en"]);

        // No ticks is every language the site offers.
        let open = EventForm::from_fields(&fields(&[
            ("lang", "nb"),
            ("title", "Sommerfest"),
            ("starts_at", "2054-06-20T15:00"),
            ("time_zone", "Europe/Oslo"),
        ]));
        assert!(open.details(Utc::now()).unwrap().offered.is_empty());
        assert!(!open.narrowed());
    }

    #[test]
    fn checks_the_hosts_question() {
        let form = |extra: &[(&str, &str)]| {
            let mut all = fields(&[
                ("lang", "nb"),
                ("title", "Sommerfest"),
                ("starts_at", "2054-06-20T15:00"),
                ("time_zone", "Europe/Oslo"),
            ]);
            all.extend(fields(extra));
            EventForm::from_fields(&all).details(Utc::now())
        };
        // None at all is fine, and so is a question with choices, trimmed a line each.
        assert!(form(&[]).unwrap().question(Lang::NB).is_none());
        let asked = form(&[
            ("question", "Hva tar du med?"),
            ("choices", " Salat \r\n\nDessert\n"),
            ("question-en", "What will you bring?"),
            ("choices-en", "Salad\nDessert"),
        ])
        .unwrap();
        assert_eq!(asked.choices.own(Lang::NB), "Salat\nDessert");
        assert_eq!(
            asked.question(Lang::EN),
            Some(("What will you bring?", vec!["Salad", "Dessert"]))
        );
        // Nynorsk has none of its own, so it falls back to Bokmål.
        assert_eq!(asked.question(Lang::NN).unwrap().1, ["Salat", "Dessert"]);

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
