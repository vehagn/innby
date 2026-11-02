use std::{fmt::Display, sync::LazyLock};

use chrono::{DateTime, Locale};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::db::rsvp::{NameDisplay, Response};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "lang", rename_all = "lowercase")]
pub enum Lang {
    Nb,
    En,
}

impl Lang {
    pub const DEFAULT: Lang = Lang::Nb;
    pub const ALL: [Lang; 2] = [Lang::Nb, Lang::En];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Nb => "nb",
            Lang::En => "en",
        }
    }

    /// Parses a primary language subtag. Nynorsk and plain "no" map to bokmål.
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "nb" | "nn" | "no" => Some(Lang::Nb),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// Picks the most preferred supported language from an `Accept-Language` header.
    pub fn negotiate(accept_language: &str) -> Option<Self> {
        let mut best: Option<(f32, Lang)> = None;
        for item in accept_language.split(',') {
            let mut parts = item.split(';');
            let primary = parts.next().unwrap_or_default().trim().split('-').next();
            let quality = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.parse::<f32>().ok());
            if let (Some(lang), Some(quality)) = (primary.and_then(Lang::from_code), quality)
                && quality > 0.0
                && best.is_none_or(|(best_quality, _)| quality > best_quality)
            {
                best = Some((quality, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }

    pub fn other(self) -> Lang {
        match self {
            Lang::Nb => Lang::En,
            Lang::En => Lang::Nb,
        }
    }

    pub fn messages(self) -> &'static Messages {
        match self {
            Lang::Nb => &NB,
            Lang::En => &EN,
        }
    }

    fn file(self) -> &'static str {
        match self {
            Lang::Nb => include_str!("../locales/nb.toml"),
            Lang::En => include_str!("../locales/en.toml"),
        }
    }

    fn locale(self) -> Locale {
        match self {
            Lang::Nb => Locale::nb_NO,
            Lang::En => Locale::en_GB,
        }
    }

    /// A date and time as people write it, e.g. "lørdag 12. desember 2026 kl. 19:00".
    pub fn format_date(self, date: DateTime<Tz>) -> String {
        date.format_localized(&self.messages().date_format, self.locale())
            .to_string()
    }
}

/// A value that exists in every supported language.
#[derive(Debug, Clone)]
pub struct Localized<T> {
    pub nb: T,
    pub en: T,
}

impl<T> Localized<T> {
    pub fn get(&self, lang: Lang) -> &T {
        match lang {
            Lang::Nb => &self.nb,
            Lang::En => &self.en,
        }
    }
}

static NB: LazyLock<Messages> = LazyLock::new(|| Messages::load(Lang::Nb));
static EN: LazyLock<Messages> = LazyLock::new(|| Messages::load(Lang::En));

/// Reads every language's texts now, so a broken file stops the program at start rather
/// than failing the first page that needs it.
pub fn load() {
    for lang in Lang::ALL {
        lang.messages();
    }
}

