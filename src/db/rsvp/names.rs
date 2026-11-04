use serde::{Deserialize, Serialize};

use super::Response;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "name_display", rename_all = "lowercase")]
pub enum NameDisplay {
    Full,
    /// First name and an initial: "Kari N.".
    Partial,
    Hidden,
}

impl NameDisplay {
    pub const ALL: [Self; 3] = [Self::Full, Self::Partial, Self::Hidden];

    /// The form value.
    pub fn code(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Hidden => "hidden",
        }
    }

    /// How a name shows in the list. Until the guest chooses, it's their first name and
    /// an initial, or their full name once they're coming. Declined guests aren't listed.
    pub fn listed(response: Response, chosen: Option<Self>) -> Self {
        match (response, chosen) {
            (Response::Declined, _) => Self::Hidden,
            (_, Some(chosen)) => chosen,
            (Response::Attending, None) => Self::Full,
            (Response::Pending, None) => Self::Partial,
        }
    }

    /// The name as this choice shows it, or `None` for hidden.
    pub fn apply(self, name: &str) -> Option<String> {
        match self {
            Self::Full => Some(name.to_owned()),
            Self::Partial => Some(partial_name(name)),
            Self::Hidden => None,
        }
    }
}

/// The first word and the initial of the last: "Kari Nordmann" becomes "Kari N.". Chinese,
/// Japanese and Korean names come family name first and without spaces, so they keep
/// only the family name: "王小明" becomes "王**".
fn partial_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if let [family, given @ ..] = chars.as_slice()
        && is_cjk(*family)
        && !given.is_empty()
        && !given.iter().any(|c| c.is_whitespace())
    {
        return format!("{family}{}", "*".repeat(given.len()));
    }
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or_default();
    match words.next_back().and_then(|last| last.chars().next()) {
        Some(initial) => format!("{first} {}.", initial.to_uppercase()),
        None => first.to_owned(),
    }
}

/// Collapses whitespace, drops invisible characters and checks the length the database
/// allows. A name needs a letter or digit, so one can't be blank to the eye. Control and
/// bidirectional characters, which could make a name show as someone else's, are refused
/// rather than stripped, since no keyboard produces them.
pub fn normalize_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|&c| !is_invisible(c)).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = name.chars().count();
    let valid = (1..=100).contains(&length)
        && name.chars().any(char::is_alphanumeric)
        && !name.chars().any(|c| c.is_control() || is_bidi_control(c));
    valid.then_some(name)
}

/// Han, kana and Hangul characters.
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
    )
}

/// Characters that take up no space, which a pasted name can carry along. The joiners
/// stay, since emoji and some scripts need them.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}' | '\u{180E}' | '\u{200B}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}'
    )
}

/// Characters that reorder the text around them.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Trims and lower-cases an address and checks that it looks deliverable. Lower-casing
/// the local part is technically wrong, but no real mailbox depends on it, and it keeps
/// one address from being registered twice.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = email.len() <= 254
        && (1..=64).contains(&local.len())
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && !email
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>(),;:\\\"[]".contains(c));
    valid.then_some(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(
            normalize_name("  Kari \t Nordmann ").unwrap(),
            "Kari Nordmann"
        );
        assert_eq!(
            normalize_name(&"ø".repeat(100)).unwrap().chars().count(),
            100
        );
        assert!(normalize_name("   ").is_none());
        assert!(normalize_name(&"a".repeat(101)).is_none());
        assert!(normalize_name("Kari\u{0}").is_none());

        assert_eq!(normalize_name("\u{feff}Ka\u{200b}ri").unwrap(), "Kari");
        assert_eq!(
            normalize_name("Kari 👨\u{200d}👩").unwrap(),
            "Kari 👨\u{200d}👩"
        );
        for blank in ["\u{200b}", "\u{200d}\u{200d}", "- . -"] {
            assert!(normalize_name(blank).is_none(), "{blank:?}");
        }
        for reordering in ["Bob\u{202e}gnp.exe", "\u{2067}Kari"] {
            assert!(normalize_name(reordering).is_none(), "{reordering:?}");
        }
    }

    #[test]
    fn partial_names_keep_the_first_name_and_an_initial() {
        assert_eq!(partial_name("Kari Nordmann"), "Kari N.");
        assert_eq!(partial_name("Kari & Ola  Østby"), "Kari Ø.");
        assert_eq!(partial_name("Per"), "Per");
        assert_eq!(partial_name("anne van der berg"), "anne B.");
        assert_eq!(partial_name("王小明"), "王**");
        assert_eq!(partial_name("김민준"), "김**");
        assert_eq!(partial_name("王"), "王");
    }

    #[test]
    fn lists_names_by_answer_and_choice() {
        use NameDisplay::{Full, Hidden, Partial};
        use Response::{Attending, Declined, Pending};
        assert_eq!(NameDisplay::listed(Pending, None), Partial);
        assert_eq!(NameDisplay::listed(Attending, None), Full);
        assert_eq!(NameDisplay::listed(Pending, Some(Full)), Full);
        assert_eq!(NameDisplay::listed(Attending, Some(Hidden)), Hidden);
        assert_eq!(NameDisplay::listed(Declined, Some(Full)), Hidden);
        assert_eq!(Partial.apply("Kari Nordmann").as_deref(), Some("Kari N."));
        assert_eq!(Hidden.apply("Kari Nordmann"), None);
    }

    #[test]
    fn normalizes_emails() {
        assert_eq!(
            normalize_email("  Kari@Example.COM ").unwrap(),
            "kari@example.com"
        );
        assert_eq!(
            normalize_email("ola+fest@sub.example.no").unwrap(),
            "ola+fest@sub.example.no"
        );
        for bad in [
            "",
            "kari",
            "kari@",
            "@example.com",
            "kari@example",
            "kari@@example.com",
            "kari@exa mple.com",
            "kari@.example.com",
            "kari@example.com.",
            "<kari@example.com>",
        ] {
            assert!(normalize_email(bad).is_none(), "{bad:?}");
        }
        assert!(normalize_email(&format!("{}@example.com", "a".repeat(65))).is_none());
    }
}
