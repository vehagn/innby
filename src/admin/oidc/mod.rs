//! Logging in to the admin UI through an OpenID Connect provider such as Authelia, as an
//! alternative or a replacement for the built-in user.
//!
//! The authorization code flow, for a confidential client:
//! 1. `/login/oidc` makes a `state`, a `nonce` and a PKCE verifier, keeps them in a
//!    short-lived signed cookie, and sends the browser to the provider.
//! 2. The provider sends it back to `/login/oidc/callback` with a code. The `state` must
//!    match the cookie's, so nobody can log a browser in to their account (login CSRF).
//! 3. Innby trades the code for tokens, authenticating with its client secret and the
//!    PKCE verifier, and checks the ID token (see `jws`).
//! 4. The roles claim, from the ID token or else from userinfo (whose `sub` must be the
//!    ID token's), gives the host's role. No role, no login.
//!
//! The provider's settings come from its discovery document, whose `issuer` must be the
//! configured one exactly, and are kept for an hour. Its keys are fetched again when a
//! token names one Innby hasn't seen, at most once a minute.
//!
//! The login lasts `OIDC_SESSION` without a refresh token; after that the provider is
//! asked again, which is seamless while the host is logged in there.

mod http;
pub mod jws;

use std::{
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use url::Url;

use super::{
    auth::{self, Auth},
    lang,
    role::Role,
};
use crate::{i18n::fill, paths};

/// How long a login through the provider lasts before the provider is asked again.
pub const OIDC_SESSION: TimeDelta = TimeDelta::hours(12);
/// How long the host has at the provider before the login must start over.
const FLOW_LIFETIME: TimeDelta = TimeDelta::minutes(10);
/// The cookie that holds a login in progress.
pub const FLOW_COOKIE: &str = "oidc_flow";
const METADATA_TTL: Duration = Duration::from_secs(60 * 60);
const KEYS_TTL: Duration = Duration::from_secs(60 * 60);
/// A token with an unknown key fetches the keys again, but not more often than this.
const KEYS_MIN_AGE: Duration = Duration::from_secs(60);

/// `INNBY_OIDC_*`.
#[derive(Clone)]
pub struct OidcConfig {
    /// Exactly as the provider's discovery document has it, e.g.
    /// `https://authelia.example.com`.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// The provider's name on the login button.
    pub name: String,
    /// Space-separated, with `openid`.
    pub scopes: String,
    /// The claim that lists the host's Innby roles, named after the app as the homelab
    /// names them: `innby: [editor]`.
    pub roles_claim: String,
    /// `{INNBY_ADMIN_URL}/login/oidc/callback`, to register with the provider.
    pub redirect_uri: Url,
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"..")
            .field("name", &self.name)
            .field("scopes", &self.scopes)
            .field("roles_claim", &self.roles_claim)
            .field("redirect_uri", &self.redirect_uri.as_str())
            .finish()
    }
}

impl OidcConfig {
    /// Where the provider's discovery document is: the issuer with
    /// `/.well-known/openid-configuration` after it.
    fn discovery(&self) -> Result<Url> {
        let url: Url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        )
        .parse()?;
        http::check_url(&url)?;
        Ok(url)
    }

    /// Rejects settings that can't work.
    pub fn check(&self) -> Result<()> {
        let issuer: Url = self.issuer.parse().context("not a URL")?;
        ensure!(
            issuer.query().is_none() && issuer.fragment().is_none(),
            "an issuer has no query or fragment"
        );
        self.discovery()?;
        ensure!(
            self.scopes
                .split_whitespace()
                .any(|scope| scope == "openid"),
            "INNBY_OIDC_SCOPES must include openid"
        );
        Ok(())
    }
}

/// What the discovery document says, checked.
struct Metadata {
    authorization_endpoint: Url,
    token_endpoint: Url,
    jwks_uri: Url,
    userinfo_endpoint: Option<Url>,
    /// RFC 9207: the provider names itself in the callback, so another provider the
    /// browser was sent to can't pass its code off as this one's.
    iss_parameter: bool,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: Option<String>,
    code_challenge_methods_supported: Option<Vec<String>>,
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    authorization_response_iss_parameter_supported: bool,
}

