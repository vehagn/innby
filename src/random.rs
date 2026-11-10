//! Randomness from the operating system, for tokens, nonces and IDs.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

/// `N` random bytes.
pub fn bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the system has randomness");
    bytes
}

/// 32 random bytes as base64url: a token nobody can guess.
pub fn token() -> String {
    URL_SAFE_NO_PAD.encode(bytes::<32>())
}

/// Whether a form's hidden field holds what could be a `token`, rather than something
/// made up to fill a column: base64url, of a sane length.
pub fn is_token(text: &str) -> bool {
    (16..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_own_tokens() {
        assert!(is_token(&token()));
        assert_ne!(token(), token());
        for other in ["", "short", "has spaces in the token", &"a".repeat(65)] {
            assert!(!is_token(other), "{other}");
        }
    }
}
