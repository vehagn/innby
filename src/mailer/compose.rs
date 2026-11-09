//! The mails, in the guest's language. Plain text only: nothing to track, nothing to
//! render remotely. A reminder or a change notice to a guest who's coming carries the
//! event as a calendar file, as their page offers it.

use std::fmt::Display;

use chrono::{Duration, Utc};

use crate::{
    calendar,
    config::Config,
    db::{
        event::Event,
        outbox::Kind,
        rsvp::{EmailStatus, Response, Rsvp},
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
    /// The event as a calendar file, attached.
    pub calendar: Option<String>,
}

/// The mail for a job, or why there's nothing to send any more: the guest may have
/// removed or changed their address since the job was queued.
pub fn compose(
    config: &Config,
    kind: Kind,
    rsvp: &Rsvp,
    event: &Event,
) -> Result<Mail, &'static str> {
    let (id, event) = (event, &event.details);
    let to = rsvp.email.clone().ok_or("address removed")?;
    let needed = match kind {
        Kind::ConfirmEmail => EmailStatus::Unconfirmed,
        Kind::EmailInUse | Kind::RsvpLink | Kind::Reminder | Kind::EventChanged => {
            EmailStatus::Confirmed
        }
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
        Kind::EventChanged => (&t.mail.event_changed, rsvp_url()),
    };

    let title = event.title(lang);
    let mut values: Vec<(&str, &dyn Display)> =
        vec![("title", &title), ("url", &url), ("base", &config.base_url)];
    // Anyone can type in any address, so the confirmation carries nothing a guest wrote,
    // not even their name: it can't be used to send someone else a message.
    if kind != Kind::ConfirmEmail {
        values.push(("name", &rsvp.name));
    }
    let (when, answer) = (
        lang.format_when(event.starts_at, event.ends_at),
        t.answer(rsvp.response, rsvp.party_size),
    );
    // What the event is now, read as the mail goes out.
    let about_event = matches!(kind, Kind::Reminder | Kind::EventChanged);
    if about_event {
        values.extend([
            ("when", &when as &dyn Display),
            ("place", &event.location),
            ("answer", &answer),
        ]);
    }
    // For their calendar, as their page offers it to those coming.
    let calendar = (about_event && rsvp.response == Response::Attending)
        .then(|| calendar::file(id, lang, &config.base_url));
    let mut body = fill(&text.body, &values);
    if calendar.is_some() {
        body = format!("{body}\n\n{}", t.mail.calendar_attached);
    }
    Ok(Mail {
        to,
        subject: fill(&text.subject, &values),
        body: format!("{body}\n\n-- \n{}\n", fill(&t.mail.footer, &values)),
        calendar,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::{config::tests::config, i18n::Lang};

    fn guest(lang: Lang, email_status: EmailStatus) -> Rsvp {
        Rsvp {
            id: Uuid::nil(),
            event_id: Uuid::nil(),
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
            choice: None,
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
                Kind::EventChanged,
            ] {
                let status = match kind {
                    Kind::ConfirmEmail => EmailStatus::Unconfirmed,
                    _ => EmailStatus::Confirmed,
                };
                let event = Event {
                    id: Uuid::nil(),
                    details: crate::db::event::tests::details(),
                    changed_at: None,
                };
                let mail = compose(&config, kind, &guest(lang, status), &event).unwrap();
                let text = format!("{}\n{}", mail.subject, mail.body);
                assert!(!text.contains(['{', '}']), "{lang:?} {kind:?}: {text}");
                assert!(text.contains("https://fest.example.com/"), "{text}");
                let greets = matches!(kind, Kind::RsvpLink | Kind::Reminder | Kind::EventChanged);
                assert_eq!(text.contains("Kari"), greets, "{lang:?} {kind:?}: {text}");
                // Only a reminder or a change notice carries the calendar file, and says so.
                let about_event = matches!(kind, Kind::Reminder | Kind::EventChanged);
                assert_eq!(mail.calendar.is_some(), about_event, "{lang:?} {kind:?}");
                let said = text.contains(&lang.messages().mail.calendar_attached);
                assert_eq!(said, about_event, "{lang:?} {kind:?}");
            }
        }
    }

    #[test]
    fn a_reminder_carries_the_calendar_file_only_for_those_coming() {
        let event = Event {
            id: Uuid::nil(),
            details: crate::db::event::tests::details(),
            changed_at: None,
        };
        let mut kari = guest(Lang::EnGb, EmailStatus::Confirmed);
        let mail = compose(&config(), Kind::Reminder, &kari, &event).unwrap();
        let file = mail.calendar.unwrap();
        assert!(file.contains("\r\nSUMMARY:Christmas party\r\n"), "{file}");
        assert!(!file.contains("Kari"), "{file}");
        kari.response = Response::Pending;
        let mail = compose(&config(), Kind::Reminder, &kari, &event).unwrap();
        assert!(mail.calendar.is_none());
    }
}
