//! Texts the host writes, such as an event's title and description: in the event's own
//! language, and in any other the host gives, falling back as the site's own texts do,
//! and in the end to the event's language.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Lang;

/// A text the host writes, such as an event's title: in the event's language, and in
/// others if the host gives them. A language without one falls back as its file says,
/// as the site's own texts do, and in the end to the event's language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts(BTreeMap<Lang, String>);

impl Texts {
    /// The text in `lang`, or the one it falls back to, and in the end the one in `main`,
    /// the language the host writes the event in.
    pub fn get(&self, lang: Lang, main: Lang) -> &str {
        std::iter::once(lang)
            .chain(lang.fallbacks().iter().copied())
            .map(|lang| self.own(lang))
            .find(|text| !text.is_empty())
            .unwrap_or_else(|| self.own(main))
    }

    /// The text written for this language, without falling back; empty if there's none.
    pub fn own(&self, lang: Lang) -> &str {
        self.0.get(&lang).map_or("", String::as_str)
    }

    /// Sets the text for one language; a blank one falls back again.
    pub fn set(&mut self, lang: Lang, text: &str) {
        match text.trim() {
            "" => self.0.remove(&lang),
            text => self.0.insert(lang, text.to_owned()),
        };
    }
}

/// Texts for tests, as `Texts::from([(Lang::NB, "Julebord")])`.
#[cfg(test)]
impl<const N: usize> From<[(Lang, &str); N]> for Texts {
    fn from(texts: [(Lang, &str); N]) -> Self {
        let mut made = Self::default();
        for (lang, text) in texts {
            made.set(lang, text);
        }
        made
    }
}

/// Texts as a map by language code, `{"nb": "Julebord", "en": "Christmas party"}`, as the
/// database keeps them: only the languages that have one, and unknown codes left out.
impl Serialize for Texts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            Lang::all()
                .iter()
                .map(|&lang| (lang.code(), self.own(lang)))
                .filter(|(_, text)| !text.is_empty()),
        )
    }
}

impl<'de> Deserialize<'de> for Texts {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let by_code = BTreeMap::<String, String>::deserialize(deserializer)?;
        let mut texts = Texts::default();
        for (code, text) in &by_code {
            if let Some(lang) = Lang::by_code(code) {
                texts.set(lang, text);
            }
        }
        Ok(texts)
    }
}
