//! The provider's side of a login: discovery, its keys, the authorization request, and
//! finishing the login the browser comes back with.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use url::Url;

use crate::admin::role::Role;

use super::{FLOW_LIFETIME, KEYS_MIN_AGE, KEYS_TTL, METADATA_TTL, OidcConfig, http, jws};
use crate::random;

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
    pub(super) code: Option<String>,
    pub(super) state: Option<String>,
    pub(super) iss: Option<String>,
    pub(super) error: Option<String>,
    pub(super) error_description: Option<String>,
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
            state: random::token(),
            nonce: random::token(),
            verifier: random::token(),
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

#[cfg(test)]
mod tests;
