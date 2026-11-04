//! Settings only `innby mailer` needs, on top of `Config`: the SMTP server, its login,
//! and the sender. The password never shows in `Debug`.

use anyhow::{Context, Result, ensure};

use super::{Env, process_env};

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
mod tests {
    use super::*;

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
}
