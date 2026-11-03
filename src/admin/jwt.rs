//! JSON Web Tokens for the host's login, made the way LLDAP makes them: HS512, naming the
//! user and when the token was issued and expires. Only the header this module writes is
//! accepted, so a token can't pick its own algorithm.

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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Issued at, in seconds since the epoch.
    pub iat: i64,
    /// Expires at, in seconds since the epoch.
    pub exp: i64,
    pub user: String,
    /// Names the token when it's logged out.
    pub jti: Uuid,
}

/// Signs and checks tokens.
#[derive(Clone)]
pub struct Key(Hmac<Sha512>);

impl Key {
    /// The secret bound to the user name and password hash: changing either logs every
    /// session out.
    pub fn new(secret: &Secret, user: &str, password_hash: &str) -> Self {
        let mut derive = mac(&secret.0);
        for field in [user.as_bytes(), password_hash.as_bytes()] {
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
            "admin",
            password_hash,
        )
    }

    fn claims(now: DateTime<Utc>) -> Claims {
        Claims {
            iat: now.timestamp(),
            exp: (now + TimeDelta::hours(1)).timestamp(),
            user: "admin".into(),
            jti: Uuid::nil(),
        }
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
    fn secrets_must_be_long_base64() {
        assert!(Secret::from_base64(SECRET).is_ok());
        assert!(Secret::from_base64("c2hvcnQ=").is_err());
        assert!(Secret::from_base64("not base64!").is_err());
    }
}
