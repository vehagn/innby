//! HTTP basic auth against an argon2 hash. The admin listener is only reachable from the
//! internal gateway; this keeps everyone else on the LAN out too.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::config::AdminConfig;

pub struct Credentials {
    user: String,
    password_hash: String,
}

impl From<&AdminConfig> for Credentials {
    fn from(config: &AdminConfig) -> Self {
        Self {
            user: config.user.clone(),
            password_hash: config.password_hash.clone(),
        }
    }
}

/// Rejects anything that isn't an argon2 PHC string.
pub fn check_hash(hash: &str) -> Result<()> {
    PasswordHash::new(hash).map_err(|err| anyhow!("not an argon2 hash: {err}"))?;
    Ok(())
}

/// Hashes a password with argon2id and a random salt, for `FEST_ADMIN_PASSWORD_HASH`.
pub fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow!("hashing password: {err}"))
}

pub async fn require_login(
    State(credentials): State<Arc<Credentials>>,
    request: Request,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| STANDARD.decode(encoded.trim()).ok())
        .and_then(|decoded| String::from_utf8(decoded).ok());
    let accepted = match supplied {
        // Argon2 takes tens of milliseconds by design; keep it off the async workers.
        Some(supplied) => tokio::task::spawn_blocking(move || credentials.accept(&supplied))
            .await
            .unwrap_or(false),
        None => false,
    };
    if accepted {
        return next.run(request).await;
    }
    if request.headers().contains_key(header::AUTHORIZATION) {
        tracing::warn!("admin login failed");
    }
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            r#"Basic realm="fest admin", charset="UTF-8""#,
        )],
        "login required",
    )
        .into_response()
}

impl Credentials {
    /// Checks `user:password`. The password is always verified, so a wrong user name
    /// takes as long as a wrong password.
    fn accept(&self, supplied: &str) -> bool {
        let Some((user, password)) = supplied.split_once(':') else {
            return false;
        };
        let Ok(hash) = PasswordHash::new(&self.password_hash) else {
            return false;
        };
        let password_ok = Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok();
        password_ok && user == self.user
    }
}

#[cfg(test)]
pub mod tests {
    use argon2::{Algorithm, Params, Version};

    use super::*;

    /// A hash of "secret" with tiny argon2 parameters, so tests stay fast.
    pub fn cheap_hash(password: &str) -> String {
        let params = Params::new(8, 1, 1, None).unwrap();
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string()
    }

    fn credentials() -> Credentials {
        Credentials {
            user: "admin".into(),
            password_hash: cheap_hash("secret"),
        }
    }

    #[test]
    fn accepts_only_the_right_user_and_password() {
        let credentials = credentials();
        assert!(credentials.accept("admin:secret"));
        assert!(!credentials.accept("admin:wrong"));
        assert!(!credentials.accept("root:secret"));
        assert!(!credentials.accept("admin"));
    }

    #[test]
    fn passwords_may_contain_colons() {
        let credentials = Credentials {
            user: "admin".into(),
            password_hash: cheap_hash("a:b"),
        };
        assert!(credentials.accept("admin:a:b"));
    }

    #[test]
    fn hashes_verify_and_check() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        check_hash(&hash).unwrap();
        assert!(check_hash("plaintext").is_err());
    }
}
