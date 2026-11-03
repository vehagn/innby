use std::{fmt::Display, sync::LazyLock};

use chrono::{DateTime, Locale};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::db::rsvp::{NameDisplay, Response};

/// The site's languages. Enum order is `Lang::ALL` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "lang", rename_all = "lowercase")]
pub enum Lang {
    /// Norwegian Bokmål.
    Nb,
    /// Norwegian Nynorsk.
    Nn,
    /// British English, "traditional".
    #[serde(rename = "en")]
    #[sqlx(rename = "en")]
    EnGb,
    /// American English, "simplified".
    #[serde(rename = "en-us")]
    #[sqlx(rename = "en-us")]
    EnUs,
    /// Simplified Chinese.
    #[serde(rename = "zh-hans")]
    #[sqlx(rename = "zh-hans")]
    ZhHans,
    /// Traditional Chinese.
    #[serde(rename = "zh-hant")]
    #[sqlx(rename = "zh-hant")]
    ZhHant,
}

impl Lang {
    pub const DEFAULT: Lang = Lang::Nb;
    pub const ALL: [Lang; 6] = [
        Lang::Nb,
        Lang::Nn,
        Lang::EnGb,
        Lang::EnUs,
        Lang::ZhHans,
        Lang::ZhHant,
    ];

    /// The `?lang=` value.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Nb => "nb",
            Lang::Nn => "nn",
            Lang::EnGb => "en",
            Lang::EnUs => "en-us",
            Lang::ZhHans => "zh-hans",
            Lang::ZhHant => "zh-hant",
        }
    }

    /// The language tag, for `<html lang>` and `hreflang`.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::Nb => "nb",
            Lang::Nn => "nn",
            Lang::EnGb => "en-GB",
            Lang::EnUs => "en-US",
            Lang::ZhHans => "zh-Hans",
            Lang::ZhHant => "zh-Hant",
        }
    }

    /// A short mark for the language menu: two letters, or one character for Chinese.
    pub fn badge(self) -> &'static str {
        match self {
            Lang::Nb => "NB",
            Lang::Nn => "NN",
            Lang::EnGb => "GB",
            Lang::EnUs => "US",
            Lang::ZhHans => "简",
            Lang::ZhHant => "繁",
        }
    }

    /// Parses a `?lang=` value or a language tag such as "nn-NO". Plain "no" is Bokmål,
    /// English is British unless it's from the US, and Chinese is Simplified unless it's
    /// Traditional or from Taiwan, Hong Kong or Macau.
    pub fn from_code(code: &str) -> Option<Self> {
        let code = code.trim().to_ascii_lowercase();
        let (language, region) = code.split_once(['-', '_']).unwrap_or((&code, ""));
        match (language, region) {
            ("nb" | "no", _) => Some(Lang::Nb),
            ("nn", _) => Some(Lang::Nn),
            ("en", "us") => Some(Lang::EnUs),
            ("en", _) => Some(Lang::EnGb),
            ("zh", region)
                if region.starts_with("hant") || ["tw", "hk", "mo"].contains(&region) =>
            {
                Some(Lang::ZhHant)
            }
            ("zh", _) => Some(Lang::ZhHans),
            _ => None,
        }
    }

    /// Picks the most preferred supported language from an `Accept-Language` header.
    pub fn negotiate(accept_language: &str) -> Option<Self> {
        let mut best: Option<(f32, Lang)> = None;
        for item in accept_language.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or_default();
            let quality = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.parse::<f32>().ok());
            if let (Some(lang), Some(quality)) = (Lang::from_code(tag), quality)
                && quality > 0.0
                && best.is_none_or(|(best_quality, _)| quality > best_quality)
            {
                best = Some((quality, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }

    /// The languages to offer on the language switch.
    pub fn others(self) -> Vec<Lang> {
        Lang::ALL.into_iter().filter(|&lang| lang != self).collect()
    }

    pub fn messages(self) -> &'static Messages {
        &MESSAGES[self as usize]
    }

    fn file(self) -> &'static str {
        match self {
            Lang::Nb => include_str!("../locales/nb.toml"),
            Lang::Nn => include_str!("../locales/nn.toml"),
            Lang::EnGb => include_str!("../locales/en.toml"),
            Lang::EnUs => include_str!("../locales/en-us.toml"),
            Lang::ZhHans => include_str!("../locales/zh-hans.toml"),
            Lang::ZhHant => include_str!("../locales/zh-hant.toml"),
        }
    }

    fn locale(self) -> Locale {
        match self {
            Lang::Nb => Locale::nb_NO,
            Lang::Nn => Locale::nn_NO,
            Lang::EnGb => Locale::en_GB,
            Lang::EnUs => Locale::en_US,
            Lang::ZhHans => Locale::zh_CN,
            Lang::ZhHant => Locale::zh_TW,
        }
    }

    /// A date and time as people write it, e.g. "lørdag 12. desember 2026 kl. 19:00".
    pub fn format_date(self, date: DateTime<Tz>) -> String {
        let date = date
            .format_localized(&self.messages().date_format, self.locale())
            .to_string();
        // Some locales' day and month names end in a space ("laurdag ").
        date.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// A value that exists in every supported language.
#[derive(Debug, Clone)]
pub struct Localized<T> {
    pub nb: T,
    pub nn: T,
    pub en_gb: T,
    pub en_us: T,
    pub zh_hans: T,
    pub zh_hant: T,
}

impl<T> Localized<T> {
    pub fn get(&self, lang: Lang) -> &T {
        match lang {
            Lang::Nb => &self.nb,
            Lang::Nn => &self.nn,
            Lang::EnGb => &self.en_gb,
            Lang::EnUs => &self.en_us,
            Lang::ZhHans => &self.zh_hans,
            Lang::ZhHant => &self.zh_hant,
        }
    }
}

static MESSAGES: LazyLock<[Messages; 6]> = LazyLock::new(|| Lang::ALL.map(Messages::load));

/// Reads every language's texts now, so a broken file stops the program at start rather
/// than failing the first page that needs it.
pub fn load() {
    for lang in Lang::ALL {
        lang.messages();
    }
}

/// Every user-facing string, from `locales/{code}.toml`. Each file must have exactly
/// these keys. `{name}` and the like in a text are filled in with `fill`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Messages {
    /// The language's own name, on the language switch.
    pub language_name: String,
    /// What the language switch is, for screen readers.
    pub languages: String,
    pub theme: String,
    pub theme_auto: String,
    pub theme_light: String,
    pub theme_dark: String,
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
    /// After your own name on the guest list: "(you)".
    pub you: String,
    pub listed_as: String,
    pub listed_hidden: String,
    pub listed_declined: String,
    pub show_instead: String,
    pub name_display_label: String,
    pub display_full: String,
    pub display_partial: String,
    pub display_hidden: String,
    /// A choice with what others would see: "{label} ({name})".
    pub display_choice: String,

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

    pub admin: AdminMessages,
    pub mail: Mails,
}

