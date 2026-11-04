//! The host's login. There are two ways in, and either can be turned off:
//!
//! - The built-in user, the way LLDAP does it: the login form checks the password against
//!   an argon2 hash and sets two cookies, a JWT that's good for a day and a refresh token
//!   that gets new JWTs for a month. This user is an admin. Leaving out
//!   `INNBY_ADMIN_PASSWORD_HASH` turns it off.
//! - An OpenID Connect provider (see `oidc`), which says who the host is and what role
//!   they have. That login gets a JWT without a refresh token.
//!
//! The JWT names the user and their role. Logging out revokes it, and the refresh token
//! if there is one. The admin listener is only reachable from the internal gateway; this
//! keeps everyone else on the LAN out too.
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

use super::{
    jwt, lang,
    oidc::{self, OIDC_SESSION},
    role::Role,
};
use crate::{
    config::{AdminConfig, PasswordLogin},
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
    /// The built-in user, unless it's turned off.
    credentials: Option<Arc<Credentials>>,
    oidc: Option<oidc::Provider>,
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
    LoggedIn(Role),
    /// The built-in user, logged in with the refresh token, which got this new `token`
    /// cookie.
    Refreshed(HeaderValue),
    LoggedOut,
}

impl Auth {
    pub fn new(config: &AdminConfig, https: bool, db: PgPool) -> Self {
        // The key is bound to how the host logs in: changing the user, the password or
        // the provider logs every session out.
        let mut bound: Vec<&[u8]> = Vec::new();
        if let Some(PasswordLogin {
            user,
            password_hash,
        }) = &config.login
        {
            bound.extend([
                b"password".as_slice(),
                user.as_bytes(),
                password_hash.as_bytes(),
            ]);
        }
        if let Some(oidc) = &config.oidc {
            bound.extend([
                b"oidc".as_slice(),
                oidc.issuer.as_bytes(),
                oidc.client_id.as_bytes(),
            ]);
        }
        Self {
            credentials: config.login.as_ref().map(|login| {
                Arc::new(Credentials {
                    user: login.user.clone(),
                    password_hash: login.password_hash.clone(),
                })
            }),
            oidc: config.oidc.clone().map(oidc::Provider::new),
            key: jwt::Key::new(&config.jwt_secret, &bound),
            https,
            db,
            checking: Arc::new(Semaphore::new(1)),
            attempts: RateLimiter::new(ATTEMPTS, ATTEMPT_WINDOW),
        }
    }

    pub fn oidc(&self) -> Option<&oidc::Provider> {
        self.oidc.as_ref()
    }

