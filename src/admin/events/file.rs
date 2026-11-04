//! An event as a file: its details and its group links, in TOML, to keep, copy or share,
//! and to import as a new event. Never its guests: their names stay in Innby, and go
//! when the event does.
//!
//! ```toml
//! apiVersion = "innby/v1"
//! kind = "Event"
//! starts = "2026-12-12T19:00"
//! time_zone = "Europe/Oslo"
//! location = "Storgata 1, Oslo"
//!
//! [title]
//! nb = "Julebord"
//! en = "Christmas party"
//!
//! [[groups]]
//! name = "Climbing club"
//! max_party_size = 2
//! ```
//!
//! Like a Kubernetes manifest, a file names its format, `apiVersion`, and what it holds,
//! `kind`. These are read first, so a file from a later format, or of another kind, is
//! refused by name rather than misread; a change to the format gets a new version, read
//! alongside the old one.
//!
//! Reading a file only fills in the form for a new event, which checks it as it checks
//! a host's typing; nothing is made until the host creates it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::form::{EventForm, LOCAL_TIME};
use crate::{
    admin::lang,
    db::{event::Details, group::Group},
    i18n::{Lang, Texts, fill, filled},
};

/// The largest event file read, far more than any event needs.
pub const MAX_BYTES: usize = 64 * 1024;
/// The format this Innby writes and reads.
pub const API_VERSION: &str = "innby/v1";
pub const KIND: &str = "Event";

/// What every file says first: its format and what it holds.
#[derive(Deserialize)]
struct TypeMeta {
    #[serde(rename = "apiVersion")]
    api_version: Option<String>,
    kind: Option<String>,
}

/// What an event file says, as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    /// Local time in `time_zone`, as `2026-12-12T19:00`.
    pub starts: String,
    pub time_zone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_by: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    /// By language code: `nb`, which every event has, and any of `nn`, `en`, `en-us`,
    /// `zh-hans` and `zh-hant`.
    #[serde(serialize_with = "in_language_order")]
    pub title: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        serialize_with = "in_language_order"
    )]
    pub description: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupEntry {
    pub name: String,
    #[serde(default = "default_party_size")]
    pub max_party_size: i32,
}

fn default_party_size() -> i32 {
    2
}

/// Texts in the language menu's order, Bokmål first, rather than alphabetically.
fn in_language_order<S: serde::Serializer>(
    texts: &BTreeMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let order = |code: &str| Lang::ALL.iter().position(|lang| lang.code() == code);
    let mut entries: Vec<_> = texts.iter().collect();
    entries.sort_by_key(|(code, _)| order(code));
    serializer.collect_map(entries)
}

/// What's wrong with a file, for the host.
#[derive(Debug, PartialEq, Eq)]
pub enum FileError {
    /// Not TOML, or not an event: the parser's message, with the line.
    Syntax(String),
    /// A text under a key that isn't a language code.
    Language { field: &'static str, code: String },
    /// No `apiVersion` and `kind`, so not an Innby file.
    Unversioned,
    /// A format this Innby doesn't read, such as a later one.
    Version(String),
    /// Something other than an event.
    Kind(String),
}

impl FileError {
    /// What's wrong, in the admin page's language.
    pub fn message(&self) -> String {
        let text = lang::text();
        match self {
            Self::Syntax(error) => filled(&text.error_event_file, "error", error),
            Self::Unversioned => fill(
                &text.error_file_unversioned,
                &[("api", &API_VERSION), ("kind", &KIND)],
            ),
            Self::Version(found) => fill(
                &text.error_file_version,
                &[("found", found), ("api", &API_VERSION)],
            ),
            Self::Kind(found) => filled(&text.error_file_kind, "found", found),
            Self::Language { field, code } => {
                let codes: Vec<&str> = Lang::ALL.iter().map(|lang| lang.code()).collect();
                fill(
                    &text.error_file_language,
                    &[
                        ("field", field),
                        ("code", code),
                        ("codes", &codes.join(", ")),
                    ],
                )
            }
        }
    }
}

const HEADER: &str = "\
# An Innby event: its details and group links, but none of its guests. Import it on the
# admin's Events page, or with `innby event import`, to make a new event from it.
#
# apiVersion and kind say what this file is; leave them as they are. starts and
# answer_by are local times in time_zone, as 2026-12-12T19:00. The title and description
# are by language: nb (which every event needs), nn, en, en-us, zh-hans, zh-hant. A
# group's max_party_size is how many each sign-up may cover.

";

impl EventFile {
    /// The file of an event and its group links.
    pub fn new(details: &Details, groups: &[Group]) -> Self {
        let texts = |texts: &Texts| {
            Lang::ALL
                .into_iter()
                .map(|lang| (lang, texts.own(lang)))
                .filter(|(_, text)| !text.is_empty())
                .map(|(lang, text)| (lang.code().to_owned(), text.to_owned()))
                .collect()
        };
        Self {
            api_version: API_VERSION.into(),
            kind: KIND.into(),
            starts: details.starts_at.format(LOCAL_TIME).to_string(),
            time_zone: details.starts_at.timezone().name().to_owned(),
            answer_by: details
                .rsvp_deadline
                .map(|at| at.format(LOCAL_TIME).to_string()),
            location: details.location.clone(),
            title: texts(&details.title),
            description: texts(&details.description),
            groups: groups
                .iter()
                .map(|group| GroupEntry {
                    name: group.name.clone(),
                    max_party_size: group.max_party_size,
                })
                .collect(),
        }
    }

