//! The mails, in the guest's language. Plain text only: nothing to track, nothing to
//! render remotely.

use std::fmt::Display;

use chrono::{Duration, Utc};

use crate::{
    config::Config,
    db::{
        outbox::Kind,
        rsvp::{EmailStatus, Rsvp},
    },
    i18n::fill,
    link::Purpose,
    paths,
};

/// How long a confirmation link works; unconfirmed addresses are cleared after as long.
pub const CONFIRM_WITHIN: Duration = Duration::hours(48);

#[derive(Debug, Clone)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// The mail for a job, or why there's nothing to send any more: the guest may have
/// removed or changed their address since the job was queued.
pub fn compose(config: &Config, kind: Kind, rsvp: &Rsvp) -> Result<Mail, &'static str> {
    let to = rsvp.email.clone().ok_or("address removed")?;
    let needed = match kind {
        Kind::ConfirmEmail => EmailStatus::Unconfirmed,
        Kind::EmailInUse | Kind::RsvpLink | Kind::Reminder => EmailStatus::Confirmed,
    };
    if rsvp.email_status != needed {
        return Err("address no longer in the right state");
    }

    let lang = rsvp.lang;
    let t = lang.messages();
    // A page on the site, in the guest's language.
    let url = |path: &str| config.url(&paths::localized(path, lang));
    let rsvp_url = || {
        let link = config
            .link_key
            .link(Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
        url(&paths::path(paths::RSVP, link))
    };
    let (text, url) = match kind {
        Kind::ConfirmEmail => {
            let link = config
                .link_key
                .email_link(rsvp.id, &to, Utc::now() + CONFIRM_WITHIN);
            (
                &t.mail.confirm_email,
                url(&paths::path(paths::CONFIRM_EMAIL, link)),
            )
        }
        Kind::EmailInUse => (&t.mail.email_in_use, url(paths::LOST)),
        Kind::RsvpLink => (&t.mail.rsvp_link, rsvp_url()),
        Kind::Reminder => (&t.mail.reminder, rsvp_url()),
    };

    let event = &config.event;
    let mut values: Vec<(&str, &dyn Display)> = vec![
        ("title", event.title.get(lang)),
        ("url", &url),
        ("base", &config.base_url),
    ];
    // Anyone can type in any address, so the confirmation carries nothing a guest wrote,
    // not even their name: it can't be used to send someone else a message.
    if kind != Kind::ConfirmEmail {
        values.push(("name", &rsvp.name));
    }
    let (when, answer) = (
        lang.format_date(event.starts_at),
        t.answer(rsvp.response, rsvp.party_size),
    );
    if kind == Kind::Reminder {
        values.extend([
            ("when", &when as &dyn Display),
            ("place", &event.location),
            ("answer", &answer),
        ]);
    }
    Ok(Mail {
        to,
        subject: fill(&text.subject, &values),
        body: format!(
            "{}\n\n-- \n{}\n",
            fill(&text.body, &values),
            fill(&t.mail.footer, &values)
        ),
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::tests::config,
        db::rsvp::{Response, Source},
        i18n::Lang,
    };

    fn guest(lang: Lang, email_status: EmailStatus) -> Rsvp {
        Rsvp {
            id: Uuid::nil(),
            source: Source::Invite,
            name: "Kari Nordmann".into(),
            response: Response::Attending,
            party_size: 2,
            max_party_size: 2,
            link_nonce: 0,
            lang,
            email: Some("kari@example.com".into()),
            email_status,
            email_consent_at: Some(Utc::now()),
            privacy_version: None,
            responded_at: None,
            created_at: Utc::now(),
            group_id: None,
            name_display: None,
        }
    }

    #[test]
    fn every_mail_fills_in_its_placeholders() {
        let config = config();
        for lang in Lang::ALL {
            for kind in [
                Kind::ConfirmEmail,
                Kind::EmailInUse,
                Kind::RsvpLink,
                Kind::Reminder,
            ] {
                let status = match kind {
                    Kind::ConfirmEmail => EmailStatus::Unconfirmed,
                    _ => EmailStatus::Confirmed,
                };
                let mail = compose(&config, kind, &guest(lang, status)).unwrap();
                let text = format!("{}\n{}", mail.subject, mail.body);
                assert!(!text.contains(['{', '}']), "{lang:?} {kind:?}: {text}");
                assert!(text.contains("https://fest.example.com/"), "{text}");
                let greets = matches!(kind, Kind::RsvpLink | Kind::Reminder);
                assert_eq!(text.contains("Kari"), greets, "{lang:?} {kind:?}: {text}");
            }
        }
    }
}