impl Discovery {
    fn check(self, config: &OidcConfig) -> Result<Metadata> {
        ensure!(
            self.issuer == config.issuer,
            "the provider calls itself {:?}, not {:?}; set INNBY_OIDC_ISSUER to exactly that",
            self.issuer,
            config.issuer
        );
        if let Some(methods) = &self.code_challenge_methods_supported {
            ensure!(
                methods.iter().any(|m| m == "S256"),
                "the provider lacks PKCE with S256"
            );
        }
        // Absent means client_secret_basic, says OpenID Connect Discovery.
        if let Some(methods) = &self.token_endpoint_auth_methods_supported {
            ensure!(
                methods.iter().any(|m| m == "client_secret_basic"),
                "the provider lacks client_secret_basic, which Innby authenticates with"
            );
        }
        let url = |name: &str, value: &str| -> Result<Url> {
            let url: Url = value
                .parse()
                .with_context(|| format!("{name}: {value:?}"))?;
            http::check_url(&url).with_context(|| name.to_owned())?;
            Ok(url)
        };
        Ok(Metadata {
            authorization_endpoint: url("authorization_endpoint", &self.authorization_endpoint)?,
            token_endpoint: url("token_endpoint", &self.token_endpoint)?,
            jwks_uri: url("jwks_uri", &self.jwks_uri)?,
            userinfo_endpoint: self
                .userinfo_endpoint
                .map(|value| url("userinfo_endpoint", &value))
                .transpose()?,
            iss_parameter: self.authorization_response_iss_parameter_supported,
        })
    }
}

struct Fetched<T> {
    at: Instant,
    value: Arc<T>,
}

/// The provider: its settings, and what it published.
pub struct Provider {
    config: OidcConfig,
    metadata: Mutex<Option<Fetched<Metadata>>>,
    keys: Mutex<Option<Fetched<jws::JwkSet>>>,
    /// `KEYS_MIN_AGE`, but none in the tests.
    keys_min_age: Duration,
}

/// A login in progress, kept in a signed cookie in the host's browser.
#[derive(Debug, Serialize, Deserialize)]
pub struct Flow {
    state: String,
    nonce: String,
    /// The PKCE code verifier.
    verifier: String,
    /// Seconds since the epoch.
    exp: i64,
}

/// What the provider sends the browser back with.
#[derive(Debug, Default, Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// A login the provider vouched for.
#[derive(Debug, PartialEq, Eq)]
pub struct Login {
    pub user: String,
    pub role: Role,
}