    /// The file, with a header saying what it is and how to edit it.
    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).expect("an event file is valid TOML");
        format!("{HEADER}{body}")
    }

    pub fn parse(text: &str) -> Result<Self, FileError> {
        let meta: TypeMeta = toml::from_str(text).map_err(|err| syntax(text, &err))?;
        match (meta.api_version, meta.kind) {
            (None, _) | (_, None) => return Err(FileError::Unversioned),
            (Some(version), _) if version != API_VERSION => {
                return Err(FileError::Version(version));
            }
            (_, Some(kind)) if kind != KIND => return Err(FileError::Kind(kind)),
            _ => {}
        }
        toml::from_str(text).map_err(|err| syntax(text, &err))
    }

    /// The form for a new event, filled in from the file, and its group links as lines
    /// for the form's box: `Climbing club, 2`.
    pub fn into_form(self) -> Result<(EventForm, String), FileError> {
        let texts = |field: &'static str, by_code: BTreeMap<String, String>| {
            let mut texts = Texts::default();
            for (code, text) in by_code {
                let lang = Lang::ALL
                    .into_iter()
                    .find(|lang| lang.code() == code)
                    .ok_or(FileError::Language { field, code })?;
                texts.set(lang, &text);
            }
            Ok(texts)
        };
        let form = EventForm::from_parts(
            texts("title", self.title)?,
            texts("description", self.description)?,
            self.location,
            self.starts,
            self.time_zone,
            self.answer_by.unwrap_or_default(),
        );
        let groups = self
            .groups
            .iter()
            .map(|group| format!("{}, {}", group.name, group.max_party_size))
            .collect::<Vec<_>>()
            .join("\n");
        Ok((form, groups))
    }
}

/// The parser's complaint, with the line it's about.
fn syntax(text: &str, err: &toml::de::Error) -> FileError {
    let line = err
        .span()
        .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
    let message = err.message().trim().to_owned();
    FileError::Syntax(match line {
        Some(line) => format!("line {line}: {message}"),
        None => message,
    })
}

