use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, ensure};

use crate::{
    admin::{jwt, oidc::OidcConfig},
    link::LinkKey,
};

/// Secrets that `.env.example` publishes. Anyone could sign links or log in with them,
/// so a public site, one with an https base URL, refuses them.
const EXAMPLE_LINK_KEY: &str = "ZGV2LW9ubHktbGluay1rZXktZG8tbm90LXVzZS1pbi1wcm9k";
const EXAMPLE_ADMIN_PASSWORD: &str = "innby";
const EXAMPLE_ADMIN_JWT_SECRET: &str =
    "ZGV2LW9ubHktYWRtaW4tand0LXNlY3JldC1kby1ub3QtdXNlLWluLXByb2Q=";

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
struct Env<F>(F);

/// The real lookup, for `Env`.
fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

impl<F: Fn(&str) -> Option<String>> Env<F> {
    fn optional(&self, key: &str) -> Option<String> {
        (self.0)(key).filter(|value| !value.trim().is_empty())
    }

    fn required(&self, key: &str) -> Result<String> {
        self.optional(key)
            .ok_or_else(|| anyhow!("{key} must be set"))
    }

    fn socket_address(&self, key: &str, default: &str) -> Result<SocketAddr> {
        let value = self.optional(key).unwrap_or_else(|| default.into());
        value
            .parse()
            .with_context(|| format!("{key}: invalid socket address {value:?}"))
    }
}

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

/// Settings only `innby admin` needs, on top of `Config`.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    pub listen: SocketAddr,
    /// The built-in user, unless `INNBY_ADMIN_PASSWORD_HASH` is left out.
    pub login: Option<PasswordLogin>,
    /// Single sign-on, when `INNBY_OIDC_ISSUER` is set.
    pub oidc: Option<OidcConfig>,
    /// Signs the login's JWTs.
    pub jwt_secret: jwt::Secret,
}

#[derive(Debug, Clone)]
pub struct PasswordLogin {
    pub user: String,
    /// An argon2 PHC string from `innby hash-password`.
    pub password_hash: String,
}

/// Where the provider sends the host back to, under `INNBY_ADMIN_URL`.
pub const OIDC_CALLBACK: &str = "/login/oidc/callback";

impl AdminConfig {
    pub fn from_env(config: &Config) -> Result<Self> {
        Self::from_lookup(process_env, config)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>, config: &Config) -> Result<Self> {
        let env = Env(get);
        let login = env
            .optional("INNBY_ADMIN_PASSWORD_HASH")
            .map(|password_hash| -> Result<PasswordLogin> {
                crate::admin::auth::check_hash(&password_hash)
                    .context("INNBY_ADMIN_PASSWORD_HASH")?;
                ensure!(
                    !config.is_https()
                        || !crate::admin::auth::verify(&password_hash, EXAMPLE_ADMIN_PASSWORD),
                    "INNBY_ADMIN_PASSWORD_HASH: that's a hash of the example password from \
                     .env.example; make one for a real password with `just innby hash-password`"
                );
                Ok(PasswordLogin {
                    user: env
                        .optional("INNBY_ADMIN_USER")
                        .unwrap_or_else(|| "admin".into()),
                    password_hash,
                })
            })
            .transpose()?;
        let oidc = env.oidc()?;
        ensure!(
            login.is_some() || oidc.is_some(),
            "nobody could log in: set INNBY_ADMIN_PASSWORD_HASH, INNBY_OIDC_ISSUER or both"
        );
        let jwt_secret = env.required("INNBY_ADMIN_JWT_SECRET")?;
        ensure!(
            !config.is_https() || jwt_secret.trim() != EXAMPLE_ADMIN_JWT_SECRET,
            "INNBY_ADMIN_JWT_SECRET: that's the example from .env.example, which anyone can read"
        );
        Ok(Self {
            listen: env.socket_address("INNBY_ADMIN_LISTEN", "0.0.0.0:8081")?,
            login,
            oidc,
            jwt_secret: jwt::Secret::from_base64(&jwt_secret).context("INNBY_ADMIN_JWT_SECRET")?,
        })
    }
}

