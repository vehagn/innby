//! JSON Web Tokens for the host's login, made the way LLDAP makes them: HS512, naming the
//! user and how they logged in, and when the token was issued and expires. Only the
//! header this module writes is accepted, so a token can't pick its own algorithm.
//!
//! The claims use the registered names where there are any: `sub` for the user, `amr`
//! (RFC 8176) `["pwd"]` for a password login, and `roles` (RFC 9068) for the role a
//! provider gave. No standard says a provider vouched for the user, so `provider` is
//! Innby's own. A password user's role is never in the token, as it's read afresh on
//! each request; there's nothing there to mistake for it.

use std::fmt;

use anyhow::{Result, anyhow, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use uuid::Uuid;

use crate::admin::role::Role;

/// `{"alg":"HS512","typ":"JWT"}`, base64url-encoded.
const HEADER: &str = "eyJhbGciOiJIUzUxMiIsInR5cCI6IkpXVCJ9";
const MIN_SECRET_LEN: usize = 32;

/// `INNBY_ADMIN_JWT_SECRET`, decoded.
#[derive(Clone)]
pub struct Secret(Vec<u8>);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// Parses a standard base64 secret of at least 32 bytes, e.g. from `openssl rand -base64 32`.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let secret = STANDARD
            .decode(encoded.trim())
            .map_err(|_| anyhow!("not valid base64"))?;
        ensure!(
            secret.len() >= MIN_SECRET_LEN,
            "must decode to at least {MIN_SECRET_LEN} bytes, got {}",
            secret.len()
        );
        Ok(Self(secret))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Wire", try_from = "Wire")]
pub struct Claims {
    /// Issued at, in seconds since the epoch.
    pub iat: i64,
    /// Expires at, in seconds since the epoch.
    pub exp: i64,
    pub user: String,
    pub login: Login,
    /// Names the token when it's logged out.
    pub jti: Uuid,
}

/// How the host logged in, which decides where their role comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    /// With a password: their role is read from the config or the database on each
    /// request, so a change applies at once. `version` is a database user's password
    /// version, 0 for the config's: a new password ends the session.
    Password { version: i32 },
    /// The identity provider vouched for them, and for their role for every event;
    /// without one, they see the events that give them one. Taken at its word, as
    /// Innby can't ask the provider on each request.
    Provider { role: Option<Role> },
}

/// The claims as the token holds them.
#[derive(Serialize, Deserialize)]
struct Wire {
    iat: i64,
    exp: i64,
    sub: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    amr: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    provider: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roles: Vec<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<i32>,
    jti: Uuid,
}

/// The `amr` of a password login.
const PASSWORD: &str = "pwd";

impl From<Claims> for Wire {
    fn from(claims: Claims) -> Self {
        let (amr, provider, roles, version) = match claims.login {
            Login::Password { version } => {
                (vec![PASSWORD.to_owned()], false, vec![], Some(version))
            }
            Login::Provider { role } => (vec![], true, role.into_iter().collect(), None),
        };
        Self {
            iat: claims.iat,
            exp: claims.exp,
            sub: claims.user,
            amr,
            provider,
            roles,
            version,
            jti: claims.jti,
        }
    }
}

impl TryFrom<Wire> for Claims {
    type Error = &'static str;

    /// Only the shapes `From<Claims>` writes: a token this key signed is one of them.
    fn try_from(wire: Wire) -> Result<Self, Self::Error> {
        let login = match wire {
            Wire {
                provider: false,
                ref amr,
                ref roles,
                version: Some(version),
                ..
            } if amr == &[PASSWORD] && roles.is_empty() => Login::Password { version },
            Wire {
                provider: true,
                ref amr,
                ref roles,
                version: None,
                ..
            } if amr.is_empty() && roles.len() <= 1 => Login::Provider {
                role: roles.first().copied(),
            },
            _ => return Err("neither a password's claims nor a provider's"),
        };
        Ok(Self {
            iat: wire.iat,
            exp: wire.exp,
            user: wire.sub,
            login,
            jti: wire.jti,
        })
    }
}