/// Every user-facing string, from `locales/{lang}.toml`. Each file must have exactly
/// these keys. `{name}` and the like in a text are filled in with `fill`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Messages {
    /// Name of the other language, shown on the language switch.
    pub other_language: String,
    pub privacy_link: String,

    // Event
    /// `chrono` format for dates and times.
    pub date_format: String,
    pub when: String,
    pub place: String,
    pub rsvp_hint: String,

    // RSVP page
    pub greeting: String,
    /// Shown until the guest has answered: what we store and why (GDPR Art. 13/14).
    pub first_visit_notice: String,
    pub keep_link: String,
    pub saved: String,
    pub your_answer: String,
    pub answer_pending: String,
    pub answer_attending: String,
    pub answer_declined: String,
    pub how_many: String,
    /// "{n} person".
    pub people_one: String,
    /// "{n} people".
    pub people_other: String,
    pub attend: String,
    pub decline: String,
    pub reply_by: String,
    pub answers_closed: String,
    pub name_heading: String,
    pub change_name: String,
    pub data_heading: String,
    pub data_text: String,
    pub export: String,
    pub delete_confirm: String,
    pub delete: String,
    pub save_link_title: String,
    pub save_link_text: String,

    // Optional e-mail
    pub email_heading: String,
    pub email_intro: String,
    pub email_label: String,
    /// The consent statement next to the unticked box (GDPR Art. 7).
    pub email_consent: String,
    pub email_add: String,
    pub email_pending: String,
    pub email_pending_hint: String,
    pub email_confirmed: String,
    pub email_remove: String,
    pub email_sent_notice: String,
    pub email_removed_notice: String,
    pub email_confirmed_notice: String,
    pub invalid_email: String,
    pub consent_required: String,
    pub confirm_title: String,
    pub confirm_text: String,
    pub confirm_button: String,
    pub confirm_invalid_text: String,
    pub lost_link: String,
    pub lost_title: String,
    pub lost_text: String,
    pub lost_button: String,
    pub lost_sent_title: String,
    pub lost_sent_text: String,

    // Guest list
    pub guest_list_heading: String,
    /// The button to the guest list, which is on a page of its own.
    pub see_guest_list: String,
    pub back_to_your_page: String,
    pub back_to_sign_up: String,
    pub guest_list_coming: String,
    pub guest_list_pending: String,
    /// "+ {n} more", for guests who'd rather not be listed.
    pub guest_list_more: String,
    pub guest_list_empty: String,
    pub guest_list_note: String,
    pub you: String,
    pub listed_as: String,
    pub listed_hidden: String,
    pub listed_declined: String,
    pub show_instead: String,
    pub name_display_label: String,
    pub display_full: String,
    pub display_partial: String,
    pub display_hidden: String,

    // Group sign-up
    pub join_group: String,
    pub join_heading: String,
    /// What we store and why (GDPR Art. 13), shown where the data is collected.
    pub join_notice: String,
    pub your_name: String,
    pub join_button: String,

    // Errors and outcomes
    pub invalid_party_size: String,
    pub invalid_name: String,
    pub confirm_delete: String,
    pub gone_title: String,
    pub gone_text: String,
    pub deleted_title: String,
    pub deleted_text: String,
    pub to_event: String,
    pub too_many_attempts: String,
    pub not_found_title: String,
    pub not_found_text: String,

    pub mail: Mails,
}

/// The mails, in plain text.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mails {
    /// Under every mail.
    pub footer: String,
    pub confirm_email: MailText,
    pub email_in_use: MailText,
    pub rsvp_link: MailText,
    pub reminder: MailText,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailText {
    pub subject: String,
    pub body: String,
}

impl Messages {
    fn load(lang: Lang) -> Self {
        toml::from_str(lang.file())
            .unwrap_or_else(|err| panic!("locales/{}.toml: {err}", lang.code()))
    }

    /// "1 person", "3 people".
    pub fn people_count(&self, n: i32) -> String {
        let text = if n == 1 {
            &self.people_one
        } else {
            &self.people_other
        };
        fill(text, &[("n", &n)])
    }

    /// "+ 3 more".
    pub fn more_guests(&self, n: impl Display) -> String {
        fill(&self.guest_list_more, &[("n", &n)])
    }

    /// The label for a guest list choice.
    pub fn name_display(&self, display: NameDisplay) -> &str {
        match display {
            NameDisplay::Full => &self.display_full,
            NameDisplay::Partial => &self.display_partial,
            NameDisplay::Hidden => &self.display_hidden,
        }
    }

    /// A guest's answer, e.g. "Coming · 2 people".
    pub fn answer(&self, response: Response, party_size: i32) -> String {
        match response {
            Response::Pending => self.answer_pending.clone(),
            Response::Attending => {
                format!(
                    "{} · {}",
                    self.answer_attending,
                    self.people_count(party_size)
                )
            }
            Response::Declined => self.answer_declined.clone(),
        }
    }
}

