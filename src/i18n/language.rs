//! Each language is one file in `locales/`, a module of its own. Its `language` table says
//! what the language is called, how to recognise and show it, and which languages fill in
//! what the file leaves out; everything else in the file is its texts.
//!
//! A text a file leaves out comes from its `fallback` languages, in order, and from theirs
//! in turn. Bokmål falls back to nothing, so it has every text. A file can't have a text
//! Bokmål lacks, and a language can't fall back to itself, however roundabout.

use std::sync::LazyLock;

use chrono::Locale;
use serde::Deserialize;
use toml::{Table, Value};

use super::{Lang, Messages};

/// A language's `language` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct About {
    /// Its own name, on the language switch.
    name: String,
    /// The language tag, for `<html lang>` and `hreflang`.
    tag: String,
    /// A short mark for the language menu.
    badge: String,
    /// The `chrono` locale for day and month names, e.g. `nn_NO`.
    locale: String,
    /// The `Accept-Language` tags it answers, lower-case, matched as RFC 4647's lookup
    /// does: `zh-Hant-TW` tries `zh-hant-tw`, then `zh-hant`, then `zh`.
    serves: Vec<String>,
    /// Where the texts this file leaves out come from, in order.
    #[serde(default)]
    fallback: Vec<Lang>,
}

/// A language, read from its file, with its texts filled in from its fallbacks.
pub(super) struct Language {
    pub name: String,
    pub tag: String,
    pub badge: String,
    pub locale: Locale,
    pub serves: Vec<String>,
    /// Every language its texts come from, nearest first, without itself.
    pub fallbacks: Vec<Lang>,
    pub messages: Messages,
}

static LANGUAGES: LazyLock<[Language; 6]> =
    LazyLock::new(|| Lang::ALL.map(|lang| Language::load(lang, &|lang| lang.file().to_owned())));

/// The language, loaded.
pub(super) fn of(lang: Lang) -> &'static Language {
    &LANGUAGES[lang as usize]
}

/// Reads every language now, so a broken file stops the program at start rather than
/// failing the first page that needs it.
pub fn load() {
    LazyLock::force(&LANGUAGES);
}

impl Language {
    /// Loads a language, reading files with `file`, so tests can give their own.
    fn load(lang: Lang, file: &dyn Fn(Lang) -> String) -> Self {
        let about = about(lang, file);
        let texts = texts(lang, file, &mut Vec::new());
        let messages = Texts(texts)
            .try_into()
            .unwrap_or_else(|err| panic!("locales/{}.toml: {err}", lang.code()));
        Self {
            locale: Locale::try_from(about.locale.as_str()).unwrap_or_else(|_| {
                panic!("locales/{}.toml: no locale {}", lang.code(), about.locale)
            }),
            fallbacks: fallbacks(lang, file),
            name: about.name,
            tag: about.tag,
            badge: about.badge,
            serves: about.serves,
            messages,
        }
    }
}

fn parse(lang: Lang, file: &dyn Fn(Lang) -> String) -> Table {
    file(lang)
        .parse()
        .unwrap_or_else(|err| panic!("locales/{}.toml: {err}", lang.code()))
}

fn about(lang: Lang, file: &dyn Fn(Lang) -> String) -> About {
    let table = parse(lang, file)
        .remove("language")
        .unwrap_or_else(|| panic!("locales/{}.toml: no language table", lang.code()));
    table
        .try_into()
        .unwrap_or_else(|err| panic!("locales/{}.toml: language: {err}", lang.code()))
}

/// Every language `lang`'s texts come from, nearest first: its fallbacks in order, each
/// followed by its own.
fn fallbacks(lang: Lang, file: &dyn Fn(Lang) -> String) -> Vec<Lang> {
    let mut out = Vec::new();
    for fallback in about(lang, file).fallback {
        for next in std::iter::once(fallback).chain(fallbacks(fallback, file)) {
            if next != lang && !out.contains(&next) {
                out.push(next);
            }
        }
    }
    out
}

