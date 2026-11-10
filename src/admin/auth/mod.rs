//! The host's login. There are two ways in, and either can be turned off:
//!
//! - A user name and password, the way LLDAP does it: the login form checks the password
//!   against an argon2 hash and sets two cookies, a JWT that's good for a day and a
//!   refresh token that gets new JWTs for a month. The users are the built-in admin
//!   (unless `INNBY_ADMIN_PASSWORD_HASH` is left out) and those in `INNBY_ADMIN_USERS`,
//!   each with the role it gives them, or none; and those an admin makes in the admin
//!   UI (`db::admin_user`), whose role is read on each request, and whose sessions end
//!   with a new password or their deletion.
//! - An OpenID Connect provider (see `oidc`), which says who the host is and what role
//!   they have. That login gets a JWT without a refresh token.
//!
//! The JWT names the user and how they logged in, with the role a provider gave them;
//! a password user's is read on each request. `role::for_event` adds the
//! one an event gives them. Logging out revokes it, and the refresh token
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
//! - `login`: the login page and logging out. `password`: checking the users' argon2
//!   hashes. `jwt`: the session tokens. `oidc`: single sign-on.

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

use super::role::{Host, Role};
use crate::{
    config::{AdminConfig, PasswordLogin},
    db::{access, admin_login, admin_user},
    http::{ClientIp, RateLimiter, cookie, server_error},
    paths, random,
};
use jwt::Login;
use oidc::OIDC_SESSION;
use password::{Credentials, NOBODY, Outcome};

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
    /// The config's users who log in with a password; the database has more.
    credentials: Arc<Credentials>,
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
    LoggedIn(Host),
    /// A password user, logged in with the refresh token, which got this new `token`
    /// cookie.
    Refreshed(HeaderValue, Host),
    LoggedOut,
}

