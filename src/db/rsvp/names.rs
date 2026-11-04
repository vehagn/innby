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

/// What a name is for: a person or household, or a group, whose name may have digits
/// ("Class of 2010").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    Person,
    Group,
}

/// Collapses whitespace, drops invisible characters and checks the length the database
/// allows. A name is letters, in any script, with their accents, spaces, and the
/// punctuation names have: hyphens, apostrophes, full stops, commas, middle dots, and `&`
/// for a household ("Kari & Ola"). Anything else, such as `<`, `;`, quotes, emoji or digits
/// (but digits in a group's name), is refused rather than stripped, so the host or
/// guest sees why. So are control and bidirectional characters, which could make a name
/// show as someone else's. A name needs a letter, so one can't be blank to the eye.
pub fn normalize_name(name: &str, kind: NameKind) -> Option<String> {
    let name: String = name.chars().filter(|&c| !is_invisible(c)).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = name.chars().count();
    let valid = (1..=100).contains(&length)
        && name.chars().any(char::is_alphabetic)
        && name.chars().all(|c| is_name_char(c, kind));
    valid.then_some(name)
}

/// The punctuation a name may have, besides letters and spaces.
const NAME_PUNCTUATION: &[char] = &[
    '-', '\u{2010}', // hyphens
    '\'', '\u{2019}', '\u{02BC}', // apostrophes: O'Brien, D’Angelo
    '.',        // initials: J. R. R. Tolkien
    ',',        // family name first: Nordmann, Kari
    '\u{00B7}', '\u{30FB}', // middle dots, between a given and a family name
    '&',        // a household: Kari & Ola
    '\u{200C}', '\u{200D}', // zero-width (non-)joiners, part of spelling in some scripts
];

fn is_name_char(c: char, kind: NameKind) -> bool {
    c == ' '
        || c.is_alphabetic()
        || is_mark(c)
        || NAME_PUNCTUATION.contains(&c)
        || (kind == NameKind::Group && c.is_numeric())
}

/// Combining marks: accents typed as separate characters, and the vowel signs, viramas
/// and tone marks of scripts such as Devanagari and Thai that aren't letters themselves.
/// The standard library has no general category, so these are the blocks' ranges.
fn is_mark(c: char) -> bool {
    let brahmic_or_thai = matches!(c, '\u{0900}'..='\u{0DFF}' | '\u{0E00}'..='\u{0EFF}')
        && !c.is_numeric()
        && !matches!(
            c,
            '\u{0964}'
                | '\u{0965}'
                | '\u{0970}'
                | '\u{0E3F}'
                | '\u{0E4F}'
                | '\u{0E5A}'
                | '\u{0E5B}'
        );
    brahmic_or_thai
        || matches!(
            c,
            '\u{0300}'..='\u{036F}' // combining diacritical marks
                | '\u{0483}'..='\u{0489}' // Cyrillic
                | '\u{0591}'..='\u{05BD}' | '\u{05BF}' | '\u{05C1}'..='\u{05C2}'
                | '\u{05C4}'..='\u{05C5}' | '\u{05C7}' // Hebrew points
                | '\u{0610}'..='\u{061A}' | '\u{064B}'..='\u{065F}' | '\u{0670}' // Arabic
                | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}'
                | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}' // more diacritics
                | '\u{3099}'..='\u{309A}' // Japanese voicing marks
        )
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
        let person = |name: &str| normalize_name(name, NameKind::Person);
        assert_eq!(person("  Kari \t Nordmann ").unwrap(), "Kari Nordmann");
        assert_eq!(person(&"ø".repeat(100)).unwrap().chars().count(), 100);
        assert!(person("   ").is_none());
        assert!(person(&"a".repeat(101)).is_none());
        assert_eq!(person("\u{feff}Ka\u{200b}ri").unwrap(), "Kari");
        for blank in ["\u{200b}", "\u{200d}\u{200d}", "- . -", "&"] {
            assert!(person(blank).is_none(), "{blank:?}");
        }
    }

    #[test]
    fn names_are_letters_and_name_punctuation() {
        let person = |name: &str| normalize_name(name, NameKind::Person);
        for name in [
            "Kari & Ola Nordmann",
            "Anne-Marie O'Brien",
            "D’Angelo",
            "J. R. R. Tolkien",
            "Ståle Ødegård",
            "Rene\u{301}e", // é typed as e and an accent
            "王小明",
            "やまだ・たろう",
            "김민준",
            "Владимир",
            "مُحَمَّد",
            "לאה",
            "लक्ष्मी", // with a virama
            "ก่อ",    // with a tone mark
            "约翰·史密斯",
            "Nordmann, Kari",
        ] {
            assert_eq!(person(name).as_deref(), Some(name), "{name:?}");
        }
        for name in [
            "<script>alert(1)</script>",
            "Kari 👨\u{200d}👩",
            "Kari2",
            "1234",
            "Kari; DROP TABLE rsvp",
            "Robert\"); --",
            "@kari",
            "Kari (nabo)",
            "Kari\u{0}",
            "Bob\u{202e}gnp.exe",
            "\u{2067}Kari",
            "Kari{name}",
        ] {
            assert!(person(name).is_none(), "{name:?}");
        }
        // A group may have digits; still no other symbols.
        let group = |name: &str| normalize_name(name, NameKind::Group);
        assert_eq!(group("Class of 2010").as_deref(), Some("Class of 2010"));
        assert!(group("2010").is_none());
        assert!(group("<b>Club</b>").is_none());
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
