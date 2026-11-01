use chrono::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Nb,
    En,
}

impl Lang {
    pub const DEFAULT: Lang = Lang::Nb;

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

    pub fn locale(self) -> Locale {
        match self {
            Lang::Nb => Locale::nb_NO,
            Lang::En => Locale::en_GB,
        }
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

/// Every user-facing string. A missing translation is a compile error.
pub struct Messages {
    /// Name of the other language, shown on the language switch.
    pub other_language: &'static str,
    /// `chrono` format for the event's start time.
    pub date_format: &'static str,
    pub when: &'static str,
    pub place: &'static str,
    pub rsvp_hint: &'static str,
}

static NB: Messages = Messages {
    other_language: "English",
    date_format: "%A %-d. %B %Y kl. %H:%M",
    when: "Når",
    place: "Hvor",
    rsvp_hint: "Har du fått en invitasjon? Bruk lenken du fikk for å svare.",
};

static EN: Messages = Messages {
    other_language: "Norsk",
    date_format: "%A %-d %B %Y, %H:%M",
    when: "When",
    place: "Where",
    rsvp_hint: "Got an invitation? Use the link you received to respond.",
};

#[cfg(test)]
mod tests {
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
}
