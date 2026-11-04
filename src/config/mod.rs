//! Settings, from `INNBY_*` environment variables (`.env.example` lists them all).
//!
//! - `mod.rs`: `Config`, what every workload needs, and `database_url`.
//! - `admin`: `AdminConfig`, the admin UI's logins and where it's served.
//! - `mail`: `MailConfig`, the mailer's SMTP server and sender.
//! - `env`: reading and checking values, from any lookup so tests needn't touch the
//!   process environment.

mod admin;
mod env;
mod mail;

pub use admin::{AdminConfig, OIDC_CALLBACK, PasswordLogin};
pub use mail::MailConfig;

use std::net::SocketAddr;

use anyhow::{Context, Result, ensure};

use env::{Env, process_env};

use crate::link::LinkKey;

/// Secrets that `.env.example` publishes. Anyone could sign links or log in with them,
/// so a public site, one with an https base URL, refuses them.
const EXAMPLE_LINK_KEY: &str = "ZGV2LW9ubHktbGluay1rZXktZG8tbm90LXVzZS1pbi1wcm9k";

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    /// The public site's origin, e.g. `https://fest.example.com`, for links the admin UI
    /// and mails hand out.
    pub base_url: String,
    pub link_key: LinkKey,
    /// Take the client address from the last `X-Forwarded-For` entry, which the gateway
    /// appends. Only safe when every request comes through that gateway.
    pub trust_forwarded: bool,
    /// Who is responsible for the guests' data, for the privacy notice.
    pub controller: Controller,
}

impl Config {
    /// Whether the site is served over HTTPS, which turns on HSTS.
    pub fn is_https(&self) -> bool {
        self.base_url.starts_with("https://")
    }

    /// The full URL of a path on the public site, for mails and the admin UI.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

#[derive(Debug, Clone)]
pub struct Controller {
    pub name: String,
    pub email: String,
}

/// The Postgres connection string. Kept apart from `Config` because `innby migrate`
/// needs nothing else.
pub fn database_url() -> Result<String> {
    Env(process_env).required("DATABASE_URL")
}

/// Reads settings from any key lookup, so tests don't touch the process environment.
/// Blank values count as unset.
impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(process_env)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let env = Env(get);
        let listen = env.socket_address("INNBY_LISTEN", "0.0.0.0:8080")?;

        let base_url = env.required("INNBY_BASE_URL")?;
        let base_url = base_url.trim_end_matches('/').to_owned();
        let host = base_url
            .strip_prefix("https://")
            .or_else(|| base_url.strip_prefix("http://"));
        ensure!(
            host.is_some_and(|host| !host.is_empty() && !host.contains('/')),
            "INNBY_BASE_URL: expected an origin like https://fest.example.com, got {base_url:?}"
        );

        let link_key = env.required("INNBY_LINK_KEY")?;
        ensure!(
            !base_url.starts_with("https://") || link_key.trim() != EXAMPLE_LINK_KEY,
            "INNBY_LINK_KEY: that's the example from .env.example, which anyone can read"
        );
        let link_key = LinkKey::from_base64(&link_key).context("INNBY_LINK_KEY")?;
        let trust_forwarded = match env.optional("INNBY_TRUST_FORWARDED").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                anyhow::bail!("INNBY_TRUST_FORWARDED: expected true or false, got {other:?}")
            }
        };

        let controller_email = env.required("INNBY_CONTROLLER_EMAIL")?;
        ensure!(
            crate::db::rsvp::normalize_email(&controller_email).is_some(),
            "INNBY_CONTROLLER_EMAIL: not an e-mail address: {controller_email:?}"
        );

        Ok(Self {
            listen,
            base_url,
            link_key,
            trust_forwarded,
            controller: Controller {
                name: env.required("INNBY_CONTROLLER_NAME")?,
                email: controller_email,
            },
        })
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;

    use super::*;

    const VALID: &[(&str, &str)] = &[
        ("INNBY_BASE_URL", "https://fest.example.com"),
        ("INNBY_LINK_KEY", crate::link::tests::KEY),
        ("INNBY_CONTROLLER_NAME", "Kari Nordmann"),
        ("INNBY_CONTROLLER_EMAIL", "kari@example.com"),
    ];

    pub fn parse(overrides: &[(&str, &str)]) -> Result<Config> {
        let mut env: HashMap<&str, &str> = VALID.iter().copied().collect();
        env.extend(overrides.iter().copied());
        Config::from_lookup(|key| env.get(key).map(|value| value.to_string()))
    }

    /// A valid config for tests in other modules.
    pub fn config() -> Config {
        parse(&[]).expect("valid test config")
    }

    #[test]
    fn parses_valid_config_with_defaults() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.listen, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.controller.name, "Kari Nordmann");
    }

    #[test]
    fn blank_required_value_is_an_error() {
        let err = parse(&[("INNBY_CONTROLLER_NAME", " ")]).unwrap_err();
        assert_eq!(err.to_string(), "INNBY_CONTROLLER_NAME must be set");
    }

    #[test]
    fn public_site_refuses_the_example_secrets() {
        let local = [("INNBY_BASE_URL", "http://localhost:8080")];
        let example = ("INNBY_LINK_KEY", EXAMPLE_LINK_KEY);
        let err = parse(&[example]).unwrap_err();
        assert!(err.to_string().contains(".env.example"), "{err}");
        parse(&[local[0], example]).unwrap();
    }

    #[test]
    fn example_secrets_are_the_ones_in_env_example() {
        let example = include_str!("../../.env.example");
        let value = |key: &str| {
            example
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .unwrap_or_else(|| panic!("{key} in .env.example"))
        };
        assert_eq!(value("INNBY_LINK_KEY"), EXAMPLE_LINK_KEY);
        assert_eq!(
            value("INNBY_ADMIN_JWT_SECRET"),
            admin::EXAMPLE_ADMIN_JWT_SECRET
        );
        assert_eq!(
            admin::EXAMPLE_ADMIN_JWT_SECRET,
            crate::admin::auth::jwt::tests::SECRET
        );
        let hash = value("INNBY_ADMIN_PASSWORD_HASH").trim_matches('\'');
        assert!(crate::admin::auth::verify(
            hash,
            admin::EXAMPLE_ADMIN_PASSWORD
        ));
    }

    #[test]
    fn trusts_forwarded_for_only_when_told() {
        assert!(!parse(&[]).unwrap().trust_forwarded);
        assert!(
            parse(&[("INNBY_TRUST_FORWARDED", "true")])
                .unwrap()
                .trust_forwarded
        );
        assert!(parse(&[("INNBY_TRUST_FORWARDED", "yes")]).is_err());
    }

    #[test]
    fn base_url_must_be_an_origin() {
        let config = parse(&[("INNBY_BASE_URL", "http://localhost:8080/")]).unwrap();
        assert_eq!(config.base_url, "http://localhost:8080");
        for bad in [
            "fest.example.com",
            "https://",
            "https://fest.example.com/rsvp",
        ] {
            assert!(parse(&[("INNBY_BASE_URL", bad)]).is_err(), "{bad}");
        }
    }
}
