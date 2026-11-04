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
