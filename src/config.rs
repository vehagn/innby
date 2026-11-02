use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;

use crate::{i18n::Localized, link::LinkKey};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    /// The public site's origin, e.g. `https://fest.example.com`, for links the admin UI
    /// and mails hand out.
    pub base_url: String,
    pub link_key: LinkKey,
    /// The secret part of the shared `/join/{code}` link. The open RSVP is off without it.
    pub open_rsvp_code: Option<String>,
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

/// The Postgres connection string. Kept apart from `Config` because `fest migrate`
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

    fn localized(&self, key: &str) -> Result<Localized<String>> {
        let nb = self.required(&format!("{key}_NB"))?;
        let en = self
            .optional(&format!("{key}_EN"))
            .unwrap_or_else(|| nb.clone());
        Ok(Localized { nb, en })
    }

    fn time_zone(&self) -> Result<Tz> {
        let name = self
            .optional("FEST_EVENT_TIMEZONE")
            .unwrap_or_else(|| "Europe/Oslo".into());
        name.parse()
            .map_err(|_| anyhow!("FEST_EVENT_TIMEZONE: unknown time zone {name:?}"))
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
        let start = self.required("FEST_EVENT_START")?;
        self.local_time("FEST_EVENT_START", &start, self.time_zone()?)
    }

    fn socket_address(&self, key: &str, default: &str) -> Result<SocketAddr> {
        let value = self.optional(key).unwrap_or_else(|| default.into());
        value
            .parse()
            .with_context(|| format!("{key}: invalid socket address {value:?}"))
    }
}

/// When the event starts. `fest janitor` needs only this, besides the database.
pub fn event_start() -> Result<DateTime<Tz>> {
    Env(process_env).event_start()
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(process_env)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let env = Env(get);
        let listen = env.socket_address("FEST_LISTEN", "0.0.0.0:8080")?;

        let base_url = env.required("FEST_BASE_URL")?;
        let base_url = base_url.trim_end_matches('/').to_owned();
        let host = base_url
            .strip_prefix("https://")
            .or_else(|| base_url.strip_prefix("http://"));
        ensure!(
            host.is_some_and(|host| !host.is_empty() && !host.contains('/')),
            "FEST_BASE_URL: expected an origin like https://fest.example.com, got {base_url:?}"
        );

        let link_key =
            LinkKey::from_base64(&env.required("FEST_LINK_KEY")?).context("FEST_LINK_KEY")?;

        let open_rsvp_code = env.optional("FEST_OPEN_RSVP_CODE");
        if let Some(code) = &open_rsvp_code {
            ensure!(
                code.len() >= 8
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
                "FEST_OPEN_RSVP_CODE: use at least 8 letters, digits, '-' or '_'"
            );
        }
        let trust_forwarded = match env.optional("FEST_TRUST_FORWARDED").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                anyhow::bail!("FEST_TRUST_FORWARDED: expected true or false, got {other:?}")
            }
        };

        let starts_at = env.event_start()?;
        let rsvp_deadline = env
            .optional("FEST_RSVP_DEADLINE")
            .map(|value| env.local_time("FEST_RSVP_DEADLINE", &value, starts_at.timezone()))
            .transpose()?;

        let controller_email = env.required("FEST_CONTROLLER_EMAIL")?;
        ensure!(
            crate::db::rsvp::normalize_email(&controller_email).is_some(),
            "FEST_CONTROLLER_EMAIL: not an e-mail address: {controller_email:?}"
        );

        Ok(Self {
            listen,
            base_url,
            link_key,
            open_rsvp_code,
            trust_forwarded,
            controller: Controller {
                name: env.required("FEST_CONTROLLER_NAME")?,
                email: controller_email,
            },
            event: Event {
                title: env.localized("FEST_EVENT_TITLE")?,
                description: env.localized("FEST_EVENT_DESCRIPTION")?,
                location: env.required("FEST_EVENT_LOCATION")?,
                starts_at,
                rsvp_deadline,
            },
        })
    }
}

/// Settings only `fest admin` needs, on top of `Config`.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    pub listen: SocketAddr,
    pub user: String,
    /// An argon2 PHC string from `fest hash-password`.
    pub password_hash: String,
}

impl AdminConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(process_env)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let env = Env(get);
        let password_hash = env.required("FEST_ADMIN_PASSWORD_HASH")?;
        crate::admin::auth::check_hash(&password_hash).context("FEST_ADMIN_PASSWORD_HASH")?;
        Ok(Self {
            listen: env.socket_address("FEST_ADMIN_LISTEN", "0.0.0.0:8081")?,
            user: env
                .optional("FEST_ADMIN_USER")
                .unwrap_or_else(|| "admin".into()),
            password_hash,
        })
    }
}

