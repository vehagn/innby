//! The site's six languages and every text in them. Each language is a module of its
//! own, its file in `locales/`, which says what it is and which languages fill in the
//! texts it leaves out.
//!
//! - `mod.rs`: `Lang`, the list of languages, negotiated from `Accept-Language` or
//!   picked with `?lang=`.
//! - `language`: reading a language's file and filling in its texts from its fallbacks.
//! - `messages`: the texts, as `Messages` (the site, the admin UI's `AdminMessages` and
//!   the mails).
//! - `texts`: `Texts`, a text the host writes in several languages, which falls back
//!   the same way.
//! - `fill`: filling in `{placeholders}` and counting.

mod fill;
mod language;
mod messages;
mod texts;

pub use fill::{bold, bold_count, count, fill, filled};
pub use language::load;
pub use messages::{AdminMessages, Messages};
pub use texts::Texts;

use chrono::DateTime;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// The site's languages, each with its file in `locales/` (`file`). Adding one means a
/// variant here, its file, and a migration adding it to the database's `lang` type.
/// Enum order is `Lang::ALL` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
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

    /// Its own name, on the language switch.
    pub fn name(self) -> &'static str {
        &language::of(self).name
    }

    /// The language tag, for `<html lang>` and `hreflang`.
    pub fn tag(self) -> &'static str {
        &language::of(self).tag
    }

    /// A short mark for the language menu: two letters, or one character for Chinese.
    pub fn badge(self) -> &'static str {
        &language::of(self).badge
    }

    /// The languages its texts fall back to, nearest first.
    pub fn fallbacks(self) -> &'static [Lang] {
        &language::of(self).fallbacks
    }

    /// Parses a `?lang=` value or a language tag such as "nn-NO", as the languages'
    /// files say which tags they answer. The tag loses a part at a time from the end
    /// until a language answers it, as in RFC 4647's lookup: "zh-Hant-TW" tries
    /// "zh-hant-tw", "zh-hant" and "zh".
    pub fn from_code(code: &str) -> Option<Self> {
        let mut tag = code.trim().to_ascii_lowercase().replace('_', "-");
        loop {
            let found = Lang::ALL
                .into_iter()
                .find(|lang| language::of(*lang).serves.contains(&tag));
            if found.is_some() {
                return found;
            }
            tag.truncate(tag.rfind('-')?);
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
        &language::of(self).messages
    }

    /// Its file, compiled in.
    pub(super) fn file(self) -> &'static str {
        match self {
            Lang::Nb => include_str!("../../locales/nb.toml"),
            Lang::Nn => include_str!("../../locales/nn.toml"),
            Lang::EnGb => include_str!("../../locales/en.toml"),
            Lang::EnUs => include_str!("../../locales/en-us.toml"),
            Lang::ZhHans => include_str!("../../locales/zh-hans.toml"),
            Lang::ZhHant => include_str!("../../locales/zh-hant.toml"),
        }
    }

    /// A date and time as people write it, e.g. "lørdag 12. desember 2026 kl. 19:00".
    pub fn format_date(self, date: DateTime<Tz>) -> String {
        let date = date
            .format_localized(&self.messages().date_format, language::of(self).locale)
            .to_string();
        // Some locales' day and month names end in a space ("laurdag ").
        date.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

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
}