impl Auth {
    pub fn new(config: &AdminConfig, https: bool, db: PgPool) -> Self {
        // The key is bound to how hosts log in: changing a user, their role or password,
        // or the provider logs every session out.
        let mut bound: Vec<&[u8]> = Vec::new();
        for login in &config.users {
            bound.extend([
                b"password".as_slice(),
                login.user.as_bytes(),
                login.role.map_or("none", Role::name).as_bytes(),
                login.password_hash.as_bytes(),
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
            credentials: Arc::new(Credentials(config.users.clone())),
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

    /// The config's password users, which the admin UI shows but can't change.
    pub(in crate::admin) fn config_users(&self) -> &[PasswordLogin] {
        &self.credentials.0
    }

    /// Whether anyone may log in with a password: from the config, or the database.
    pub(in crate::admin) async fn has_passwords(&self) -> bool {
        !self.credentials.0.is_empty()
            || admin_user::any(&self.db).await.unwrap_or_else(|err| {
                tracing::error!("admin users: {err}");
                true
            })
    }

    /// Where a browser that isn't logged in goes: straight to the provider when it's the
    /// only way in.
    async fn login_path(&self) -> &'static str {
        if self.oidc.is_some() && !self.has_passwords().await {
            paths::LOGIN_OIDC
        } else {
            paths::LOGIN
        }
    }

    /// Hashes a password for a database user, one at a time, as logins are checked.
    pub(in crate::admin) async fn hash(&self, password: String) -> anyhow::Result<String> {
        let permit = Arc::clone(&self.checking)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            password::hash_password(&password)
        })
        .await?
    }

    async fn check(&self, client: ClientIp, user: String, password: String) -> Outcome {
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
        // The config's user by the name, else the database's, else nobody, whose dummy
        // hash is checked all the same.
        let (hash, login) = match self.credentials.find(&user) {
            Some(login) => (
                login.password_hash.clone(),
                Some((login.user.clone(), login.role, 0)),
            ),
            None => match admin_user::find(&self.db, &user).await {
                Ok(Some(stored)) => (
                    stored.password_hash,
                    Some((
                        stored.name,
                        stored_role(stored.role.as_deref()),
                        stored.version,
                    )),
                ),
                Ok(None) => (NOBODY.to_owned(), None),
                Err(err) => {
                    tracing::error!("admin users: {err}");
                    return Outcome::Rejected;
                }
            },
        };
        let matches = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            password::verify(&hash, &password)
        })
        .await
        .unwrap_or(false);
        match login.filter(|_| matches) {
            Some((user, role, version)) => Outcome::Accepted(user, role, version),
            None => {
                tracing::warn!("admin login failed");
                Outcome::Rejected
            }
        }
    }

    /// A new JWT for the host.
    fn token(&self, now: DateTime<Utc>, user: &str, login: Login, lifetime: TimeDelta) -> String {
        self.key.sign(&jwt::Claims {
            iat: now.timestamp(),
            exp: (now + lifetime).timestamp(),
            user: user.to_owned(),
            login,
            jti: uuid::Builder::from_random_bytes(random::bytes()).into_uuid(),
        })
    }

    /// The `token` cookie of a password user.
    fn password_token(&self, now: DateTime<Utc>, user: &str, version: i32) -> HeaderValue {
        let token = self.token(now, user, Login::Password { version }, TOKEN_LIFETIME);
        self.cookie(TOKEN, &token, TOKEN_LIFETIME)
    }

    /// The cookies of a password user's new login, once its refresh token is saved. The
    /// refresh token starts with the user's name and password version,
    /// `ola.3.{random}`, so it can renew their JWT while the password stands; the
    /// database keeps only its keyed digest, which covers both.
    async fn start_session(&self, user: &str, version: i32) -> sqlx::Result<[HeaderValue; 2]> {
        let now = Utc::now();
        let refresh_token = format!("{user}.{version}.{}", random::token());
        let digest = self.key.digest(&refresh_token);
        admin_login::save_refresh_token(&self.db, &digest, now + REFRESH_LIFETIME).await?;
        Ok([
            self.password_token(now, user, version),
            self.cookie(REFRESH_TOKEN, &refresh_token, REFRESH_LIFETIME),
        ])
    }

    /// A password user as they are now, if their password is still the one of `version`:
    /// the config's, at version 0, or the database's.
    async fn password_host(&self, user: &str, version: i32) -> sqlx::Result<Option<Host>> {
        if let Some(login) = self.credentials.find(user) {
            return Ok((version == 0).then(|| Host::new(&login.user, login.role)));
        }
        Ok(admin_user::find(&self.db, user)
            .await?
            .filter(|stored| stored.version == version)
            .map(|stored| Host::new(&stored.name, stored_role(stored.role.as_deref()))))
    }

    /// Whether a host the provider vouched for may log in: with a role of their own, or
    /// one an event gives them.
    pub(super) async fn admits(&self, user: &str, role: Option<Role>) -> sqlx::Result<bool> {
        Ok(role.is_some() || access::has_any(&self.db, user).await?)
    }

    /// The cookie of a login the provider vouched for.
    pub(super) fn start_oidc_session(&self, login: &oidc::Login) -> HeaderValue {
        let signed = Login::Provider { role: login.role };
        let token = self.token(Utc::now(), &login.user, signed, OIDC_SESSION);
        self.cookie(TOKEN, &token, OIDC_SESSION)
    }

    async fn session(&self, headers: &HeaderMap) -> sqlx::Result<Session> {
        let now = Utc::now();
        if let Some(claims) = self.claims(headers, now)
            && !admin_login::is_revoked(&self.db, claims.jti).await?
        {
            // The provider's role stands for the login; a password user's is read now.
            match claims.login {
                Login::Provider { role } => {
                    return Ok(Session::LoggedIn(Host::new(&claims.user, role)));
                }
                Login::Password { version } => {
                    if let Some(host) = self.password_host(&claims.user, version).await? {
                        return Ok(Session::LoggedIn(host));
                    }
                }
            }
        }
        // `{user}.{version}.{random}`: the random part never has a dot; a name may.
        if let Some(refresh_token) = cookie::get(headers, REFRESH_TOKEN)
            && let Some((rest, _)) = refresh_token.rsplit_once('.')
            && let Some((user, version)) = rest.rsplit_once('.')
            && let Ok(version) = version.parse()
            && let Some(host) = self.password_host(user, version).await?
            && admin_login::has_refresh_token(&self.db, &self.key.digest(refresh_token)).await?
        {
            let token = self.password_token(now, user, version);
            return Ok(Session::Refreshed(token, host));
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

/// A database user's role, as stored.
fn stored_role(role: Option<&str>) -> Option<Role> {
    role.and_then(Role::from_name)
}

/// Lets a request through if it's logged in, and sends it to the login page otherwise.
pub async fn require_login(
    State(auth): State<Arc<Auth>>,
    request: Request,
    next: Next,
) -> Response {
    match auth.session(request.headers()).await {
        Ok(Session::LoggedIn(host)) => host.scope(next.run(request)).await,
        Ok(Session::Refreshed(token, host)) => {
            let mut response = host.scope(next.run(request)).await;
            response.headers_mut().append(header::SET_COOKIE, token);
            response
        }
        Ok(Session::LoggedOut) => {
            Redirect::to(&paths::admin(auth.login_path().await)).into_response()
        }
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

    /// `admin` with the password `secret`, the editor `ola.n` with `ola`, `nils` with no
    /// role of their own with `nils`, and no provider.
    pub fn admin_config() -> AdminConfig {
        let user = |user: &str, role, password| PasswordLogin {
            user: user.into(),
            role,
            password_hash: cheap_hash(password),
        };
        AdminConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            path: String::new(),
            users: vec![
                user("admin", Some(Role::Admin), "secret"),
                user("ola.n", Some(Role::Editor), "ola"),
                user("nils", None, "nils"),
            ],
            oidc: None,
            jwt_secret: jwt::Secret::from_base64(jwt::tests::SECRET).unwrap(),
        }
    }

    /// A JWT for this user and role that's valid now, as the provider would vouch for
    /// them, for tests that don't log in through a form.
    pub fn token(config: &AdminConfig, db: PgPool, user: &str, role: Option<Role>) -> String {
        let login = Login::Provider { role };
        Auth::new(config, true, db).token(Utc::now(), user, login, TOKEN_LIFETIME)
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
        let admin = Outcome::Accepted("admin".into(), Some(Role::Admin), 0);
        assert_eq!(check(1, "secret").await, admin);
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
        assert_eq!(check(2, "secret").await, admin);
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
