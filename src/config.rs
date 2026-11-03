use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;

use crate::{admin::jwt, i18n::Localized, link::LinkKey};

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
    pub event: Event,
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

#[derive(Debug, Clone)]
pub struct Event {
    pub title: Localized<String>,
    pub description: Localized<String>,
    pub location: String,
    pub starts_at: DateTime<Tz>,
    /// Guests can answer until then; unset means until the event.
    pub rsvp_deadline: Option<DateTime<Tz>>,
}

impl Event {
    /// Whether guests can still answer or change their answer.
    pub fn accepts_answers(&self, now: DateTime<chrono::Utc>) -> bool {
        now < self.rsvp_deadline.unwrap_or(self.starts_at)
    }
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

    /// `{key}_NB`, with optional `_NN`, `_EN` (British), `_EN_US`, `_ZH_HANS` and
    /// `_ZH_HANT`. Nynorsk and British English fall back to Bokmål, American English and
    /// Simplified Chinese to British, and Traditional Chinese to Simplified.
    fn localized(&self, key: &str) -> Result<Localized<String>> {
        let nb = self.required(&format!("{key}_NB"))?;
        let or = |suffix: &str, fallback: &String| {
            self.optional(&format!("{key}_{suffix}"))
                .unwrap_or_else(|| fallback.clone())
        };
        let en_gb = or("EN", &nb);
        let zh_hans = or("ZH_HANS", &en_gb);
        Ok(Localized {
            nn: or("NN", &nb),
            en_us: or("EN_US", &en_gb),
            zh_hant: or("ZH_HANT", &zh_hans),
            zh_hans,
            en_gb,
            nb,
        })
    }

    fn time_zone(&self) -> Result<Tz> {
        let name = self
            .optional("INNBY_EVENT_TIMEZONE")
            .unwrap_or_else(|| "Europe/Oslo".into());
        name.parse()
            .map_err(|_| anyhow!("INNBY_EVENT_TIMEZONE: unknown time zone {name:?}"))
    }

    /// A local time like `2026-12-12T19:00` in the event's time zone.
    fn local_time(&self, key: &str, value: &str, tz: Tz) -> Result<DateTime<Tz>> {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
            .with_context(|| format!("{key}: expected YYYY-MM-DDTHH:MM, got {value:?}"))?
            .and_local_timezone(tz)
            .single()
            .ok_or_else(|| anyhow!("{key}: {value} is ambiguous or skipped in {tz}"))
    }

    fn event_start(&self) -> Result<DateTime<Tz>> {
        let start = self.required("INNBY_EVENT_START")?;
        self.local_time("INNBY_EVENT_START", &start, self.time_zone()?)
    }

    fn socket_address(&self, key: &str, default: &str) -> Result<SocketAddr> {
        let value = self.optional(key).unwrap_or_else(|| default.into());
        value
            .parse()
            .with_context(|| format!("{key}: invalid socket address {value:?}"))
    }
}

/// When the event starts. `innby janitor` needs only this, besides the database.
pub fn event_start() -> Result<DateTime<Tz>> {
    Env(process_env).event_start()
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

        let starts_at = env.event_start()?;
        let rsvp_deadline = env
            .optional("INNBY_RSVP_DEADLINE")
            .map(|value| env.local_time("INNBY_RSVP_DEADLINE", &value, starts_at.timezone()))
            .transpose()?;

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
            event: Event {
                title: env.localized("INNBY_EVENT_TITLE")?,
                description: env.localized("INNBY_EVENT_DESCRIPTION")?,
                location: env.required("INNBY_EVENT_LOCATION")?,
                starts_at,
                rsvp_deadline,
            },
        })
    }
}

/// Settings only `innby admin` needs, on top of `Config`.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    pub listen: SocketAddr,
    pub user: String,
    /// An argon2 PHC string from `innby hash-password`.
    pub password_hash: String,
    /// Signs the login's JWTs.
    pub jwt_secret: jwt::Secret,
}

