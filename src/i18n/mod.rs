//! The site's six languages and every text in them.
//!
//! - `mod.rs`: `Lang`, negotiated from `Accept-Language` or picked with `?lang=`, with
//!   its locale and date format.
//! - `messages`: the texts in `locales/{code}.toml`, as `Messages` (the site, the admin
//!   UI's `AdminMessages` and the `Mails`).
//! - `texts`: `Texts`, a text the host writes in several languages, with fallbacks.
//! - `fill`: filling in `{placeholders}` and counting.

mod fill;
mod messages;
mod texts;

pub use fill::{bold, bold_count, count, fill, filled};
pub use messages::{AdminMessages, Messages, load};
pub use texts::Texts;

use chrono::{DateTime, Locale};
use chrono_tz::Tz;
use serde::Serialize;

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
        messages::of(self)
    }

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
