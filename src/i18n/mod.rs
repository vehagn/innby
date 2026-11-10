//! The site's languages and every text in them. Each language is a file in `locales/`,
//! found when Innby is built, which says what it is and which languages fill in the
//! texts it leaves out. Adding a language is adding its file: no list in the code names
//! them.
//!
//! - `mod.rs`: `Lang`, one of the languages found, negotiated from `Accept-Language` or
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

pub use fill::{bold, bold_count, count, escape_html, fill, filled};
pub use language::load;
pub use messages::{AdminMessages, Messages};
pub use texts::Texts;

use std::fmt;

use chrono::DateTime;
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// `FILES` and `NOTICES`, the languages' files by code, from build.rs.
include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// One of the site's languages: a file in `locales/`, by its place among them. The file's
/// name is its code, as in `?lang=en-us`, and how the database and event files name it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Lang(u8);

impl fmt::Debug for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl Lang {
    /// Every language, as the language menu lists them: the base language first, then
    /// the others by their own names.
    pub fn all() -> &'static [Lang] {
        &language::registry().order
    }

    /// The language every other one falls back to in the end, which has every text: the
    /// one whose file names no fallback. It's also the site's when the browser asks for
    /// none it has.
    pub fn base() -> Lang {
        language::registry().order[0]
    }

    /// The language with exactly this code, as `?lang=` and the database give it.
    pub fn by_code(code: &str) -> Option<Lang> {
        let index = FILES.iter().position(|(file, _)| *file == code)?;
        Some(Lang(u8::try_from(index).ok()?))
    }

    /// The `?lang=` value: its file's name.
    pub fn code(self) -> &'static str {
        FILES[usize::from(self.0)].0
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
            let found = Lang::all()
                .iter()
                .copied()
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
        Lang::all()
            .iter()
            .copied()
            .filter(|&lang| lang != self)
            .collect()
    }

    pub fn messages(self) -> &'static Messages {
        &language::of(self).messages
    }

    /// Its file, compiled in.
    pub(super) fn file(self) -> &'static str {
        FILES[usize::from(self.0)].1
    }

    /// Its privacy notice: its own, or the one of the nearest language it falls back to
    /// that has one. The base language must have one.
    pub fn notice(self) -> &'static str {
        std::iter::once(self)
            .chain(self.fallbacks().iter().copied())
            .find_map(|lang| {
                NOTICES
                    .iter()
                    .find(|(code, _)| *code == lang.code())
                    .map(|(_, notice)| *notice)
            })
            .expect("the base language has a privacy notice")
    }

    /// A date and time as people write it, e.g. "lørdag 12. desember 2026 kl. 19:00".
    pub fn format_date(self, date: DateTime<Tz>) -> String {
        self.format(date, &self.messages().date_format)
    }

    /// When an event is: its start, and its end if it has one, e.g. "lørdag 12. desember
    /// 2026 kl. 19:00 til 23:00". The end is a time alone when it's within a day of the
    /// start, as a party that ends at one in the night does: the time can then only be
    /// the one that follows, and the date would fill a phone's screen to say so.
    pub fn format_when(self, starts: DateTime<Tz>, ends: Option<DateTime<Tz>>) -> String {
        let Some(ends) = ends else {
            return self.format_date(starts);
        };
        let t = self.messages();
        let end = if ends - starts < chrono::Duration::days(1) {
            self.format(ends, &t.time_format)
        } else {
            self.format_date(ends)
        };
        fill(
            &t.when_until,
            &[("start", &self.format_date(starts)), ("end", &end)],
        )
    }

    fn format(self, date: DateTime<Tz>, format: &str) -> String {
        let date = date
            .format_localized(format, language::of(self).locale)
            .to_string();
        // Some locales' day and month names end in a space ("laurdag ").
        date.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// As its code, `"en-us"`, in event files, exports and the texts' maps.
impl Serialize for Lang {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Lang::by_code(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("no language {code:?}")))
    }
}

