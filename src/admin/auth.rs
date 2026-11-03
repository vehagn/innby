//! The host's login, the way LLDAP does it. The login form checks the password against an
//! argon2 hash and sets two cookies: a JWT that's good for a day, and a refresh token that
//! gets new JWTs for a month. Logging out revokes both. The admin listener is only
//! reachable from the internal gateway; this keeps everyone else on the LAN out too.
//!
//! Each argon2 check needs 19 MiB and the admin pod gets 64, so checks run one at a time.
//! After too many checks, logins are refused for a while; browsers that are logged in
//! keep working.

use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use askama::Template;
use axum::{
    Form,
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{AppendHeaders, IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use tokio::sync::Semaphore;

use super::{jwt, lang};
use crate::{
    config::AdminConfig,
    db::admin_login,
    paths,
    web::{RateLimiter, render, render_with, server_error},
};

/// Password checks allowed per window, right or wrong.
const ATTEMPTS: u32 = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);
/// How long a JWT lasts, and a refresh token: LLDAP's defaults.
const TOKEN_LIFETIME: TimeDelta = TimeDelta::days(1);
const REFRESH_LIFETIME: TimeDelta = TimeDelta::days(30);
/// The cookies, named as LLDAP names them.
const TOKEN: &str = "token";
const REFRESH_TOKEN: &str = "refresh_token";

struct Credentials {
    user: String,
    password_hash: String,
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

/// Logs the host in and out, and checks their cookies.
pub struct Auth {
    credentials: Arc<Credentials>,
    key: jwt::Key,
    /// Whether the cookies are only for HTTPS.
    https: bool,
    db: PgPool,
    /// One permit: the argon2 check that may run right now.
    checking: Arc<Semaphore>,
    attempts: RateLimiter,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Rejected,
    TooManyAttempts,
}

/// What a request's cookies say.
enum Session {
    LoggedIn,
    /// Logged in with the refresh token, which got this new `token` cookie.
    Refreshed(HeaderValue),
    LoggedOut,
}

impl Auth {
    pub fn new(config: &AdminConfig, https: bool, db: PgPool) -> Self {
        Self {
            credentials: Arc::new(Credentials {
                user: config.user.clone(),
                password_hash: config.password_hash.clone(),
            }),
            key: jwt::Key::new(&config.jwt_secret, &config.user, &config.password_hash),
            https,
            db,
            checking: Arc::new(Semaphore::new(1)),
            attempts: RateLimiter::new(ATTEMPTS, ATTEMPT_WINDOW),
        }
    }

    async fn check(&self, user: String, password: String) -> Outcome {
        // The permit moves into the blocking task, so a request that times out can't
        // let another check start while its own still runs.
        let permit = Arc::clone(&self.checking)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        if !self.attempts.check_all() {
            tracing::warn!("admin login attempts limit hit");
            return Outcome::TooManyAttempts;
        }
        let credentials = Arc::clone(&self.credentials);
        let accepted = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            credentials.accept(&user, &password)
        })
        .await
        .unwrap_or(false);
        if accepted {
            Outcome::Accepted
        } else {
            tracing::warn!("admin login failed");
            Outcome::Rejected
        }
    }

    /// A new JWT for the host.
    fn token(&self, now: DateTime<Utc>) -> String {
        self.key.sign(&jwt::Claims {
            iat: now.timestamp(),
            exp: (now + TOKEN_LIFETIME).timestamp(),
            user: self.credentials.user.clone(),
            jti: uuid::Builder::from_random_bytes(random()).into_uuid(),
        })
    }

    /// The cookies of a new login, once its refresh token is saved.
    async fn start_session(&self) -> sqlx::Result<[HeaderValue; 2]> {
        let now = Utc::now();
        let refresh_token = URL_SAFE_NO_PAD.encode(random::<32>());
        let digest = self.key.digest(&refresh_token);
        admin_login::save_refresh_token(&self.db, &digest, now + REFRESH_LIFETIME).await?;
        Ok([
            self.cookie(TOKEN, &self.token(now), TOKEN_LIFETIME),
            self.cookie(REFRESH_TOKEN, &refresh_token, REFRESH_LIFETIME),
        ])
    }

    async fn session(&self, headers: &HeaderMap) -> sqlx::Result<Session> {
        let now = Utc::now();
        if let Some(claims) = self.claims(headers, now)
            && !admin_login::is_revoked(&self.db, claims.jti).await?
        {
            return Ok(Session::LoggedIn);
        }
        if let Some(refresh_token) = cookie(headers, REFRESH_TOKEN)
            && admin_login::has_refresh_token(&self.db, &self.key.digest(refresh_token)).await?
        {
            let token = self.cookie(TOKEN, &self.token(now), TOKEN_LIFETIME);
            return Ok(Session::Refreshed(token));
        }
        Ok(Session::LoggedOut)
    }

    /// Revokes the request's JWT and refresh token, so copies of the cookies don't work
    /// either.
    async fn end_session(&self, headers: &HeaderMap) -> sqlx::Result<()> {
        if let Some(claims) = self.claims(headers, Utc::now()) {
            let expires_at = DateTime::from_timestamp(claims.exp, 0).unwrap_or_default();
            admin_login::revoke_jwt(&self.db, claims.jti, expires_at).await?;
        }
        if let Some(refresh_token) = cookie(headers, REFRESH_TOKEN) {
            admin_login::delete_refresh_token(&self.db, &self.key.digest(refresh_token)).await?;
        }
        Ok(())
    }

    fn claims(&self, headers: &HeaderMap, now: DateTime<Utc>) -> Option<jwt::Claims> {
        self.key.verify(cookie(headers, TOKEN)?, now)
    }

    /// A `Set-Cookie` value. `SameSite=Lax` rather than LLDAP's `Strict`, so a link to
    /// the admin pages from elsewhere opens them logged in; `csrf::protect` turns away
    /// forms posted from other sites.
    fn cookie(&self, name: &str, value: &str, lifetime: TimeDelta) -> HeaderValue {
        let secure = if self.https { "; Secure" } else { "" };
        let max_age = lifetime.num_seconds();
        format!("{name}={value}; Max-Age={max_age}; Path=/; HttpOnly; SameSite=Lax{secure}")
            .try_into()
            .expect("tokens are base64")
    }
}

