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
//! After too many checks from one client address, that address's logins are refused for
//! a while, so a stranger guessing can't lock the host out. A much higher limit across
//! every address bounds guessing from many at once. Browsers that are logged in keep
//! working either way.
//!
//! - `mod.rs`: `Auth`, the sessions, and `require_login`, which every page goes through.
//! - `login`: the login page and logging out. `password`: the built-in user's argon2
//!   hash. `jwt`: the session tokens. `oidc`: single sign-on.

pub mod jwt;
mod login;
pub mod oidc;
mod password;

pub(in crate::admin) use login::login_error;
pub use login::{log_in, log_out, login_page};
pub use password::{check_hash, hash_password, verify};

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgPool;
use tokio::sync::Semaphore;

use super::role::Role;
use crate::{
    config::{AdminConfig, PasswordLogin},
    db::admin_login,
    http::{ClientIp, RateLimiter, cookie, server_error},
    paths, random,
};
use oidc::OIDC_SESSION;
use password::{Credentials, Outcome};

/// Password checks allowed per client address and window, right or wrong.
const ATTEMPTS: u32 = 10;
/// The same from every address together.
const ALL_ATTEMPTS: u32 = 100;

const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How long a JWT lasts, and a refresh token: LLDAP's defaults.
const TOKEN_LIFETIME: TimeDelta = TimeDelta::days(1);

const REFRESH_LIFETIME: TimeDelta = TimeDelta::days(30);

/// The cookies, named as LLDAP names them.
const TOKEN: &str = "token";

const REFRESH_TOKEN: &str = "refresh_token";

/// Logs the host in and out, and checks their cookies.
pub struct Auth {
    /// The built-in user, unless it's turned off.
    credentials: Option<Arc<Credentials>>,
    oidc: Option<oidc::Provider>,
    key: jwt::Key,
    /// Whether the cookies are only for HTTPS.
    https: bool,
    /// The path the cookies are for: the admin UI's, so a site on the same hostname
    /// never gets them.
    cookie_path: String,
    db: PgPool,
    /// One permit: the argon2 check that may run right now.
    checking: Arc<Semaphore>,
    attempts: RateLimiter,
    all_attempts: RateLimiter,
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
            cookie_path: match config.path.as_str() {
                "" => "/".to_owned(),
                path => path.to_owned(),
            },
            db,
            checking: Arc::new(Semaphore::new(1)),
            attempts: RateLimiter::new(ATTEMPTS, ATTEMPT_WINDOW),
            all_attempts: RateLimiter::new(ALL_ATTEMPTS, ATTEMPT_WINDOW),
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

    async fn check(&self, client: ClientIp, user: String, password: String) -> Outcome {
        let Some(credentials) = &self.credentials else {
            return Outcome::Rejected;
        };
        // The permit moves into the blocking task, so a request that times out can't
        // let another check start while its own still runs.
        let permit = Arc::clone(&self.checking)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        // A client over its limit doesn't count against everyone's.
        if !self.attempts.check(client) {
            tracing::warn!("admin login attempts limit hit for a client");
            return Outcome::TooManyAttempts;
        }
        if !self.all_attempts.check_all() {
            tracing::warn!("admin login attempts limit hit for all clients");
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
            jti: uuid::Builder::from_random_bytes(random::bytes()).into_uuid(),
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
        let refresh_token = random::token();
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
            && let Some(refresh_token) = cookie::get(headers, REFRESH_TOKEN)
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
        if let Some(refresh_token) = cookie::get(headers, REFRESH_TOKEN) {
            admin_login::delete_refresh_token(&self.db, &self.key.digest(refresh_token)).await?;
        }
        Ok(())
    }

    fn claims(&self, headers: &HeaderMap, now: DateTime<Utc>) -> Option<jwt::Claims> {
        self.key.verify(cookie::get(headers, TOKEN)?, now)
    }

    /// A `Set-Cookie` value for the admin UI's paths. `SameSite=Lax` rather than LLDAP's
    /// `Strict`, so a link to the admin pages from elsewhere opens them logged in;
    /// `http::csrf` turns away forms posted from other sites.
    pub(super) fn cookie(&self, name: &str, value: &str, lifetime: TimeDelta) -> HeaderValue {
        cookie::set(
            name,
            value,
            &self.cookie_path,
            lifetime.num_seconds(),
            self.https,
        )
    }
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

#[cfg(test)]
pub mod tests {
    use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
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
            path: String::new(),
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

    #[tokio::test]
    async fn limits_guessing_per_client() {
        let auth = auth();
        let client = |last: u8| ClientIp([192, 0, 2, last].into());
        let check =
            |last: u8, password: &str| auth.check(client(last), "admin".into(), password.into());
        assert_eq!(check(1, "secret").await, Outcome::Accepted);
        // The login above was the first of the attempts.
        for i in 1..ATTEMPTS {
            assert_eq!(
                check(1, &format!("guess{i}")).await,
                Outcome::Rejected,
                "{i}"
            );
        }
        assert_eq!(check(1, "secret").await, Outcome::TooManyAttempts);
        // Someone else still gets in.
        assert_eq!(check(2, "secret").await, Outcome::Accepted);
    }

    #[tokio::test]
    async fn limits_guessing_from_many_clients() {
        let auth = auth();
        // 100 checks so far, from ten clients, ten each.
        for client in 0..10u8 {
            for _ in 0..10 {
                let ip = ClientIp([192, 0, 2, client].into());
                assert_eq!(
                    auth.check(ip, "admin".into(), "guess".into()).await,
                    Outcome::Rejected
                );
            }
        }
        let fresh = ClientIp([198, 51, 100, 1].into());
        assert_eq!(
            auth.check(fresh, "admin".into(), "secret".into()).await,
            Outcome::TooManyAttempts
        );
    }

    #[tokio::test]
    async fn cookies_are_http_only_and_secure_over_https() {
        let cookie = auth().cookie(TOKEN, "t", TOKEN_LIFETIME);
        assert_eq!(
            cookie,
            "token=t; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax; Secure"
        );
    }
}
