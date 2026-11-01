use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;

use crate::i18n::Localized;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub event: Event,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub title: Localized<String>,
    pub description: Localized<String>,
    pub location: String,
    pub starts_at: DateTime<Tz>,
}

/// The Postgres connection string. Kept apart from `Config` because `fest migrate`
/// needs nothing else.
pub fn database_url() -> Result<String> {
    std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("DATABASE_URL must be set"))
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Builds the config from any key lookup, so tests don't touch the process environment.
    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let optional = |key: &str| get(key).filter(|value| !value.trim().is_empty());
        let required = |key: &str| optional(key).ok_or_else(|| anyhow!("{key} must be set"));
        let localized = |key: &str| -> Result<Localized<String>> {
            let nb = required(&format!("{key}_NB"))?;
            let en = optional(&format!("{key}_EN")).unwrap_or_else(|| nb.clone());
            Ok(Localized { nb, en })
        };

        let listen = optional("FEST_LISTEN").unwrap_or_else(|| "0.0.0.0:8080".into());
        let listen = listen
            .parse()
            .with_context(|| format!("FEST_LISTEN: invalid socket address {listen:?}"))?;

        let tz_name = optional("FEST_EVENT_TIMEZONE").unwrap_or_else(|| "Europe/Oslo".into());
        let tz: Tz = tz_name
            .parse()
            .map_err(|_| anyhow!("FEST_EVENT_TIMEZONE: unknown time zone {tz_name:?}"))?;
        let start = required("FEST_EVENT_START")?;
        let starts_at = NaiveDateTime::parse_from_str(&start, "%Y-%m-%dT%H:%M")
            .with_context(|| format!("FEST_EVENT_START: expected YYYY-MM-DDTHH:MM, got {start:?}"))?
            .and_local_timezone(tz)
            .single()
            .ok_or_else(|| {
                anyhow!("FEST_EVENT_START: {start} is ambiguous or skipped in {tz_name}")
            })?;

        Ok(Self {
            listen,
            event: Event {
                title: localized("FEST_EVENT_TITLE")?,
                description: localized("FEST_EVENT_DESCRIPTION")?,
                location: required("FEST_EVENT_LOCATION")?,
                starts_at,
            },
        })
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;

    use super::*;

    const VALID: &[(&str, &str)] = &[
        ("FEST_EVENT_TITLE_NB", "Julebord"),
        ("FEST_EVENT_TITLE_EN", "Christmas party"),
        ("FEST_EVENT_DESCRIPTION_NB", "Velkommen!"),
        ("FEST_EVENT_LOCATION", "Storgata 1, Oslo"),
        ("FEST_EVENT_START", "2026-12-12T19:00"),
    ];

    fn parse(overrides: &[(&str, &str)]) -> Result<Config> {
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
}
