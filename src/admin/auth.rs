//! HTTP basic auth against an argon2 hash. The admin listener is only reachable from the
//! internal gateway; this keeps everyone else on the LAN out too.
//!
//! Each argon2 check needs 19 MiB and the admin pod gets 64, so checks run one at a time
//! and a login that checked out is remembered rather than checked on every request. After
//! too many checks, new logins are refused for a while; remembered ones keep working.

use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use crate::{config::AdminConfig, web::RateLimiter};

/// Password checks allowed per window, right or wrong.
const ATTEMPTS: u32 = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);
/// How long a login that checked out is remembered.
const REMEMBER_FOR: Duration = Duration::from_secs(60 * 60);

struct Credentials {
    user: String,
    password_hash: String,
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

/// Whether the password matches an argon2 PHC string.
pub fn verify(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// Checks logins for the admin listener.
pub struct Login {
    credentials: Arc<Credentials>,
    /// One permit: the argon2 check that may run right now.
    checking: Arc<Semaphore>,
    /// SHA-256 of the last `Authorization` header that checked out, and when.
    remembered: Mutex<Option<([u8; 32], Instant)>>,
    attempts: RateLimiter,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Rejected,
    TooManyAttempts,
}

impl Login {
    pub fn new(config: &AdminConfig) -> Self {
        Self::with(Credentials {
            user: config.user.clone(),
            password_hash: config.password_hash.clone(),
        })
    }

    fn with(credentials: Credentials) -> Self {
        Self {
            credentials: Arc::new(credentials),
            checking: Arc::new(Semaphore::new(1)),
            remembered: Mutex::default(),
            attempts: RateLimiter::new(ATTEMPTS, ATTEMPT_WINDOW),
        }
    }

    async fn check(&self, authorization: &HeaderValue) -> Outcome {
        let digest: [u8; 32] = Sha256::digest(authorization.as_bytes()).into();
        if self.remembers(&digest) {
            return Outcome::Accepted;
        }
        let Some(supplied) = decode(authorization) else {
            return Outcome::Rejected;
        };
        // The permit moves into the blocking task, so a request that times out can't
        // let another check start while its own still runs.
        let permit = Arc::clone(&self.checking)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        // The browser fetches the page and its stylesheet at once: the first request
        // may have logged in while this one waited.
        if self.remembers(&digest) {
            return Outcome::Accepted;
        }
        if !self.attempts.check_all() {
            tracing::warn!("admin login attempts limit hit");
            return Outcome::TooManyAttempts;
        }
        let credentials = Arc::clone(&self.credentials);
        let accepted = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            credentials.accept(&supplied)
        })
        .await
        .unwrap_or(false);
        if accepted {
            *self.lock_remembered() = Some((digest, Instant::now()));
            Outcome::Accepted
        } else {
            tracing::warn!("admin login failed");
            Outcome::Rejected
        }
    }

    fn remembers(&self, digest: &[u8; 32]) -> bool {
        self.lock_remembered().is_some_and(|(remembered, at)| {
            at.elapsed() < REMEMBER_FOR && bool::from(remembered.ct_eq(digest))
        })
    }

    fn lock_remembered(&self) -> MutexGuard<'_, Option<([u8; 32], Instant)>> {
        self.remembered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// `user:password` from a basic auth header.
fn decode(authorization: &HeaderValue) -> Option<String> {
    let encoded = authorization.to_str().ok()?.strip_prefix("Basic ")?;
    String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()
}

pub async fn require_login(
    State(login): State<Arc<Login>>,
    request: Request,
    next: Next,
) -> Response {
    let outcome = match request.headers().get(header::AUTHORIZATION) {
        Some(authorization) => login.check(authorization).await,
        None => Outcome::Rejected,
    };
    match outcome {
        Outcome::Accepted => next.run(request).await,
        Outcome::Rejected => (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                r#"Basic realm="fest admin", charset="UTF-8""#,
            )],
            "login required",
        )
            .into_response(),
        Outcome::TooManyAttempts => (
            StatusCode::TOO_MANY_REQUESTS,
            "too many login attempts, try again in 15 minutes",
        )
            .into_response(),
    }
}

impl Credentials {
    /// Checks `user:password`. The password is always verified, so a wrong user name
    /// takes as long as a wrong password.
    fn accept(&self, supplied: &str) -> bool {
        let Some((user, password)) = supplied.split_once(':') else {
            return false;
        };
        verify(&self.password_hash, password) && user == self.user
    }
}

#[cfg(test)]
pub mod tests {
    use argon2::{Algorithm, Params, Version};

    use super::*;

    /// A hash with tiny argon2 parameters, so tests stay fast.
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

    fn basic(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).unwrap()
    }

    fn login_header(user_password: &str) -> HeaderValue {
        basic(&format!("Basic {}", STANDARD.encode(user_password)))
    }

    #[tokio::test]
    async fn remembers_logins_and_limits_guessing() {
        let login = Login::with(credentials());
        let right = login_header("admin:secret");
        assert_eq!(login.check(&right).await, Outcome::Accepted);
        assert_eq!(login.check(&basic("Basic !!!")).await, Outcome::Rejected);
        // The login above was the first of the attempts.
        for i in 1..ATTEMPTS {
            let guess = login_header(&format!("admin:guess{i}"));
            assert_eq!(login.check(&guess).await, Outcome::Rejected, "{i}");
        }
        let guess = login_header("admin:one-more");
        assert_eq!(login.check(&guess).await, Outcome::TooManyAttempts);

        // The host's browser keeps working; a new login has to wait.
        assert_eq!(login.check(&right).await, Outcome::Accepted);
        let new_login = basic(&format!("{} ", right.to_str().unwrap()));
        assert_eq!(login.check(&new_login).await, Outcome::TooManyAttempts);
    }

    #[tokio::test]
    async fn parallel_requests_check_the_password_once() {
        let login = Arc::new(Login::with(credentials()));
        let right = login_header("admin:secret");
        let requests: Vec<_> = (0..ATTEMPTS + 5)
            .map(|_| {
                let (login, right) = (Arc::clone(&login), right.clone());
                tokio::spawn(async move { login.check(&right).await })
            })
            .collect();
        for request in requests {
            assert_eq!(request.await.unwrap(), Outcome::Accepted);
        }
    }
}