/// A short, safe name for an event's file: its Bokmål title in lower-case ASCII, and the
/// year, as `julebord-2026.toml`.
pub fn file_name(details: &Details) -> String {
    let mut name = String::new();
    for c in details.title.nb.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'æ' => "ae".into(),
            'ø' => "o".into(),
            'å' => "a".into(),
            c if c.is_ascii_alphanumeric() => c.to_string(),
            _ => "-".into(),
        };
        if !(c == "-" && (name.is_empty() || name.ends_with('-'))) {
            name.push_str(&c);
        }
    }
    let name = name.trim_end_matches('-');
    let name = if name.is_empty() { "event" } else { name };
    format!("{name}-{}.toml", details.starts_at.format("%Y"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::event::tests::details_with_deadline;

    fn group(name: &str, max_party_size: i32) -> Group {
        Group {
            id: uuid::Uuid::nil(),
            event_id: uuid::Uuid::nil(),
            name: name.into(),
            max_party_size,
            link_nonce: 0,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn an_event_goes_into_a_file_and_back() {
        let mut details = details_with_deadline();
        details
            .description
            .set(Lang::EnUs, "Come one,\ncome all \"friends\"!");
        let groups = [group("Climbing club", 2), group("Naboer", 4)];
        let file = EventFile::new(&details, &groups);
        let toml = file.to_toml();
        assert!(toml.starts_with("# An Innby event"), "{toml}");
        assert!(
            toml.contains("apiVersion = \"innby/v1\"\nkind = \"Event\"\n"),
            "{toml}"
        );
        assert!(
            toml.contains("[title]\nnb = \"Julebord\"\nen = \"Christmas party\""),
            "{toml}"
        );
        assert!(
            toml.contains("[[groups]]\nname = \"Climbing club\""),
            "{toml}"
        );
        assert_eq!(EventFile::parse(&toml).unwrap(), file);

        let (form, lines) = EventFile::parse(&toml).unwrap().into_form().unwrap();
        assert_eq!(lines, "Climbing club, 2\nNaboer, 4");
        assert_eq!(form.details().unwrap(), details);
    }

    #[test]
    fn a_file_needs_little() {
        let file = EventFile::parse(
            "apiVersion = \"innby/v1\"\nkind = \"Event\"\n\
             starts = \"2055-06-20T15:00\"\ntime_zone = \"Europe/Oslo\"\n\
             title = { nb = \"Sommerfest\" }\n[[groups]]\nname = \"Naboer\"",
        )
        .unwrap();
        assert_eq!(file.groups[0].max_party_size, 2);
        let (form, lines) = file.into_form().unwrap();
        assert_eq!(form.details().unwrap().title.nb, "Sommerfest");
        assert_eq!(lines, "Naboer, 2");
    }

    #[test]
    fn says_what_is_wrong_and_where() {
        let err = |text: &str| EventFile::parse(text).unwrap_err();
        let head = "apiVersion = \"innby/v1\"\nkind = \"Event\"\n";
        let FileError::Syntax(message) = err(&format!("{head}starts = \"x\"\ntitel = {{}}")) else {
            panic!()
        };
        assert!(
            message.starts_with("line 4:") && message.contains("titel"),
            "{message}"
        );
        assert!(matches!(err("not toml at all"), FileError::Syntax(_)));
        let wrong_language = EventFile::parse(&format!(
            "{head}starts = \"x\"\ntime_zone = \"y\"\ntitle = {{ nb = \"Fest\", de = \"Party\" }}"
        ))
        .unwrap()
        .into_form()
        .unwrap_err();
        assert_eq!(
            wrong_language,
            FileError::Language {
                field: "title",
                code: "de".into()
            }
        );
    }

    #[test]
    fn reads_only_the_format_and_kind_it_knows() {
        let file = |head: &str| {
            EventFile::parse(&format!(
                "{head}starts = \"2055-06-20T15:00\"\ntime_zone = \"Europe/Oslo\"\n\
                 title = {{ nb = \"Sommerfest\" }}"
            ))
        };
        assert!(file("apiVersion = \"innby/v1\"\nkind = \"Event\"\n").is_ok());
        assert_eq!(file("").unwrap_err(), FileError::Unversioned);
        assert_eq!(
            file("kind = \"Event\"\n").unwrap_err(),
            FileError::Unversioned
        );
        assert_eq!(
            file("apiVersion = \"innby/v2\"\nkind = \"Event\"\nnewThing = 1\n").unwrap_err(),
            FileError::Version("innby/v2".into())
        );
        assert_eq!(
            file("apiVersion = \"innby/v1\"\nkind = \"Guest\"\n").unwrap_err(),
            FileError::Kind("Guest".into())
        );
    }

    #[test]
    fn names_files_after_the_title_and_year() {
        let mut details = details_with_deadline();
        assert_eq!(file_name(&details), "julebord-2054.toml");
        details.title.nb = "  Sommerfest på Bygdøy! ".into();
        assert_eq!(file_name(&details), "sommerfest-pa-bygdoy-2054.toml");
        details.title.nb = "圣诞".into();
        assert_eq!(file_name(&details), "event-2054.toml");
    }
}