    /// Where a browser that isn't logged in goes: straight to the provider when it's the
    /// only way in.
    fn login_path(&self) -> &'static str {
        match (&self.credentials, &self.oidc) {
            (None, Some(_)) => paths::LOGIN_OIDC,
            _ => paths::LOGIN,
        }
    }

    async fn check(&self, user: String, password: String) -> Outcome {
        let Some(credentials) = &self.credentials else {
            return Outcome::Rejected;
        };
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
        let credentials = Arc::clone(credentials);
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
    fn token(&self, now: DateTime<Utc>, user: &str, role: Role, lifetime: TimeDelta) -> String {
        self.key.sign(&jwt::Claims {
            iat: now.timestamp(),
            exp: (now + lifetime).timestamp(),
            user: user.to_owned(),
            role,
            jti: uuid::Builder::from_random_bytes(random()).into_uuid(),
        })
    }

    /// A new JWT for the built-in user, who is an admin.
    fn admin_token(&self, now: DateTime<Utc>) -> Option<HeaderValue> {
        let user = &self.credentials.as_ref()?.user;
        let token = self.token(now, user, Role::Admin, TOKEN_LIFETIME);
        Some(self.cookie(TOKEN, &token, TOKEN_LIFETIME))
    }

    /// The cookies of a new login of the built-in user, once its refresh token is saved.
    async fn start_session(&self) -> sqlx::Result<Vec<HeaderValue>> {
        let now = Utc::now();
        let refresh_token = URL_SAFE_NO_PAD.encode(random::<32>());
        let digest = self.key.digest(&refresh_token);
        admin_login::save_refresh_token(&self.db, &digest, now + REFRESH_LIFETIME).await?;
        Ok(self
            .admin_token(now)
            .into_iter()
            .chain([self.cookie(REFRESH_TOKEN, &refresh_token, REFRESH_LIFETIME)])
            .collect())
    }

    /// The cookie of a login the provider vouched for.
    pub(super) fn start_oidc_session(&self, login: &oidc::Login) -> HeaderValue {
        let token = self.token(Utc::now(), &login.user, login.role, OIDC_SESSION);
        self.cookie(TOKEN, &token, OIDC_SESSION)
    }

    async fn session(&self, headers: &HeaderMap) -> sqlx::Result<Session> {
        let now = Utc::now();
        if let Some(claims) = self.claims(headers, now)
            && !admin_login::is_revoked(&self.db, claims.jti).await?
        {
            return Ok(Session::LoggedIn(claims.role));
        }
        if self.credentials.is_some()
            && let Some(refresh_token) = cookie(headers, REFRESH_TOKEN)
            && admin_login::has_refresh_token(&self.db, &self.key.digest(refresh_token)).await?
            && let Some(token) = self.admin_token(now)
        {
            return Ok(Session::Refreshed(token));
        }
        Ok(Session::LoggedOut)
    }

    /// Signs a value for a cookie of this purpose, such as a login in progress.
    pub(super) fn seal(&self, purpose: &str, value: &[u8]) -> String {
        self.key.seal(purpose, value)
    }

    /// A value `seal` signed for this purpose.
    pub(super) fn open(&self, purpose: &str, sealed: &str) -> Option<Vec<u8>> {
        self.key.open(purpose, sealed)
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
    pub(super) fn cookie(&self, name: &str, value: &str, lifetime: TimeDelta) -> HeaderValue {
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
pub(super) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
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
        Ok(Session::LoggedIn(role)) => role.scope(next.run(request)).await,
        Ok(Session::Refreshed(token)) => {
            let mut response = Role::Admin.scope(next.run(request)).await;
            response.headers_mut().append(header::SET_COOKIE, token);
            response
        }
        Ok(Session::LoggedOut) => Redirect::to(&paths::admin(auth.login_path())).into_response(),
        Err(err) => server_error(err),
    }
}

#[derive(Template, Default)]
#[template(path = "admin/login.html")]
struct LoginPage {
    logged_out: bool,
    /// Whether the built-in user may log in.
    password: bool,
    /// The provider's name, when there is one.
    oidc: Option<String>,
    /// The user name typed in, when the login failed.
    user: String,
    error: Option<String>,
}

impl LoginPage {
    fn new(auth: &Auth) -> Self {
        Self {
            password: auth.credentials.is_some(),
            oidc: auth.oidc.as_ref().map(|oidc| oidc.name().to_owned()),
            ..Self::default()
        }
    }
}

/// The login page, with what went wrong.
pub(super) fn login_error(auth: &Auth, status: StatusCode, error: String) -> Response {
    let page = LoginPage {
        error: Some(error),
        ..LoginPage::new(auth)
    };
    render_with(status, page)
}

#[derive(Deserialize)]
pub struct LoginQuery {
    logged_out: Option<String>,
}

pub async fn login_page(
    State(auth): State<Arc<Auth>>,
    Query(query): Query<LoginQuery>,
) -> Response {
    render(LoginPage {
        logged_out: query.logged_out.is_some(),
        ..LoginPage::new(&auth)
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
                    let cookies = cookies
                        .into_iter()
                        .map(|cookie| (header::SET_COOKIE, cookie));
                    let home = Redirect::to(&paths::admin(paths::HOME));
                    (AppendHeaders(cookies), home).into_response()
                }
                Err(err) => server_error(err),
            };
        }
        Outcome::Rejected => (StatusCode::UNAUTHORIZED, &text.wrong_login),
        Outcome::TooManyAttempts => (StatusCode::TOO_MANY_REQUESTS, &text.too_many_logins),
    };
    let page = LoginPage {
        user: form.user,
        error: Some(error.clone()),
        ..LoginPage::new(&auth)
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

    /// `admin` with the password `secret`, and no provider.
    pub fn admin_config() -> AdminConfig {
        AdminConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            login: Some(PasswordLogin {
                user: "admin".into(),
                password_hash: cheap_hash("secret"),
            }),
            oidc: None,
            jwt_secret: jwt::Secret::from_base64(jwt::tests::SECRET).unwrap(),
        }
    }

    /// A JWT for this role that's valid now, for tests that don't log in through a form.
    pub fn token(config: &AdminConfig, db: PgPool, role: Role) -> String {
        Auth::new(config, true, db).token(Utc::now(), "admin", role, TOKEN_LIFETIME)
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
