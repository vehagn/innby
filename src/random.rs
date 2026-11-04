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