/// The host's admin pages. Pairs ending in `_one` and `_other` are for one of something
/// and for any other number, picked with `count`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMessages {
    /// After "Innby" in the header and the page titles.
    pub admin: String,
    pub log_out: String,
    pub log_in: String,
    pub log_in_text: String,
    pub logged_out: String,
    pub user_name: String,
    pub password: String,
    pub wrong_login: String,
    pub too_many_logins: String,
    pub not_found: String,
    pub saved: String,

    // The headcount
    pub coming_one: String,
    pub coming_other: String,
    pub parties_one: String,
    pub parties_other: String,
    pub not_coming: String,
    pub no_answer: String,

    // Invitations
    pub new_invitations: String,
    pub new_invitations_hint: String,
    pub names: String,
    /// Example lines in the empty box, one per line.
    pub names_placeholder: String,
    pub max_per_invitation: String,
    pub create_invitations: String,
    pub all_invitation_links: String,
    pub invitation_links: String,
    pub invitation_links_hint: String,
    pub no_invitations: String,
    pub invitations_created_one: String,
    pub invitations_created_other: String,

    // Group links
    pub group_links: String,
    pub group_links_hint: String,
    pub closed: String,
    pub no_groups: String,
    /// The label of a group's link field, for screen readers.
    pub link_for: String,
    pub new_groups: String,
    /// Example lines in the empty box, one per line.
    pub groups_placeholder: String,
    pub max_per_sign_up: String,
    pub create_groups: String,
    pub groups_created_one: String,
    pub groups_created_other: String,
    pub sign_up_link: String,
    pub group_link_hint_one: String,
    pub group_link_hint_other: String,
    pub group_link: String,
    pub group_new_link_hint: String,
    pub no_sign_ups: String,
    pub group_name: String,
    pub max_for_new_sign_ups: String,
    pub delete_group: String,
    pub group_in_use_hint: String,

    // Guests
    pub guests: String,
    pub no_guests: String,
    pub download_csv: String,
    pub link: String,
    pub guest_link: String,
    pub guest_link_hint: String,
    pub new_link: String,
    pub new_link_hint: String,
    pub edit: String,
    pub name_or_household: String,
    pub max_people: String,
    pub save: String,
    pub delete: String,
    pub delete_guest: String,

    // Reminders
    pub reminders: String,
    pub subscribers_one: String,
    pub subscribers_other: String,
    pub reminders_hint: String,
    pub send_reminders: String,
    pub reminders_queued_one: String,
    pub reminders_queued_other: String,

    // Table columns and facts, also the CSV's header
    pub col_name: String,
    pub col_answer: String,
    pub col_people: String,
    pub col_max_people: String,
    pub col_source: String,
    pub col_guest_list: String,
    pub col_email: String,
    pub col_answered: String,
    pub col_created: String,
    pub col_group: String,
    pub col_signed_up: String,
    pub col_people_each: String,
    pub col_link: String,

    // A guest's row
    pub answer_pending: String,
    pub answer_attending: String,
    pub answer_declined: String,
    pub source_invitation: String,
    pub source_sign_up: String,
    /// A sign-up through a group's link.
    pub source_group: String,
    pub email_unconfirmed: String,
    pub email_confirmed: String,
    pub listed_full: String,
    pub listed_partial: String,
    pub listed_hidden: String,

    // Errors
    pub error_party_size: String,
    pub error_name: String,
    /// A batch line that needs fixing: "Line {n}: {error}".
    pub error_line: String,
    pub error_no_names: String,
    pub error_confirm_delete: String,
    pub error_group_in_use: String,
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