/// Settings only `fest mailer` needs, on top of `Config`.
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
        let from = env.required("FEST_MAIL_FROM")?;
        let smtp_user = env.optional("FEST_SMTP_USER");
        let smtp_password = env.optional("FEST_SMTP_PASSWORD");
        ensure!(
            smtp_user.is_some() == smtp_password.is_some(),
            "FEST_SMTP_USER and FEST_SMTP_PASSWORD go together"
        );
        Ok(Self {
            smtp_url: env.required("FEST_SMTP_URL")?,
            smtp_user,
            smtp_password,
            from: from
                .parse()
                .with_context(|| format!("FEST_MAIL_FROM: not a mailbox: {from:?}"))?,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;

    use super::*;

    const VALID: &[(&str, &str)] = &[
        ("FEST_BASE_URL", "https://fest.example.com"),
        ("FEST_LINK_KEY", crate::link::tests::KEY),
        ("FEST_EVENT_TITLE_NB", "Julebord"),
        ("FEST_EVENT_TITLE_EN", "Christmas party"),
        ("FEST_EVENT_DESCRIPTION_NB", "Velkommen!"),
        ("FEST_EVENT_LOCATION", "Storgata 1, Oslo"),
        ("FEST_EVENT_START", "2026-12-12T19:00"),
        ("FEST_CONTROLLER_NAME", "Kari Nordmann"),
        ("FEST_CONTROLLER_EMAIL", "kari@example.com"),
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
        assert_eq!(config.event.title.en, "Christmas party");
        assert_eq!(
            config.event.starts_at.to_rfc3339(),
            "2026-12-12T19:00:00+01:00"
        );
    }

    #[test]
    fn english_falls_back_to_norwegian() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.event.description.en, "Velkommen!");
    }

    #[test]
    fn blank_required_value_is_an_error() {
        let err = parse(&[("FEST_EVENT_LOCATION", "  ")]).unwrap_err();
        assert_eq!(err.to_string(), "FEST_EVENT_LOCATION must be set");
    }

    #[test]
    fn rejects_bad_start_and_time_zone() {
        assert!(parse(&[("FEST_EVENT_START", "12.12.2026 19:00")]).is_err());
        assert!(parse(&[("FEST_EVENT_TIMEZONE", "Europe/Atlantis")]).is_err());
        // 02:30 doesn't exist in Oslo on the night clocks spring forward.
        assert!(parse(&[("FEST_EVENT_START", "2026-03-29T02:30")]).is_err());
    }

    #[test]
    fn admin_config_needs_a_valid_hash() {
        let hash = crate::admin::auth::tests::cheap_hash("secret");
        let admin = AdminConfig::from_lookup(|key| {
            (key == "FEST_ADMIN_PASSWORD_HASH").then(|| hash.clone())
        })
        .unwrap();
        assert_eq!(admin.user, "admin");
        assert_eq!(admin.listen, "0.0.0.0:8081".parse().unwrap());

        assert!(AdminConfig::from_lookup(|_| None).is_err());
        let err = AdminConfig::from_lookup(|_| Some("hunter2".into())).unwrap_err();
        assert!(format!("{err:#}").starts_with("FEST_ADMIN_PASSWORD_HASH"));
    }

    #[test]
    fn open_rsvp_is_off_unless_a_code_is_set() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.open_rsvp_code, None);
        assert!(!config.trust_forwarded);

        let config = parse(&[("FEST_OPEN_RSVP_CODE", "jul-2026_ab")]).unwrap();
        assert_eq!(config.open_rsvp_code.as_deref(), Some("jul-2026_ab"));
        for bad in ["short", "has space x", "æøå-code-1"] {
            assert!(parse(&[("FEST_OPEN_RSVP_CODE", bad)]).is_err(), "{bad}");
        }
        assert!(parse(&[("FEST_TRUST_FORWARDED", "yes")]).is_err());
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
            ("FEST_SMTP_URL", "smtp://mail:1025"),
            ("FEST_MAIL_FROM", "Julebord <fest@example.com>"),
        ]))
        .unwrap();
        assert_eq!(mail.from.email.to_string(), "fest@example.com");
        assert!(mail.smtp_user.is_none());

        assert!(
            MailConfig::from_lookup(env(&[
                ("FEST_SMTP_URL", "smtp://mail:1025"),
                ("FEST_MAIL_FROM", "not an address"),
            ]))
            .is_err()
        );
        assert!(
            MailConfig::from_lookup(env(&[
                ("FEST_SMTP_URL", "smtp://mail:1025"),
                ("FEST_MAIL_FROM", "fest@example.com"),
                ("FEST_SMTP_USER", "user"),
            ]))
            .is_err()
        );
    }

    #[test]
    fn base_url_must_be_an_origin() {
        let config = parse(&[("FEST_BASE_URL", "http://localhost:8080/")]).unwrap();
        assert_eq!(config.base_url, "http://localhost:8080");
        for bad in [
            "fest.example.com",
            "https://",
            "https://fest.example.com/rsvp",
        ] {
            assert!(parse(&[("FEST_BASE_URL", bad)]).is_err(), "{bad}");
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

        let event = parse(&[("FEST_RSVP_DEADLINE", "2026-12-01T12:00")])
            .unwrap()
            .event;
        assert!(event.accepts_answers(at("2026-12-01T10:59:00Z")));
        assert!(!event.accepts_answers(at("2026-12-01T11:00:00Z")));
    }
}
