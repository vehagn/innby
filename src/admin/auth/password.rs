//! The password users' passwords: argon2id hashes, each checked in about the same time
//! whether or not the user exists. The config's users are here; those made in the admin
//! UI are in `db::admin_user`.

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

use crate::{admin::role::Role, config::PasswordLogin};

/// The users who log in with a password; never none.
pub(super) struct Credentials(pub(super) Vec<PasswordLogin>);

/// Rejects anything that isn't an argon2 PHC string.
pub fn check_hash(hash: &str) -> Result<()> {
    PasswordHash::new(hash).map_err(|err| anyhow!("not an argon2 hash: {err}"))?;
    Ok(())
}

/// Hashes a password with argon2id and a random salt, for `INNBY_ADMIN_PASSWORD_HASH`
/// or `INNBY_ADMIN_USERS`.
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
    /// The user's name, their role, and their password's version: 0 for the config's.
    Accepted(String, Option<Role>, i32),
    Rejected,
    TooManyAttempts,
}

/// A hash of a random password, checked when nobody has the name typed, so a wrong name
/// takes as long as a wrong password. Default strength, as `hash_password` makes them.
pub(super) const NOBODY: &str = "$argon2id$v=19$m=19456,t=2,p=1$GvRh6oBJLai1yE+E2T5FKA$strZ7kNRumHibu9LDpRbInyNay/v8NMoB/Air/ftO1c";

impl Credentials {
    pub(super) fn find(&self, user: &str) -> Option<&PasswordLogin> {
        self.0.iter().find(|login| login.user == user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_is_a_hash_of_the_default_strength() {
        check_hash(NOBODY).unwrap();
        assert!(NOBODY.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    }

    #[test]
    fn hashes_verify_and_check() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        check_hash(&hash).unwrap();
        assert!(check_hash("plaintext").is_err());
    }
}