impl Credentials {
    /// The password is always verified, so a wrong user name takes as long as a wrong
    /// password.
    fn accept(&self, user: &str, password: &str) -> bool {
        verify(&self.password_hash, password) && user == self.user
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the system has randomness");
    bytes
}

/// A cookie the request sent.
fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Lets a request through if it's logged in, and sends it to the login page otherwise.
pub async fn require_login(
    State(auth): State<Arc<Auth>>,
    request: Request,
    next: Next,
) -> Response {
    match auth.session(request.headers()).await {
        Ok(Session::LoggedIn) => next.run(request).await,
        Ok(Session::Refreshed(token)) => {
            let mut response = next.run(request).await;
            response.headers_mut().append(header::SET_COOKIE, token);
            response
        }
        Ok(Session::LoggedOut) => Redirect::to(&paths::admin(paths::LOGIN)).into_response(),
        Err(err) => server_error(err),
    }
}

#[derive(Template, Default)]
#[template(path = "admin/login.html")]
struct LoginPage {
    logged_out: bool,
    /// The user name typed in, when the login failed.
    user: String,
    error: Option<&'static str>,
}

#[derive(Deserialize)]
pub struct LoginQuery {
    logged_out: Option<String>,
}

pub async fn login_page(Query(query): Query<LoginQuery>) -> Response {
    render(LoginPage {
        logged_out: query.logged_out.is_some(),
        ..LoginPage::default()
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    user: String,
    password: String,
}

pub async fn log_in(State(auth): State<Arc<Auth>>, Form(form): Form<LoginForm>) -> Response {
    let text = lang::text();
    let (status, error) = match auth.check(form.user.clone(), form.password).await {
        Outcome::Accepted => {
            return match auth.start_session().await {
                Ok(cookies) => {
                    tracing::info!("host logged in");
                    let cookies = cookies.map(|cookie| (header::SET_COOKIE, cookie));
                    let home = Redirect::to(&paths::admin(paths::HOME));
                    (AppendHeaders(cookies), home).into_response()
                }
                Err(err) => server_error(err),
            };
        }
        Outcome::Rejected => (StatusCode::UNAUTHORIZED, text.wrong_login.as_str()),
        Outcome::TooManyAttempts => (StatusCode::TOO_MANY_REQUESTS, text.too_many_logins.as_str()),
    };
    let page = LoginPage {
        logged_out: false,
        user: form.user,
        error: Some(error),
    };
    render_with(status, page)
}

/// Needs no login, so a browser whose JWT has expired can still forget its refresh token.
pub async fn log_out(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Err(err) = auth.end_session(&headers).await {
        return server_error(err);
    }
    tracing::info!("host logged out");
    let cleared = [TOKEN, REFRESH_TOKEN]
        .map(|name| (header::SET_COOKIE, auth.cookie(name, "", TimeDelta::zero())));
    let page = paths::admin(&format!("{}?logged_out", paths::LOGIN));
    (AppendHeaders(cleared), Redirect::to(&page)).into_response()
}

#[cfg(test)]
pub mod tests {
    use argon2::{Algorithm, Params, Version};
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    /// A hash with tiny argon2 parameters, so tests stay fast.
    pub fn cheap_hash(password: &str) -> String {
        let params = Params::new(8, 1, 1, None).unwrap();
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string()
    }

    /// `admin` with the password `secret`.
    pub fn admin_config() -> AdminConfig {
        AdminConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            user: "admin".into(),
            password_hash: cheap_hash("secret"),
            jwt_secret: jwt::Secret::from_base64(jwt::tests::SECRET).unwrap(),
        }
    }

    /// A JWT that's valid now, for tests that don't log in through the form.
    pub fn token(config: &AdminConfig, db: PgPool) -> String {
        Auth::new(config, true, db).token(Utc::now())
    }

    fn auth() -> Auth {
        // Checking passwords never touches the database.
        let db = PgPoolOptions::new()
            .connect_lazy("postgres://innby@127.0.0.1:1/innby")
            .unwrap();
        Auth::new(&admin_config(), true, db)
    }

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

    #[tokio::test]
    async fn limits_guessing() {
        let auth = auth();
        let check = |password: &str| auth.check("admin".into(), password.into());
        assert_eq!(check("secret").await, Outcome::Accepted);
        // The login above was the first of the attempts.
        for i in 1..ATTEMPTS {
            assert_eq!(check(&format!("guess{i}")).await, Outcome::Rejected, "{i}");
        }
        assert_eq!(check("secret").await, Outcome::TooManyAttempts);
    }

    #[test]
    fn reads_cookies() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            "theme=dark; refresh_token=r".parse().unwrap(),
        );
        headers.append(header::COOKIE, "token=t".parse().unwrap());
        assert_eq!(cookie(&headers, TOKEN), Some("t"));
        assert_eq!(cookie(&headers, REFRESH_TOKEN), Some("r"));
        assert_eq!(cookie(&headers, "theme"), Some("dark"));
        assert_eq!(cookie(&headers, "them"), None);
    }

    #[tokio::test]
    async fn cookies_are_http_only_and_secure_over_https() {
        let cookie = auth().cookie(TOKEN, "t", TOKEN_LIFETIME);
        assert_eq!(
            cookie,
            "token=t; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }
}