#[derive(Debug)]
pub enum LoginError {
    /// No login was started in this browser, or it took too long.
    Expired,
    /// The host said no at the provider.
    Cancelled,
    /// The provider knows the host, but lists no Innby role for them.
    NoRole { user: String },
    /// Something's wrong between Innby and the provider: for the log.
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for LoginError {
    fn from(err: anyhow::Error) -> Self {
        LoginError::Failed(err)
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    id_token: Option<String>,
}

fn random() -> String {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).expect("the system has randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}

impl Provider {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            metadata: Mutex::new(None),
            keys: Mutex::new(None),
            keys_min_age: KEYS_MIN_AGE,
        }
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    async fn metadata(&self) -> Result<Arc<Metadata>> {
        let mut cached = self.metadata.lock().await;
        if let Some(fetched) = &*cached
            && fetched.at.elapsed() < METADATA_TTL
        {
            return Ok(Arc::clone(&fetched.value));
        }
        let fetch = async {
            let url = self.config.discovery()?;
            let response = http::get(&url, None).await?;
            ensure!(
                response.status == StatusCode::OK,
                "{url}: {}",
                response.status
            );
            response
                .json::<Discovery>()
                .with_context(|| url.to_string())?
                .check(&self.config)
        };
        match fetch.await {
            Ok(metadata) => {
                let value = Arc::new(metadata);
                *cached = Some(Fetched {
                    at: Instant::now(),
                    value: Arc::clone(&value),
                });
                Ok(value)
            }
            // A provider that's briefly away doesn't stop logins it already described.
            Err(err) => match &*cached {
                Some(fetched) => {
                    tracing::warn!("OIDC discovery failed, using what it said before: {err:#}");
                    Ok(Arc::clone(&fetched.value))
                }
                None => Err(err.context("OIDC discovery")),
            },
        }
    }

    /// The provider's keys; with `again`, fetched anew unless they just were.
    async fn keys(&self, metadata: &Metadata, again: bool) -> Result<Arc<jws::JwkSet>> {
        let mut cached = self.keys.lock().await;
        if let Some(fetched) = &*cached {
            let age = fetched.at.elapsed();
            if age < KEYS_TTL && !(again && age >= self.keys_min_age) {
                return Ok(Arc::clone(&fetched.value));
            }
        }
        let response = http::get(&metadata.jwks_uri, None).await?;
        ensure!(
            response.status == StatusCode::OK,
            "{}: {}",
            metadata.jwks_uri,
            response.status
        );
        let keys: jws::JwkSet = response.json().context("the provider's keys")?;
        let value = Arc::new(keys);
        *cached = Some(Fetched {
            at: Instant::now(),
            value: Arc::clone(&value),
        });
        Ok(value)
    }

    /// Where to send the browser, and the login to keep until it's back.
    pub async fn start(&self, now: DateTime<Utc>) -> Result<(Url, Flow)> {
        let metadata = self.metadata().await?;
        let flow = Flow {
            state: random(),
            nonce: random(),
            verifier: random(),
            exp: (now + FLOW_LIFETIME).timestamp(),
        };
        let challenge = digest::digest(&digest::SHA256, flow.verifier.as_bytes());
        let mut url = metadata.authorization_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", self.config.redirect_uri.as_str())
            .append_pair("scope", &self.config.scopes)
            .append_pair("state", &flow.state)
            .append_pair("nonce", &flow.nonce)
            .append_pair("code_challenge", &URL_SAFE_NO_PAD.encode(challenge))
            .append_pair("code_challenge_method", "S256");
        Ok((url, flow))
    }

    /// Finishes the login the browser came back from.
    pub async fn finish(
        &self,
        flow: &Flow,
        callback: &Callback,
        now: DateTime<Utc>,
    ) -> Result<Login, LoginError> {
        let state_ok = callback
            .state
            .as_deref()
            .is_some_and(|state| state.as_bytes().ct_eq(flow.state.as_bytes()).into());
        if !state_ok || flow.exp <= now.timestamp() {
            return Err(LoginError::Expired);
        }
        let metadata = self.metadata().await?;
        match &callback.iss {
            Some(iss) if *iss != self.config.issuer => {
                return Err(anyhow!("the callback came from issuer {iss:?}").into());
            }
            None if metadata.iss_parameter => {
                return Err(anyhow!("the callback lacks the iss parameter").into());
            }
            _ => {}
        }
        if let Some(error) = &callback.error {
            if error == "access_denied" {
                return Err(LoginError::Cancelled);
            }
            let description = callback.error_description.as_deref().unwrap_or("");
            return Err(anyhow!("the provider said {error}: {description}").into());
        }
        let code = callback
            .code
            .as_deref()
            .ok_or_else(|| anyhow!("the callback has no code"))?;

        let response = http::post_form(
            &metadata.token_endpoint,
            (&self.config.client_id, &self.config.client_secret),
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.config.redirect_uri.as_str()),
                ("code_verifier", &flow.verifier),
            ],
        )
        .await?;
        if response.status != StatusCode::OK {
            return Err(anyhow!("the token endpoint said {}", response.error()).into());
        }
        let tokens: TokenResponse = response.json().context("the token response")?;
        if !tokens.token_type.eq_ignore_ascii_case("Bearer") {
            return Err(anyhow!("token type {:?}, not Bearer", tokens.token_type).into());
        }
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or_else(|| anyhow!("no ID token; is openid among the scopes?"))?;

        let mut claims = self
            .check_id_token(&metadata, id_token, flow, &tokens.access_token, now)
            .await?;
        if !claims.contains_key(&self.config.roles_claim)
            && let Some(userinfo) = &metadata.userinfo_endpoint
        {
            let more = userinfo_claims(userinfo, &tokens.access_token, &claims).await?;
            for (name, value) in more {
                claims.entry(name).or_insert(value);
            }
        }
        let user = ["preferred_username", "name", "email", "sub"]
            .iter()
            .find_map(|name| claims.get(*name).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        match claims
            .get(&self.config.roles_claim)
            .and_then(Role::from_claim)
        {
            Some(role) => Ok(Login { user, role }),
            None => Err(LoginError::NoRole { user }),
        }
    }

    async fn check_id_token(
        &self,
        metadata: &Metadata,
        id_token: &str,
        flow: &Flow,
        access_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Map<String, Value>> {
        let keys = self.keys(metadata, false).await?;
        let verified = match jws::Token::parse(id_token)?.verify(&keys) {
            Err(jws::Error::UnknownKey) => {
                let keys = self.keys(metadata, true).await?;
                jws::Token::parse(id_token)?.verify(&keys)
            }
            verified => verified,
        }?;
        let expected = jws::Expected {
            issuer: &self.config.issuer,
            client_id: &self.config.client_id,
            nonce: &flow.nonce,
            access_token,
            now: now.timestamp(),
        };
        Ok(verified.check(&expected)?)
    }
}

/// The userinfo claims, which must be about the same person as the ID token.
async fn userinfo_claims(
    url: &Url,
    access_token: &str,
    id_claims: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let response = http::get(url, Some(access_token)).await?;
    ensure!(
        response.status == StatusCode::OK,
        "userinfo said {}",
        response.error()
    );
    let claims: Map<String, Value> = response.json().context(
        "userinfo; if it's signed, set the client's userinfo_signed_response_alg to none",
    )?;
    if claims.get("sub") != id_claims.get("sub") {
        bail!("userinfo is about another sub than the ID token");
    }
    Ok(claims)
}

/// `/login/oidc`: off to the provider.
pub async fn start(State(auth): State<Arc<Auth>>) -> Response {
    let Some(provider) = auth.oidc() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match provider.start(Utc::now()).await {
        Ok((url, flow)) => {
            let sealed = auth.seal(
                FLOW_COOKIE,
                &serde_json::to_vec(&flow).expect("flows serialize"),
            );
            let cookie = auth.cookie(FLOW_COOKIE, &sealed, FLOW_LIFETIME);
            (
                AppendHeaders([(header::SET_COOKIE, cookie)]),
                Redirect::to(url.as_str()),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!("OIDC login couldn't start: {err:#}");
            failed(&auth, provider)
        }
    }
}

/// `/login/oidc/callback`: back from the provider.
pub async fn callback(
    State(auth): State<Arc<Auth>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(provider) = auth.oidc() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let field = |name: &str| query.get(name).cloned();
    let callback = Callback {
        code: field("code"),
        state: field("state"),
        iss: field("iss"),
        error: field("error"),
        error_description: field("error_description"),
    };
    let clear = auth.cookie(FLOW_COOKIE, "", TimeDelta::zero());
    let flow = auth::cookie(&headers, FLOW_COOKIE)
        .and_then(|sealed| auth.open(FLOW_COOKIE, sealed))
        .and_then(|json| serde_json::from_slice::<Flow>(&json).ok());
    let result = match &flow {
        Some(flow) => provider.finish(flow, &callback, Utc::now()).await,
        None => Err(LoginError::Expired),
    };
    let text = lang::text();
    let mut response = match result {
        Ok(login) => {
            let cookie = auth.start_oidc_session(&login);
            tracing::info!(
                user = login.user,
                role = login.role.name(),
                "host logged in through OIDC"
            );
            let home = Redirect::to(&paths::admin(paths::HOME));
            (AppendHeaders([(header::SET_COOKIE, cookie)]), home).into_response()
        }
        Err(LoginError::Expired) => {
            tracing::warn!("OIDC callback without a login in progress, or a stale one");
            auth::login_error(&auth, StatusCode::BAD_REQUEST, text.login_expired.clone())
        }
        Err(LoginError::Cancelled) => auth::login_error(
            &auth,
            StatusCode::UNAUTHORIZED,
            text.login_cancelled.clone(),
        ),
        Err(LoginError::NoRole { user }) => {
            tracing::warn!(user, "OIDC login without an Innby role");
            let message = fill(
                &text.no_role,
                &[("name", &provider.name()), ("user", &user)],
            );
            auth::login_error(&auth, StatusCode::FORBIDDEN, message)
        }
        Err(LoginError::Failed(err)) => {
            tracing::error!("OIDC login failed: {err:#}");
            failed(&auth, provider)
        }
    };
    response.headers_mut().append(header::SET_COOKIE, clear);
    response
}

fn failed(auth: &Auth, provider: &Provider) -> Response {
    let message = fill(&lang::text().login_failed, &[("name", &provider.name())]);
    auth::login_error(auth, StatusCode::BAD_GATEWAY, message)
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use serde_json::json;
    use tower::ServiceExt;

    use super::{fake::Fake, *};

    const ADMIN: &str = "https://admin.fest.example.com";

    async fn provider() -> (Fake, Provider) {
        let fake = Fake::start().await;
        let mut provider = Provider::new(fake.config(ADMIN));
        provider.keys_min_age = Duration::ZERO;
        (fake, provider)
    }

    /// The browser's trip to the provider and back: what it brings to the callback.
    async fn authorize(fake: &Fake, url: &Url) -> Callback {
        let path = format!("{}?{}", url.path(), url.query().unwrap());
        let request = Request::get(path).body(Body::empty()).unwrap();
        let response = fake.router.clone().oneshot(request).await.unwrap();
        let location: Url = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            location.as_str().split('?').next().unwrap(),
            format!("{ADMIN}/login/oidc/callback")
        );
        let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
        let field = |name: &str| query.get(name).cloned();
        Callback {
            code: field("code"),
            state: field("state"),
            iss: field("iss"),
            error: field("error"),
            error_description: field("error_description"),
        }
    }

    async fn log_in(fake: &Fake, provider: &Provider) -> Result<Login, LoginError> {
        let (url, flow) = provider.start(Utc::now()).await.unwrap();
        let callback = authorize(fake, &url).await;
        provider.finish(&flow, &callback, Utc::now()).await
    }

    fn failed(result: Result<Login, LoginError>) -> String {
        match result {
            Err(LoginError::Failed(err)) => format!("{err:#}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn logs_in_with_the_role_from_userinfo() {
        let (fake, provider) = provider().await;
        let login = log_in(&fake, &provider).await.unwrap();
        assert_eq!(
            login,
            Login {
                user: "kari".into(),
                role: Role::Editor
            }
        );
        // The ID token has no roles, as Authelia's by default, so userinfo was asked.
        assert_eq!(fake.calls("userinfo"), 1);

        // Discovery and keys are kept for the next login.
        fake.log_in("ola", json!(["viewer", "grafana_admin", "admin"]));
        let login = log_in(&fake, &provider).await.unwrap();
        assert_eq!(login.role, Role::Admin);
        assert_eq!((fake.calls("discovery"), fake.calls("jwks")), (1, 1));
    }

    #[tokio::test]
    async fn sends_pkce_state_and_nonce() {
        let (_fake, provider) = provider().await;
        let (url, flow) = provider.start(Utc::now()).await.unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(query["state"], flow.state);
        assert_eq!(query["nonce"], flow.nonce);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_ne!(query["code_challenge"], flow.verifier);
        assert_eq!(query["scope"], "openid profile email innby");
        assert_eq!(
            query["redirect_uri"],
            format!("{ADMIN}/login/oidc/callback")
        );
        let (_, again) = provider.start(Utc::now()).await.unwrap();
        assert_ne!(flow.state, again.state);
    }

    #[tokio::test]
    async fn turns_away_hosts_without_a_role_and_hears_a_no() {
        let (fake, provider) = provider().await;
        fake.log_in("per", json!(["grafana_admin"]));
        assert!(matches!(
            log_in(&fake, &provider).await,
            Err(LoginError::NoRole { user }) if user == "per"
        ));
        fake.with(|fake| fake.user = None);
        assert!(matches!(
            log_in(&fake, &provider).await,
            Err(LoginError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn the_callback_must_belong_to_this_login() {
        let (fake, provider) = provider().await;
        let (url, flow) = provider.start(Utc::now()).await.unwrap();
        let callback = authorize(&fake, &url).await;

        // Another browser's login, or none.
        let (_, other) = provider.start(Utc::now()).await.unwrap();
        let result = provider.finish(&other, &callback, Utc::now()).await;
        assert!(matches!(result, Err(LoginError::Expired)));
        // Too late.
        let late = Utc::now() + FLOW_LIFETIME;
        let result = provider.finish(&flow, &callback, late).await;
        assert!(matches!(result, Err(LoginError::Expired)));
        // From another provider (RFC 9207), or without saying.
        let elsewhere = Callback {
            iss: Some("https://evil.example.com".into()),
            ..authorize(&fake, &url).await
        };
        let result = provider.finish(&flow, &elsewhere, Utc::now()).await;
        assert!(failed(result).contains("issuer"));
        let unnamed = Callback {
            iss: None,
            ..authorize(&fake, &url).await
        };
        let result = provider.finish(&flow, &unnamed, Utc::now()).await;
        assert!(failed(result).contains("iss"));

        // The right one works, once.
        provider.finish(&flow, &callback, Utc::now()).await.unwrap();
        let result = provider.finish(&flow, &callback, Utc::now()).await;
        assert!(failed(result).contains("invalid_grant"));
    }

    #[tokio::test]
    async fn checks_the_id_token_and_userinfo() {
        let (fake, provider) = provider().await;
        fake.with(|fake| {
            fake.id_claims
                .insert("aud".into(), json!(["innby", "grafana"]))
        });
        assert!(failed(log_in(&fake, &provider).await).contains("aud"));
        fake.with(|fake| fake.id_claims.clear());
        fake.with(|fake| fake.id_claims.insert("nonce".into(), json!("another")));
        assert!(failed(log_in(&fake, &provider).await).contains("nonce"));
        fake.with(|fake| fake.id_claims.clear());
        fake.with(|fake| fake.id_claims.insert("at_hash".into(), json!("AAAA")));
        assert!(failed(log_in(&fake, &provider).await).contains("at_hash"));
        fake.with(|fake| fake.id_claims.clear());

        fake.with(|fake| fake.userinfo_sub = Some("sub-someone-else".into()));
        assert!(failed(log_in(&fake, &provider).await).contains("sub"));
        fake.with(|fake| fake.userinfo_sub = None);

        // Roles in the ID token are used as they are.
        fake.with(|fake| fake.id_claims.insert("innby".into(), json!(["admin"])));
        fake.with(|fake| {
            fake.id_claims
                .insert("preferred_username".into(), json!("kari"))
        });
        let asked = fake.calls("userinfo");
        let login = log_in(&fake, &provider).await.unwrap();
        assert_eq!(login.role, Role::Admin);
        assert_eq!(fake.calls("userinfo"), asked);
    }

    #[tokio::test]
    async fn fetches_new_keys_when_the_provider_rotates_them() {
        let (fake, provider) = provider().await;
        log_in(&fake, &provider).await.unwrap();
        fake.rotate_key();
        log_in(&fake, &provider).await.unwrap();
        assert_eq!(fake.calls("jwks"), 2);
    }

    #[tokio::test]
    async fn the_provider_must_be_the_configured_issuer() {
        let (fake, provider) = provider().await;
        fake.with(|fake| fake.issuer = Some("https://evil.example.com".into()));
        let err = provider.start(Utc::now()).await.unwrap_err();
        assert!(format!("{err:#}").contains("INNBY_OIDC_ISSUER"), "{err:#}");

        let wrong_secret = Provider::new(OidcConfig {
            client_secret: "wrong".into(),
            ..fake.config(ADMIN)
        });
        fake.with(|fake| fake.issuer = None);
        assert!(failed(log_in(&fake, &wrong_secret).await).contains("invalid_client"));
    }
}