impl<F: Fn(&str) -> Option<String>> Env<F> {
    fn oidc(&self) -> Result<Option<OidcConfig>> {
        let Some(issuer) = self.optional("INNBY_OIDC_ISSUER") else {
            return Ok(None);
        };
        let admin_url = self.required("INNBY_ADMIN_URL")?;
        let admin_url = admin_url.trim_end_matches('/');
        let redirect_uri = format!("{admin_url}{OIDC_CALLBACK}")
            .parse::<url::Url>()
            .ok()
            .filter(|url| url.path() == OIDC_CALLBACK && url.query().is_none())
            .ok_or_else(|| {
                anyhow!("INNBY_ADMIN_URL: expected an origin like https://admin.example.com")
            })?;
        let config = OidcConfig {
            issuer: issuer.trim().to_owned(),
            client_id: self.required("INNBY_OIDC_CLIENT_ID")?,
            client_secret: self.required("INNBY_OIDC_CLIENT_SECRET")?,
            name: self
                .optional("INNBY_OIDC_NAME")
                .unwrap_or_else(|| "SSO".into()),
            scopes: self
                .optional("INNBY_OIDC_SCOPES")
                .unwrap_or_else(|| "openid profile email innby".into()),
            roles_claim: self
                .optional("INNBY_OIDC_ROLES_CLAIM")
                .unwrap_or_else(|| "innby".into()),
            redirect_uri,
        };
        config.check().context("INNBY_OIDC_ISSUER")?;
        Ok(Some(config))
    }
}

/// Settings only `innby mailer` needs, on top of `Config`.
#[derive(Clone)]
pub struct MailConfig {
    /// `smtp://host:port` with `?tls=required` for STARTTLS, or `smtps://host:port`.
    pub smtp_url: String,
    pub smtp_user: Option<String>,
    pub smtp_password: Option<String>,
    pub from: lettre::message::Mailbox,
}

impl std::fmt::Debug for MailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailConfig")
            .field("smtp_url", &self.smtp_url)
            .field("smtp_user", &self.smtp_user)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

