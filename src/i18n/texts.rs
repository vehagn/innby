use super::Lang;

/// A text the host writes, such as an event's title: in Bokmål, and in other languages
/// if the host gives them. A missing language falls back: Nynorsk and British English to
/// Bokmål, American English and Simplified Chinese to British, and Traditional Chinese
/// to Simplified.
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
        let en_gb = self.en_gb.as_deref().unwrap_or(&self.nb);
        let zh_hans = self.zh_hans.as_deref().unwrap_or(en_gb);
        match lang {
            Lang::Nb => &self.nb,
            Lang::Nn => self.nn.as_deref().unwrap_or(&self.nb),
            Lang::EnGb => en_gb,
            Lang::EnUs => self.en_us.as_deref().unwrap_or(en_gb),
            Lang::ZhHans => zh_hans,
            Lang::ZhHant => self.zh_hant.as_deref().unwrap_or(zh_hans),
        }
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
