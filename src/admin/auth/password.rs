//! The password users' passwords: argon2id hashes, each checked in about the same time
//! whether or not the user exists.

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
    /// The user's name, as configured, and their role.
    Accepted(String, Role),
    Rejected,
    TooManyAttempts,
}

impl Credentials {
    /// The user, if the password is theirs. A password is always verified, against
    /// another user's hash when nobody has the name, so a wrong user name takes as long
    /// as a wrong password.
    pub(super) fn accept(&self, user: &str, password: &str) -> Option<&PasswordLogin> {
        let found = self.find(user);
        let hash = &found.or(self.0.first())?.password_hash;
        verify(hash, password).then_some(found).flatten()
    }

    pub(super) fn find(&self, user: &str) -> Option<&PasswordLogin> {
        self.0.iter().find(|login| login.user == user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::auth::tests::cheap_hash;

    #[test]
    fn accepts_only_the_right_user_and_password() {
        let user = |user: &str, role, password| PasswordLogin {
            user: user.into(),
            role,
            password_hash: cheap_hash(password),
        };
        let credentials = Credentials(vec![
            user("admin", Role::Admin, "a:b"),
            user("ola", Role::Viewer, "c"),
        ]);
        let accepted = |user, password| {
            credentials
                .accept(user, password)
                .map(|login| (login.user.as_str(), login.role))
        };
        assert_eq!(accepted("admin", "a:b"), Some(("admin", Role::Admin)));
        assert_eq!(accepted("ola", "c"), Some(("ola", Role::Viewer)));
        assert_eq!(accepted("admin", "wrong"), None);
        assert_eq!(accepted("ola", "a:b"), None);
        // Nobody's name, with the password checked against the first user's.
        assert_eq!(accepted("root", "a:b"), None);
    }

    #[test]
    fn hashes_verify_and_check() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        check_hash(&hash).unwrap();
        assert!(check_hash("plaintext").is_err());
    }
}