/// The texts of `lang`'s file, with what it leaves out filled in from its fallbacks.
/// `path` is the languages on the way here, to catch a language falling back to itself.
fn texts(lang: Lang, file: &dyn Fn(Lang) -> String, path: &mut Vec<Lang>) -> Table {
    if path.contains(&lang) {
        let path: Vec<&str> = path.iter().chain([&lang]).map(|lang| lang.code()).collect();
        panic!(
            "locales: a language falls back to itself: {}",
            path.join(" → ")
        );
    }
    path.push(lang);
    let mut table = parse(lang, file);
    table.remove("language");
    for fallback in about(lang, file).fallback {
        fill_in(&mut table, texts(fallback, file, path));
    }
    path.pop();
    table
}

/// Adds what `table` lacks from `from`, table by table.
fn fill_in(table: &mut Table, from: Table) {
    for (key, value) in from {
        match (table.get_mut(&key), value) {
            (Some(Value::Table(mine)), Value::Table(theirs)) => fill_in(mine, theirs),
            (Some(_), _) => {}
            (None, value) => {
                table.insert(key, value);
            }
        }
    }
}

/// A language's texts, filled in, before they're checked against `Messages`.
struct Texts(Table);

impl TryFrom<Texts> for Messages {
    type Error = toml::de::Error;

    fn try_from(Texts(table): Texts) -> Result<Self, Self::Error> {
        Value::Table(table).try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real files, but with these in place of some of them.
    fn files(replaced: &[(Lang, String)]) -> impl Fn(Lang) -> String {
        let replaced = replaced.to_vec();
        move |lang| {
            replaced
                .iter()
                .find(|(other, _)| *other == lang)
                .map_or_else(|| lang.file().to_owned(), |(_, file)| file.clone())
        }
    }

    const NYNORSK: &str = r#"
        language.name = "Nynorsk"
        language.tag = "nn"
        language.badge = "NN"
        language.locale = "nn_NO"
        language.serves = ["nn"]
        language.fallback = ["nb"]
        greeting = "Hei, {name}!"
        [admin]
        log_in = "Logg inn"
    "#;

    #[test]
    fn a_file_leaves_out_what_its_fallback_has() {
        let nn = Language::load(Lang::Nn, &files(&[(Lang::Nn, NYNORSK.into())]));
        let nb = &of(Lang::Nb).messages;
        assert_eq!(nn.messages.greeting, "Hei, {name}!");
        assert_eq!(nn.messages.admin.log_in, "Logg inn");
        // From Bokmål: a text, an admin text, and a mail.
        assert_eq!(nn.messages.when, nb.when);
        assert_eq!(nn.messages.admin.log_out, nb.admin.log_out);
        assert_eq!(nn.messages.mail.reminder.subject, nb.mail.reminder.subject);
        assert_eq!(nn.fallbacks, [Lang::Nb]);
    }

    #[test]
    fn fallbacks_follow_each_language_in_turn() {
        let fallbacks = |lang: Lang| of(lang).fallbacks.clone();
        assert_eq!(fallbacks(Lang::Nb), []);
        assert_eq!(fallbacks(Lang::EnUs), [Lang::EnGb, Lang::Nb]);
        assert_eq!(
            fallbacks(Lang::ZhHant),
            [Lang::ZhHans, Lang::EnGb, Lang::Nb]
        );
    }

    #[test]
    #[should_panic(expected = "falls back to itself: nb → nn → nb")]
    fn a_language_cannot_fall_back_to_itself() {
        let nb = Lang::Nb.file().replacen(
            "language.serves",
            "language.fallback = [\"nn\"]\nlanguage.serves",
            1,
        );
        Language::load(
            Lang::Nb,
            &files(&[(Lang::Nn, NYNORSK.into()), (Lang::Nb, nb)]),
        );
    }

    #[test]
    #[should_panic(expected = "typo")]
    fn a_text_bokmal_lacks_is_refused() {
        let nn = format!("{NYNORSK}\n[mail]\ntypo = \"x\"");
        Language::load(Lang::Nn, &files(&[(Lang::Nn, nn)]));
    }

    #[test]
    #[should_panic(expected = "missing field `greeting`")]
    fn bokmal_must_have_every_text() {
        let nb = Lang::Nb.file().replace("\ngreeting = ", "\n# greeting = ");
        Language::load(Lang::Nb, &files(&[(Lang::Nb, nb)]));
    }
}
