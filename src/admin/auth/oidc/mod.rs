//! Logging in to the admin UI through an OpenID Connect provider such as Authelia, beside
//! the password users or instead of them.
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
//!
//! - `mod.rs`: `OidcConfig` and the flow's lifetimes.
//! - `routes`: the two endpoints. `provider`: discovery, keys, the token exchange and
//!   userinfo. `jws`: checking ID tokens. `http`: the HTTP client. `fake`: a provider
//!   for the tests.

mod http;
pub mod jws;
mod provider;
mod routes;

use provider::{Callback, Flow, LoginError};
pub use provider::{Login, Provider};
pub use routes::{callback, start};

use std::{fmt, time::Duration};

use anyhow::{Context, Result, ensure};
use chrono::TimeDelta;
use url::Url;

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

#[cfg(test)]
pub mod fake;