/// As its code, in a text column. A code whose file has since been taken away reads as
/// the base language, so a language can go without breaking the guests who chose it.
impl sqlx::Type<sqlx::Postgres> for Lang {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <&str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Lang {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(self.code(), buf)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Lang {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let code = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Lang::by_code(code).unwrap_or_else(Lang::base))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    #[test]
    fn negotiates_by_quality_and_region() {
        assert_eq!(Lang::negotiate("en-GB,en;q=0.9,nb;q=0.8"), Some(Lang::EN));
        assert_eq!(Lang::negotiate("de-DE, nb;q=0.5, en;q=0.7"), Some(Lang::EN));
        assert_eq!(Lang::negotiate("nn-NO, nb;q=0.9"), Some(Lang::NN));
        assert_eq!(Lang::negotiate("no, en;q=0.3"), Some(Lang::NB));
        assert_eq!(Lang::negotiate("en-US,en;q=0.9"), Some(Lang::EN_US));
        assert_eq!(Lang::negotiate("en-AU"), Some(Lang::EN));
        assert_eq!(Lang::negotiate("zh-CN,zh;q=0.9"), Some(Lang::ZH_HANS));
        assert_eq!(Lang::negotiate("zh-TW"), Some(Lang::ZH_HANT));
        assert_eq!(Lang::negotiate("zh-Hant-HK"), Some(Lang::ZH_HANT));
        assert_eq!(Lang::negotiate("zh"), Some(Lang::ZH_HANS));
    }

    #[test]
    fn ignores_unsupported_zero_and_malformed_entries() {
        assert_eq!(Lang::negotiate("de, fr;q=0.8"), None);
        assert_eq!(Lang::negotiate("en;q=0, nb;q=0.1"), Some(Lang::NB));
        assert_eq!(Lang::negotiate("en;q=abc"), None);
        assert_eq!(Lang::negotiate(""), None);
    }

    #[test]
    fn codes_and_tags_round_trip() {
        for &lang in Lang::all() {
            assert_eq!(Lang::by_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_code(lang.tag()), Some(lang));
            assert_eq!(lang.others().len(), Lang::all().len() - 1);
        }
        assert_eq!(Lang::by_code("xx"), None);
    }

    #[test]
    fn formats_dates_the_local_way() {
        let party = chrono_tz::Europe::Oslo
            .with_ymd_and_hms(2026, 12, 12, 19, 0, 0)
            .unwrap();
        for (lang, date) in [
            (Lang::NB, "lørdag 12. desember 2026 kl. 19:00"),
            (Lang::NN, "laurdag 12. desember 2026 kl. 19:00"),
            (Lang::EN, "Saturday 12 December 2026 at 7.00 pm"),
            (Lang::EN_US, "Sat, Dec 12, 2026, 7:00 PM"),
            (Lang::ZH_HANS, "2026年12月12日（星期六）19:00"),
            (Lang::ZH_HANT, "2026年12月12日（週六）19:00"),
        ] {
            assert_eq!(lang.format_date(party), date, "{lang:?}");
        }
    }

    #[test]
    fn lists_the_base_language_first_then_by_name() {
        assert_eq!(Lang::base(), Lang::NB);
        let codes: Vec<&str> = Lang::all().iter().map(|lang| lang.code()).collect();
        assert_eq!(codes, ["nb", "en-us", "en", "nn", "zh-hans", "zh-hant"]);
    }

    #[test]
    fn says_when_an_event_ends_the_short_way_on_the_same_day() {
        let at = |day, hour| {
            chrono_tz::Europe::Oslo
                .with_ymd_and_hms(2026, 12, day, hour, 0, 0)
                .unwrap()
        };
        assert_eq!(
            Lang::EN.format_when(at(12, 19), None),
            "Saturday 12 December 2026 at 7.00 pm"
        );
        assert_eq!(
            Lang::EN.format_when(at(12, 19), Some(at(12, 23))),
            "Saturday 12 December 2026 at 7.00 pm until 11.00 pm"
        );
        assert_eq!(
            Lang::NB.format_when(at(12, 19), Some(at(13, 1))),
            "lørdag 12. desember 2026 kl. 19:00 til 01:00"
        );
        // A day or more later, the end needs its date.
        assert_eq!(
            Lang::NB.format_when(at(12, 19), Some(at(13, 19))),
            "lørdag 12. desember 2026 kl. 19:00 til søndag 13. desember 2026 kl. 19:00"
        );
        assert_eq!(
            Lang::EN_US.format_when(at(12, 19), Some(at(12, 23))),
            "Sat, Dec 12, 2026, 7:00 PM – 11:00 PM"
        );
        assert_eq!(
            Lang::ZH_HANS.format_when(at(12, 19), Some(at(12, 23))),
            "2026年12月12日（星期六）19:00至23:00"
        );
    }
}