/// `one` if `n` is 1, else `other`, with `{n}` filled in.
pub fn count(n: impl Display, one: &str, other: &str) -> String {
    let n = n.to_string();
    fill(if n == "1" { one } else { other }, &[("n", &n)])
}

/// `count`, with the number as `<strong>n</strong>` and all of it escaped. For
/// templates, with `|safe`.
pub fn bold_count(n: impl Display, one: &str, other: &str) -> String {
    let n = n.to_string();
    bold(if n == "1" { one } else { other }, "n", &n)
}

/// `text` with its one `{key}` filled in. For templates, which escape the result.
pub fn filled(text: &str, key: &str, value: impl Display) -> String {
    fill(text, &[(key, &value)])
}

/// `text` with its one `{key}` filled in as `<strong>value</strong>`, all of it escaped.
/// For templates, with `|safe`.
pub fn bold(text: &str, key: &str, value: &str) -> String {
    let placeholder = format!("{{{key}}}");
    let (before, after) = text.split_once(&placeholder).unwrap_or((text, ""));
    format!(
        "{}<strong>{}</strong>{}",
        escape_html(before),
        escape_html(value),
        escape_html(after)
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&#38;")
        .replace('<', "&#60;")
        .replace('>', "&#62;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
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

    use chrono::TimeZone;
    use serde_json::Value;

    use super::*;

    #[test]
    fn negotiates_by_quality_and_region() {
        assert_eq!(Lang::negotiate("en-GB,en;q=0.9,nb;q=0.8"), Some(Lang::EnGb));
        assert_eq!(
            Lang::negotiate("de-DE, nb;q=0.5, en;q=0.7"),
            Some(Lang::EnGb)
        );
        assert_eq!(Lang::negotiate("nn-NO, nb;q=0.9"), Some(Lang::Nn));
        assert_eq!(Lang::negotiate("no, en;q=0.3"), Some(Lang::Nb));
        assert_eq!(Lang::negotiate("en-US,en;q=0.9"), Some(Lang::EnUs));
        assert_eq!(Lang::negotiate("en-AU"), Some(Lang::EnGb));
        assert_eq!(Lang::negotiate("zh-CN,zh;q=0.9"), Some(Lang::ZhHans));
        assert_eq!(Lang::negotiate("zh-TW"), Some(Lang::ZhHant));
        assert_eq!(Lang::negotiate("zh-Hant-HK"), Some(Lang::ZhHant));
        assert_eq!(Lang::negotiate("zh"), Some(Lang::ZhHans));
    }

    #[test]
    fn ignores_unsupported_zero_and_malformed_entries() {
        assert_eq!(Lang::negotiate("de, fr;q=0.8"), None);
        assert_eq!(Lang::negotiate("en;q=0, nb;q=0.1"), Some(Lang::Nb));
        assert_eq!(Lang::negotiate("en;q=abc"), None);
        assert_eq!(Lang::negotiate(""), None);
    }

    #[test]
    fn codes_and_tags_round_trip() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_code(lang.tag()), Some(lang));
            assert_eq!(lang.others().len(), Lang::ALL.len() - 1);
        }
    }

    #[test]
    fn formats_dates_the_local_way() {
        let party = chrono_tz::Europe::Oslo
            .with_ymd_and_hms(2026, 12, 12, 19, 0, 0)
            .unwrap();
        let dates = Lang::ALL.map(|lang| lang.format_date(party));
        assert_eq!(
            dates,
            [
                "lørdag 12. desember 2026 kl. 19:00",
                "laurdag 12. desember 2026 kl. 19:00",
                "Saturday 12 December 2026 at 7.00 pm",
                "Sat, Dec 12, 2026, 7:00 PM",
                "2026年12月12日（星期六）19:00",
                "2026年12月12日（週六）19:00",
            ]
        );
    }

    /// The `{…}` placeholders in a text.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(key, _)| key))
            .collect()
    }

    /// Every text of `lang` next to the Bokmål one, as `(key, bokmål, text)`.
    fn next_to_bokmal(lang: Lang) -> Vec<(String, String, String)> {
        fn walk(path: String, nb: &Value, other: &Value, out: &mut Vec<(String, String, String)>) {
            match (nb, other) {
                (Value::Object(nb), Value::Object(other)) => {
                    for (key, nb) in nb {
                        walk(format!("{path}{key}."), nb, &other[key], out);
                    }
                }
                (Value::String(nb), Value::String(other)) => {
                    out.push((path, nb.clone(), other.clone()));
                }
                mismatch => panic!("{path}: {mismatch:?}"),
            }
        }
        let [nb, other] =
            [Lang::Nb, lang].map(|lang| serde_json::to_value(lang.messages()).unwrap());
        let mut out = Vec::new();
        walk(String::new(), &nb, &other, &mut out);
        out
    }

    #[test]
    fn languages_use_the_same_placeholders() {
        for lang in Lang::ALL {
            let texts = next_to_bokmal(lang);
            assert!(texts.len() > 90, "{}", texts.len());
            for (key, nb, text) in texts {
                assert_eq!(placeholders(&nb), placeholders(&text), "{lang:?} {key}");
            }
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
    fn bolds_a_value_and_escapes_everything() {
        assert_eq!(
            bold("Hi <{name}> & co", "name", "<b>Kari</b>"),
            "Hi &#60;<strong>&#60;b&#62;Kari&#60;/b&#62;</strong>&#62; &#38; co"
        );
        assert_eq!(filled("{name}，你好！", "name", "王小明"), "王小明，你好！");
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
        let [nb, nn, en, us, zh, _] = Lang::ALL.map(Lang::messages);
        assert_eq!(en.people_count(1), "1 guest");
        assert_eq!(nb.people_count(3), "3 personer");
        assert_eq!(nn.people_count(3), "3 personar");
        assert_eq!(us.people_count(2), "2 people");
        assert_eq!(zh.answer(Response::Attending, 3), "参加 · 3 人");
        assert_eq!(en.answer(Response::Attending, 2), "Attending · 2 guests");
        assert_eq!(us.answer(Response::Attending, 2), "Going · 2 people");
        assert_eq!(nb.answer(Response::Declined, 2), "Kommer ikke");
        assert_eq!(en.more_guests(3), "+ 3 more");
        assert_eq!(nb.more_guests(1), "+ 1 til");
        assert_eq!(
            count(1, &us.admin.coming_one, &us.admin.coming_other),
            "1 person going"
        );
        assert_eq!(
            bold_count(3, &nb.admin.coming_one, &nb.admin.coming_other),
            "<strong>3</strong> personer kommer"
        );
    }
}