/// Signs and checks tokens.
#[derive(Clone)]
pub struct Key(Hmac<Sha512>);

impl Key {
    /// The secret bound to these fields, such as the user name and password hash:
    /// changing any of them logs every session out.
    pub fn new(secret: &Secret, bound: &[&[u8]]) -> Self {
        let mut derive = mac(&secret.0);
        for field in bound {
            derive.update(&(field.len() as u32).to_be_bytes());
            derive.update(field);
        }
        Self(mac(&derive.finalize().into_bytes()))
    }

    pub fn sign(&self, claims: &Claims) -> String {
        let claims = serde_json::to_vec(claims).expect("claims serialize");
        let message = format!("{HEADER}.{}", URL_SAFE_NO_PAD.encode(claims));
        let signature = self.mac(message.as_bytes()).finalize().into_bytes();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    /// The claims of a token this key signed, if it hasn't expired by `now`.
    pub fn verify(&self, token: &str, now: DateTime<Utc>) -> Option<Claims> {
        let (message, signature) = token.rsplit_once('.')?;
        let (header, claims) = message.split_once('.')?;
        if header != HEADER {
            return None;
        }
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        self.mac(message.as_bytes()).verify_slice(&signature).ok()?;
        let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).ok()?).ok()?;
        (now.timestamp() < claims.exp).then_some(claims)
    }

    /// What the database keeps of a refresh token. Keyed, so a new password or secret
    /// makes the stored ones useless too.
    pub fn digest(&self, refresh_token: &str) -> Vec<u8> {
        let mut mac = self.mac(b"refresh token:");
        mac.update(refresh_token.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    /// A value and its signature, for a cookie of this purpose.
    pub fn seal(&self, purpose: &str, value: &[u8]) -> String {
        let value = URL_SAFE_NO_PAD.encode(value);
        let tag = self.seal_mac(purpose, &value).finalize().into_bytes();
        format!("{value}.{}", URL_SAFE_NO_PAD.encode(tag))
    }

    /// The value, if this key sealed it for this purpose.
    pub fn open(&self, purpose: &str, sealed: &str) -> Option<Vec<u8>> {
        let (value, tag) = sealed.split_once('.')?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        self.seal_mac(purpose, value).verify_slice(&tag).ok()?;
        URL_SAFE_NO_PAD.decode(value).ok()
    }

    fn seal_mac(&self, purpose: &str, value: &str) -> Hmac<Sha512> {
        // Unlike a JWT's, this message starts with neither a header nor a dot.
        let mut mac = self.mac(b"sealed:");
        mac.update(&(purpose.len() as u32).to_be_bytes());
        mac.update(purpose.as_bytes());
        mac.update(value.as_bytes());
        mac
    }

    fn mac(&self, message: &[u8]) -> Hmac<Sha512> {
        let mut mac = self.0.clone();
        mac.update(message);
        mac
    }
}

fn mac(key: &[u8]) -> Hmac<Sha512> {
    Hmac::new_from_slice(key).expect("HMAC takes keys of any length")
}

#[cfg(test)]
pub mod tests {
    use chrono::TimeDelta;

    use super::*;

    /// `dev-only-admin-jwt-secret-do-not-use-in-prod`, the one in `.env.example`.
    pub const SECRET: &str = "ZGV2LW9ubHktYWRtaW4tand0LXNlY3JldC1kby1ub3QtdXNlLWluLXByb2Q=";

    fn key_for(password_hash: &str) -> Key {
        Key::new(
            &Secret::from_base64(SECRET).unwrap(),
            &[b"admin", password_hash.as_bytes()],
        )
    }

    fn claims(now: DateTime<Utc>) -> Claims {
        Claims {
            iat: now.timestamp(),
            exp: (now + TimeDelta::hours(1)).timestamp(),
            user: "admin".into(),
            login: Login::Provider {
                role: Some(Role::Editor),
            },
            jti: Uuid::nil(),
        }
    }

    /// The token's claims, as JSON.
    fn payload(token: &str) -> serde_json::Value {
        let claims = token.split('.').nth(1).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap()
    }

    #[test]
    fn names_its_claims_as_the_standards_do() {
        let now = Utc::now();
        let key = key_for("hash");
        let password = Claims {
            login: Login::Password { version: 3 },
            ..claims(now)
        };
        let token = key.sign(&password);
        let json = payload(&token);
        assert_eq!(json["sub"], "admin");
        assert_eq!(json["amr"], serde_json::json!(["pwd"]));
        assert_eq!(json["version"], 3);
        // A password user's role isn't the token's to say.
        for absent in ["roles", "role", "provider", "user"] {
            assert!(json.get(absent).is_none(), "{absent} in {json}");
        }
        assert_eq!(key.verify(&token, now), Some(password));

        let token = key.sign(&claims(now));
        let json = payload(&token);
        assert_eq!(json["provider"], true);
        assert_eq!(json["roles"], serde_json::json!(["editor"]));
        assert!(json.get("amr").is_none() && json.get("version").is_none());
        // A provider's login with no role of its own has none in the token either.
        let none = Claims {
            login: Login::Provider { role: None },
            ..claims(now)
        };
        let token = key.sign(&none);
        assert!(payload(&token).get("roles").is_none());
        assert_eq!(key.verify(&token, now), Some(none));
    }

    #[test]
    fn header_is_hs512() {
        assert_eq!(
            URL_SAFE_NO_PAD.encode(r#"{"alg":"HS512","typ":"JWT"}"#),
            HEADER
        );
    }

    #[test]
    fn verifies_its_own_tokens_until_they_expire() {
        let now = Utc::now();
        let key = key_for("hash");
        let token = key.sign(&claims(now));
        assert_eq!(key.verify(&token, now), Some(claims(now)));
        assert_eq!(key.verify(&token, now + TimeDelta::hours(1)), None);
    }

    #[test]
    fn rejects_tampered_and_foreign_tokens() {
        let now = Utc::now();
        let key = key_for("hash");
        let token = key.sign(&claims(now));
        let (message, _) = token.rsplit_once('.').unwrap();

        // Another password hash, as after `just innby hash-password`, makes another key.
        assert_eq!(key_for("new hash").verify(&token, now), None);
        // Claims that weren't signed.
        let mut longer = claims(now);
        longer.exp += 3600;
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&longer).unwrap());
        let signature = token.rsplit('.').next().unwrap();
        assert_eq!(
            key.verify(&format!("{HEADER}.{forged}.{signature}"), now),
            None
        );
        // No signature, and another algorithm.
        let none = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let (_, claims) = message.split_once('.').unwrap();
        assert_eq!(key.verify(&format!("{none}.{claims}."), now), None);
        assert_eq!(key.verify(message, now), None);
        assert_eq!(key.verify(&format!("{token}.x"), now), None);
    }

    #[test]
    fn seals_values_for_one_purpose() {
        let key = key_for("hash");
        let sealed = key.seal("oidc_flow", b"{\"state\":1}");
        assert_eq!(key.open("oidc_flow", &sealed).unwrap(), b"{\"state\":1}");
        assert_eq!(key.open("other", &sealed), None);
        assert_eq!(key_for("new hash").open("oidc_flow", &sealed), None);
        let (_, tag) = sealed.split_once('.').unwrap();
        let forged = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(b"{\"state\":2}"));
        assert_eq!(key.open("oidc_flow", &forged), None);
        assert_eq!(key.open("oidc_flow", "nonsense"), None);
    }

    #[test]
    fn secrets_must_be_long_base64() {
        assert!(Secret::from_base64(SECRET).is_ok());
        assert!(Secret::from_base64("c2hvcnQ=").is_err());
        assert!(Secret::from_base64("not base64!").is_err());
    }
}
