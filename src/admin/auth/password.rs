//! The built-in user's password: argon2id hashes, checked in constant time per user name.

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

pub(super) struct Credentials {
    pub(super) user: String,
    pub(super) password_hash: String,
}

/// Rejects anything that isn't an argon2 PHC string.
pub fn check_hash(hash: &str) -> Result<()> {
    PasswordHash::new(hash).map_err(|err| anyhow!("not an argon2 hash: {err}"))?;
    Ok(())
}

/// Hashes a password with argon2id and a random salt, for `INNBY_ADMIN_PASSWORD_HASH`.
pub fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow!("hashing password: {err}"))
}

/// Whether the password matches an argon2 PHC string.
pub fn verify(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Accepted,
    Rejected,
    TooManyAttempts,
}

impl Credentials {
    /// The password is always verified, so a wrong user name takes as long as a wrong
    /// password.
    pub(super) fn accept(&self, user: &str, password: &str) -> bool {
        verify(&self.password_hash, password) && user == self.user
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::auth::tests::cheap_hash;

    #[test]
    fn accepts_only_the_right_user_and_password() {
        let credentials = Credentials {
            user: "admin".into(),
            password_hash: cheap_hash("a:b"),
        };
        assert!(credentials.accept("admin", "a:b"));
        assert!(!credentials.accept("admin", "wrong"));
        assert!(!credentials.accept("root", "a:b"));
    }

    #[test]
    fn hashes_verify_and_check() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        check_hash(&hash).unwrap();
        assert!(check_hash("plaintext").is_err());
    }
}
