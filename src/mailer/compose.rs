//! The mails, in the guest's language, or for a host's password, the language of the page
//! that asked for it. Plain text only: nothing to track, nothing to render remotely. A
//! reminder or a change notice to a guest who's coming carries the event as a calendar
//! file, as their page offers it.

use std::fmt::Display;

use chrono::{DateTime, Duration, Utc};

use crate::{
    calendar,
    config::Config,
    db::{
        admin_user::AdminUser,
        event::Event,
        outbox::Kind,
        rsvp::{EmailStatus, Response, Rsvp},
    },
    i18n::{Lang, fill},
    paths,
};

/// How long a confirmation link works; unconfirmed addresses are cleared after as long.
pub const CONFIRM_WITHIN: Duration = Duration::hours(48);

/// How long a link to set a password works, from when it was asked for: an hour for
/// someone who forgot theirs, a week for a new user, who may not read their mail at once.
pub const RESET_WITHIN: Duration = Duration::hours(1);
pub const FIRST_WITHIN: Duration = Duration::days(7);

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
        Kind::PasswordReset | Kind::PasswordSet => return Err("a host's mail, not a guest's"),
    };
    if rsvp.email_status != needed {
        return Err("address no longer in the right state");
    }

    let lang = rsvp.lang;
    let t = lang.messages();
    // A page on the site, in the guest's language.
    let url = |path: &str| config.url(&paths::localized(path, lang));
    let rsvp_url = || url(&paths::path(paths::RSVP, rsvp.link(&config.link_key)));
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
        Kind::PasswordReset | Kind::PasswordSet => unreachable!("refused above"),
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

/// The mail to a host made in the admin UI with a link to set their password, which
/// `admin_url` leads to, or why there's nothing to send: they may have removed their
/// address meanwhile.
pub fn compose_for_host(
    config: &Config,
    admin_url: Option<&str>,
    kind: Kind,
    user: &AdminUser,
    lang: Lang,
    asked_at: DateTime<Utc>,
) -> Result<Mail, &'static str> {
    let to = user.email.clone().ok_or("address removed")?;
    let admin_url = admin_url.ok_or("INNBY_ADMIN_URL isn't set")?;
    let (text, within) = match kind {
        Kind::PasswordReset => (&lang.messages().mail.password_reset, RESET_WITHIN),
        Kind::PasswordSet => (&lang.messages().mail.password_set, FIRST_WITHIN),
        _ => return Err("a guest's mail, not a host's"),
    };
    let token = config
        .link_key
        .password_link(&user.name, &to, user.version, asked_at + within);
    let path = paths::path(paths::PASSWORD, &user.name).replace("{token}", &token);
    let url = format!("{admin_url}{}", paths::localized(&path, lang));
    let hours = within.num_hours();
    let values: [(&str, &dyn Display); 4] = [
        ("name", &user.name),
        ("url", &url),
        ("hours", &hours),
        ("days", &within.num_days()),
    ];
    Ok(Mail {
        to,
        subject: fill(&text.subject, &values),
        body: format!("{}\n", fill(&text.body, &values)),
        calendar: None,
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
        for &lang in Lang::all() {
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
        let mut kari = guest(Lang::EN, EmailStatus::Confirmed);
        let mail = compose(&config(), Kind::Reminder, &kari, &event).unwrap();
        let file = mail.calendar.unwrap();
        assert!(file.contains("\r\nSUMMARY:Christmas party\r\n"), "{file}");
        assert!(!file.contains("Kari"), "{file}");
        kari.response = Response::Pending;
        let mail = compose(&config(), Kind::Reminder, &kari, &event).unwrap();
        assert!(mail.calendar.is_none());
    }

    #[test]
    fn a_hosts_mail_has_a_link_that_works_for_a_while() {
        let config = config();
        let kari = AdminUser {
            name: "kari".into(),
            role: None,
            password_hash: None,
            version: 2,
            created_at: Utc::now(),
            email: Some("kari@example.com".into()),
        };
        let asked = Utc::now();
        for &lang in Lang::all() {
            for kind in [Kind::PasswordReset, Kind::PasswordSet] {
                let mail = compose_for_host(
                    &config,
                    Some("https://fest.example.com/admin"),
                    kind,
                    &kari,
                    lang,
                    asked,
                )
                .unwrap();
                let text = format!("{}\n{}", mail.subject, mail.body);
                assert!(!text.contains(['{', '}']), "{lang:?} {kind:?}: {text}");
                assert!(
                    text.contains("https://fest.example.com/admin/password/kari/"),
                    "{text}"
                );
            }
        }
        // The link is the one the admin UI checks.
        let mail = compose_for_host(
            &config,
            Some("https://a.example"),
            Kind::PasswordReset,
            &kari,
            Lang::EN,
            asked,
        )
        .unwrap();
        let token = mail.body.split("/password/kari/").nth(1).unwrap();
        let token = token.split(['?', '\n']).next().unwrap();
        let link: crate::link::PasswordLink = token.parse().unwrap();
        assert!(
            config
                .link_key
                .verify_password(&link, "kari", "kari@example.com", 2)
        );
        assert_eq!(link.expires.timestamp(), (asked + RESET_WITHIN).timestamp());
        // No address, no mail; no admin URL, no link.
        let gone = AdminUser {
            email: None,
            ..kari.clone()
        };
        assert!(
            compose_for_host(
                &config,
                Some("https://a.example"),
                Kind::PasswordSet,
                &gone,
                Lang::EN,
                asked
            )
            .is_err()
        );
        assert!(
            compose_for_host(&config, None, Kind::PasswordSet, &kari, Lang::EN, asked).is_err()
        );
    }
}