impl MailConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(process_env)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let env = Env(get);
        let from = env.required("INNBY_MAIL_FROM")?;
        let smtp_user = env.optional("INNBY_SMTP_USER");
        let smtp_password = env.optional("INNBY_SMTP_PASSWORD");
        ensure!(
            smtp_user.is_some() == smtp_password.is_some(),
            "INNBY_SMTP_USER and INNBY_SMTP_PASSWORD go together"
        );
        Ok(Self {
            smtp_url: env.required("INNBY_SMTP_URL")?,
            smtp_user,
            smtp_password,
            from: from
                .parse()
                .with_context(|| format!("INNBY_MAIL_FROM: not a mailbox: {from:?}"))?,
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

    fn admin_with(hash: &str, jwt_secret: &str, config: &Config) -> Result<AdminConfig> {
        AdminConfig::from_lookup(
            |key| match key {
                "INNBY_ADMIN_PASSWORD_HASH" => Some(hash.to_owned()),
                "INNBY_ADMIN_JWT_SECRET" => Some(jwt_secret.to_owned()),
                _ => None,
            },
            config,
        )
    }

    const OIDC: &[(&str, &str)] = &[
        ("INNBY_ADMIN_JWT_SECRET", crate::link::tests::KEY),
        ("INNBY_ADMIN_URL", "https://admin.fest.example.com/"),
        ("INNBY_OIDC_ISSUER", "https://authelia.example.com"),
        ("INNBY_OIDC_CLIENT_ID", "innby"),
        ("INNBY_OIDC_CLIENT_SECRET", "s3cret"),
    ];

    fn admin_oidc(overrides: &[(&str, &str)]) -> Result<AdminConfig> {
        let mut env: HashMap<&str, &str> = OIDC.iter().copied().collect();
        env.extend(overrides.iter().copied());
        AdminConfig::from_lookup(|key| env.get(key).map(|value| value.to_string()), &config())
    }

    #[test]
    fn single_sign_on_can_replace_the_built_in_user() {
        let admin = admin_oidc(&[]).unwrap();
        assert!(admin.login.is_none());
        let oidc = admin.oidc.unwrap();
        assert_eq!(
            oidc.redirect_uri.as_str(),
            "https://admin.fest.example.com/login/oidc/callback"
        );
        assert_eq!(
            (
                oidc.scopes.as_str(),
                oidc.roles_claim.as_str(),
                oidc.name.as_str()
            ),
            ("openid profile email innby", "innby", "SSO")
        );
        assert!(!format!("{oidc:?}").contains("s3cret"));

        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let both = admin_oidc(&[("INNBY_ADMIN_PASSWORD_HASH", &hash)]).unwrap();
        assert!(both.login.is_some() && both.oidc.is_some());

        let err = |overrides: &[(&str, &str)]| format!("{:#}", admin_oidc(overrides).unwrap_err());
        assert!(err(&[("INNBY_OIDC_ISSUER", "")]).starts_with("nobody could log in"));
        assert!(err(&[("INNBY_ADMIN_URL", "")]).contains("INNBY_ADMIN_URL"));
        assert!(err(&[("INNBY_ADMIN_URL", "https://admin.example.com/admin")]).contains("origin"));
        assert!(err(&[("INNBY_OIDC_CLIENT_SECRET", "")]).contains("INNBY_OIDC_CLIENT_SECRET"));
        assert!(err(&[("INNBY_OIDC_ISSUER", "http://authelia.example.com")]).contains("https"));
        assert!(err(&[("INNBY_OIDC_SCOPES", "profile innby")]).contains("openid"));
        // A provider on this machine, while developing.
        admin_oidc(&[("INNBY_OIDC_ISSUER", "http://127.0.0.1:9091")]).unwrap();
    }

    fn admin_with_hash(hash: &str, config: &Config) -> Result<AdminConfig> {
        admin_with(hash, crate::link::tests::KEY, config)
    }

    #[test]
    fn admin_config_needs_a_valid_hash() {
        let config = config();
        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let admin = admin_with_hash(&hash, &config).unwrap();
        assert_eq!(admin.login.unwrap().user, "admin");
        assert_eq!(admin.listen, "0.0.0.0:8081".parse().unwrap());

        assert!(AdminConfig::from_lookup(|_| None, &config).is_err());
        let err = admin_with_hash("hunter2", &config).unwrap_err();
        assert!(format!("{err:#}").starts_with("INNBY_ADMIN_PASSWORD_HASH"));
        let err = admin_with(&hash, "c2hvcnQ=", &config).unwrap_err();
        assert!(format!("{err:#}").starts_with("INNBY_ADMIN_JWT_SECRET"));
    }

    #[test]
    fn public_site_refuses_the_example_secrets() {
        let local = [("INNBY_BASE_URL", "http://localhost:8080")];
        let example = ("INNBY_LINK_KEY", EXAMPLE_LINK_KEY);
        let err = parse(&[example]).unwrap_err();
        assert!(err.to_string().contains(".env.example"), "{err}");
        parse(&[local[0], example]).unwrap();

        let hash = crate::admin::auth::tests::cheap_hash(EXAMPLE_ADMIN_PASSWORD);
        assert!(admin_with_hash(&hash, &config()).is_err());
        admin_with_hash(&hash, &parse(&local).unwrap()).unwrap();

        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let err = admin_with(&hash, EXAMPLE_ADMIN_JWT_SECRET, &config()).unwrap_err();
        assert!(err.to_string().contains(".env.example"), "{err}");
        admin_with(&hash, EXAMPLE_ADMIN_JWT_SECRET, &parse(&local).unwrap()).unwrap();
    }

    #[test]
    fn example_secrets_are_the_ones_in_env_example() {
        let example = include_str!("../.env.example");
        let value = |key: &str| {
            example
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .unwrap_or_else(|| panic!("{key} in .env.example"))
        };
        assert_eq!(value("INNBY_LINK_KEY"), EXAMPLE_LINK_KEY);
        assert_eq!(value("INNBY_ADMIN_JWT_SECRET"), EXAMPLE_ADMIN_JWT_SECRET);
        assert_eq!(EXAMPLE_ADMIN_JWT_SECRET, jwt::tests::SECRET);
        let hash = value("INNBY_ADMIN_PASSWORD_HASH").trim_matches('\'');
        assert!(crate::admin::auth::verify(hash, EXAMPLE_ADMIN_PASSWORD));
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
    fn mail_config_parses_sender_and_credentials() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string())
            }
        };
        let mail = MailConfig::from_lookup(env(&[
            ("INNBY_SMTP_URL", "smtp://mail:1025"),
            ("INNBY_MAIL_FROM", "Julebord <fest@example.com>"),
        ]))
        .unwrap();
        assert_eq!(mail.from.email.to_string(), "fest@example.com");
        assert!(mail.smtp_user.is_none());

        assert!(
            MailConfig::from_lookup(env(&[
                ("INNBY_SMTP_URL", "smtp://mail:1025"),
                ("INNBY_MAIL_FROM", "not an address"),
            ]))
            .is_err()
        );
        assert!(
            MailConfig::from_lookup(env(&[
                ("INNBY_SMTP_URL", "smtp://mail:1025"),
                ("INNBY_MAIL_FROM", "fest@example.com"),
                ("INNBY_SMTP_USER", "user"),
            ]))
            .is_err()
        );
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