/// `text` with each `{key}` replaced by its value, in one pass, so a value that contains
/// braces (a guest's name, say) is never filled in itself. Unknown keys stay as they are.
pub fn fill(text: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let known = after.split_once('}').and_then(|(key, tail)| {
            let (_, value) = values.iter().find(|(name, _)| *name == key)?;
            Some((value, tail))
        });
        match known {
            Some((value, tail)) => {
                filled.push_str(&value.to_string());
                rest = tail;
            }
            None => {
                filled.push('{');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    #[test]
    fn negotiates_by_quality() {
        assert_eq!(Lang::negotiate("en-GB,en;q=0.9,nb;q=0.8"), Some(Lang::En));
        assert_eq!(Lang::negotiate("de-DE, nb;q=0.5, en;q=0.7"), Some(Lang::En));
        assert_eq!(Lang::negotiate("nn-NO, en;q=0.3"), Some(Lang::Nb));
    }

    #[test]
    fn ignores_unsupported_zero_and_malformed_entries() {
        assert_eq!(Lang::negotiate("de, fr;q=0.8"), None);
        assert_eq!(Lang::negotiate("en;q=0, nb;q=0.1"), Some(Lang::Nb));
        assert_eq!(Lang::negotiate("en;q=abc"), None);
        assert_eq!(Lang::negotiate(""), None);
    }

    /// The `{…}` placeholders in a text.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(key, _)| key))
            .collect()
    }

    /// Every text as `(key, nb, en)`, for comparing the languages.
    fn texts() -> Vec<(String, String, String)> {
        fn walk(path: String, nb: &Value, en: &Value, out: &mut Vec<(String, String, String)>) {
            match (nb, en) {
                (Value::Object(nb), Value::Object(en)) => {
                    for (key, nb) in nb {
                        walk(format!("{path}{key}."), nb, &en[key], out);
                    }
                }
                (Value::String(nb), Value::String(en)) => {
                    out.push((path, nb.clone(), en.clone()));
                }
                other => panic!("{path}: {other:?}"),
            }
        }
        let mut out = Vec::new();
        let [nb, en] = Lang::ALL.map(|lang| serde_json::to_value(lang.messages()).unwrap());
        walk(String::new(), &nb, &en, &mut out);
        out
    }

    #[test]
    fn languages_use_the_same_placeholders() {
        let texts = texts();
        assert!(texts.len() > 90, "{}", texts.len());
        for (key, nb, en) in texts {
            assert_eq!(placeholders(&nb), placeholders(&en), "{key}");
        }
    }

    #[test]
    fn unknown_or_missing_keys_are_refused() {
        let nb = include_str!("../locales/nb.toml");
        assert!(toml::from_str::<Messages>(&format!("typo = \"x\"\n{nb}")).is_err());
        let without_greeting = nb.replace("greeting = ", "# greeting = ");
        assert!(toml::from_str::<Messages>(&without_greeting).is_err());
    }

    #[test]
    fn fills_in_each_placeholder_once() {
        let name = "{url}";
        let filled = fill(
            "Hi, {name}! {url} {unknown}",
            &[("name", &name), ("url", &"x")],
        );
        assert_eq!(filled, "Hi, {url}! x {unknown}");
        assert_eq!(fill("{", &[]), "{");
    }

    #[test]
    fn counts_people() {
        assert_eq!(EN.people_count(1), "1 person");
        assert_eq!(NB.people_count(3), "3 personer");
        assert_eq!(EN.people_count(2), "2 people");
        assert_eq!(EN.answer(Response::Attending, 2), "Coming · 2 people");
        assert_eq!(NB.answer(Response::Declined, 2), "Kommer ikke");
        assert_eq!(EN.more_guests(3), "+ 3 more");
        assert_eq!(NB.more_guests(1), "+ 1 til");
    }
}
