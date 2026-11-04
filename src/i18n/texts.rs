//! Texts the host writes, such as an event's title and description: in Bokmål, and in
//! any other language the host gives, falling back as the site's own texts do.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Lang;

/// A text the host writes, such as an event's title: in Bokmål, and in other languages
/// if the host gives them. A language without one falls back as its file says, as the
/// site's own texts do, and in the end to Bokmål.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts {
    pub nb: String,
    pub nn: Option<String>,
    pub en_gb: Option<String>,
    pub en_us: Option<String>,
    pub zh_hans: Option<String>,
    pub zh_hant: Option<String>,
}

impl Texts {
    /// The text in this language, or the one it falls back to.
    pub fn get(&self, lang: Lang) -> &str {
        std::iter::once(lang)
            .chain(lang.fallbacks().iter().copied())
            .map(|lang| self.own(lang))
            .find(|text| !text.is_empty())
            .unwrap_or(&self.nb)
    }

    /// The text written for this language, without falling back; empty if there's none.
    pub fn own(&self, lang: Lang) -> &str {
        match lang {
            Lang::Nb => &self.nb,
            Lang::Nn => self.nn.as_deref().unwrap_or_default(),
            Lang::EnGb => self.en_gb.as_deref().unwrap_or_default(),
            Lang::EnUs => self.en_us.as_deref().unwrap_or_default(),
            Lang::ZhHans => self.zh_hans.as_deref().unwrap_or_default(),
            Lang::ZhHant => self.zh_hant.as_deref().unwrap_or_default(),
        }
    }

    /// Sets the text for one language; a blank one falls back again.
    pub fn set(&mut self, lang: Lang, text: &str) {
        let text = text.trim();
        let other = (!text.is_empty()).then(|| text.to_owned());
        match lang {
            Lang::Nb => text.clone_into(&mut self.nb),
            Lang::Nn => self.nn = other,
            Lang::EnGb => self.en_gb = other,
            Lang::EnUs => self.en_us = other,
            Lang::ZhHans => self.zh_hans = other,
            Lang::ZhHant => self.zh_hant = other,
        }
    }
}

/// Texts as a map by language code, `{"nb": "Julebord", "en": "Christmas party"}`, as the
/// database keeps them: only the languages that have one, and unknown codes left out.
impl Serialize for Texts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            Lang::ALL
                .into_iter()
                .map(|lang| (lang.code(), self.own(lang)))
                .filter(|(_, text)| !text.is_empty()),
        )
    }
}

impl<'de> Deserialize<'de> for Texts {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let by_code = BTreeMap::<String, String>::deserialize(deserializer)?;
        let mut texts = Texts::default();
        for (code, text) in &by_code {
            if let Some(lang) = Lang::from_code(code) {
                texts.set(lang, text);
            }
        }
        Ok(texts)
    }
}