impl AdminConfig {
    pub fn from_env(config: &Config) -> Result<Self> {
        Self::from_lookup(process_env, config)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>, config: &Config) -> Result<Self> {
        let env = Env(get);
        let password_hash = env.required("INNBY_ADMIN_PASSWORD_HASH")?;
        crate::admin::auth::check_hash(&password_hash).context("INNBY_ADMIN_PASSWORD_HASH")?;
        ensure!(
            !config.is_https()
                || !crate::admin::auth::verify(&password_hash, EXAMPLE_ADMIN_PASSWORD),
            "INNBY_ADMIN_PASSWORD_HASH: that's a hash of the example password from \
             .env.example; make one for a real password with `just innby hash-password`"
        );
        let jwt_secret = env.required("INNBY_ADMIN_JWT_SECRET")?;
        ensure!(
            !config.is_https() || jwt_secret.trim() != EXAMPLE_ADMIN_JWT_SECRET,
            "INNBY_ADMIN_JWT_SECRET: that's the example from .env.example, which anyone can read"
        );
        Ok(Self {
            listen: env.socket_address("INNBY_ADMIN_LISTEN", "0.0.0.0:8081")?,
            user: env
                .optional("INNBY_ADMIN_USER")
                .unwrap_or_else(|| "admin".into()),
            password_hash,
            jwt_secret: jwt::Secret::from_base64(&jwt_secret).context("INNBY_ADMIN_JWT_SECRET")?,
        })
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
        ("INNBY_EVENT_TITLE_NB", "Julebord"),
        ("INNBY_EVENT_TITLE_EN", "Christmas party"),
        ("INNBY_EVENT_DESCRIPTION_NB", "Velkommen!"),
        ("INNBY_EVENT_LOCATION", "Storgata 1, Oslo"),
        ("INNBY_EVENT_START", "2026-12-12T19:00"),
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
        assert_eq!(config.event.title.en_gb, "Christmas party");
        assert_eq!(
            config.event.starts_at.to_rfc3339(),
            "2026-12-12T19:00:00+01:00"
        );
    }

    #[test]
    fn languages_fall_back_on_each_other() {
        let event = parse(&[]).unwrap().event;
        assert_eq!(event.description.en_gb, "Velkommen!");
        assert_eq!(event.title.nn, "Julebord");
        assert_eq!(event.title.en_us, "Christmas party");
        assert_eq!(event.title.zh_hant, "Christmas party");

        let event = parse(&[
            ("INNBY_EVENT_TITLE_NN", "Juleball"),
            ("INNBY_EVENT_TITLE_EN_US", "Holiday party"),
        ])
        .unwrap()
        .event;
        assert_eq!(
            (event.title.nn.as_str(), event.title.en_us.as_str()),
            ("Juleball", "Holiday party")
        );
    }

    #[test]
    fn blank_required_value_is_an_error() {
        let err = parse(&[("INNBY_EVENT_LOCATION", "  ")]).unwrap_err();
        assert_eq!(err.to_string(), "INNBY_EVENT_LOCATION must be set");
    }

    #[test]
    fn rejects_bad_start_and_time_zone() {
        assert!(parse(&[("INNBY_EVENT_START", "12.12.2026 19:00")]).is_err());
        assert!(parse(&[("INNBY_EVENT_TIMEZONE", "Europe/Atlantis")]).is_err());
        // 02:30 doesn't exist in Oslo on the night clocks spring forward.
        assert!(parse(&[("INNBY_EVENT_START", "2026-03-29T02:30")]).is_err());
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

    fn admin_with_hash(hash: &str, config: &Config) -> Result<AdminConfig> {
        admin_with(hash, crate::link::tests::KEY, config)
    }

    #[test]
    fn admin_config_needs_a_valid_hash() {
        let config = config();
        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let admin = admin_with_hash(&hash, &config).unwrap();
        assert_eq!(admin.user, "admin");
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

    #[test]
    fn answers_close_at_deadline_or_event_start() {
        let at = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value)
                .unwrap()
                .to_utc()
        };
        let event = parse(&[]).unwrap().event;
        assert!(event.accepts_answers(at("2026-12-12T17:59:00Z")));
        assert!(!event.accepts_answers(at("2026-12-12T18:00:00Z")));

        let event = parse(&[("INNBY_RSVP_DEADLINE", "2026-12-01T12:00")])
            .unwrap()
            .event;
        assert!(event.accepts_answers(at("2026-12-01T10:59:00Z")));
        assert!(!event.accepts_answers(at("2026-12-01T11:00:00Z")));
    }
}
