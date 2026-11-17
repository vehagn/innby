use std::sync::Mutex;

use super::*;
use crate::{
    config::tests::config,
    db::{
        outbox::Kind,
        rsvp::{Rsvp, create_invite},
    },
    i18n::Lang,
};

#[test]
fn rejects_unusable_recipients() {
    let mail = |to: &str| Mail {
        to: to.into(),
        subject: "Hei".into(),
        body: "Hei".into(),
        calendar: None,
    };
    assert!(message(&from(), mail("kari@example.com")).is_ok());
    assert!(message(&from(), mail("not an address")).is_err());
}

#[test]
fn a_subject_cant_add_headers() {
    // An editor writes the event's title, which goes into the subject; a line break in
    // it mustn't start a header of its own, such as a Bcc to the editor.
    let mail = Mail {
        to: "kari@example.com".into(),
        subject: "Julebord\r\nBcc: editor@example.com\nX-Evil: 1".into(),
        body: "Hei".into(),
        calendar: None,
    };
    let sent = String::from_utf8(message(&from(), mail).unwrap().formatted()).unwrap();
    let headers = sent.split("\r\n\r\n").next().unwrap();
    for line in headers.split("\r\n") {
        assert!(
            !line.starts_with("Bcc:") && !line.starts_with("X-Evil:"),
            "{headers}"
        );
    }
}

#[test]
fn a_calendar_file_goes_beside_the_text() {
    let mail = Mail {
        to: "kari@example.com".into(),
        subject: "Julebord".into(),
        body: "Hei Kari".into(),
        calendar: Some("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into()),
    };
    let sent = String::from_utf8(message(&from(), mail).unwrap().formatted()).unwrap();
    assert!(sent.contains("Content-Type: multipart/mixed"), "{sent}");
    assert!(
        sent.contains("Content-Type: text/plain; charset=utf-8"),
        "{sent}"
    );
    assert!(
        sent.contains("Content-Type: text/calendar; charset=utf-8"),
        "{sent}"
    );
    assert!(
        sent.contains(r#"Content-Disposition: attachment; filename="event.ics""#),
        "{sent}"
    );
}

/// Collects mails instead of sending them, or fails every send.
#[derive(Default)]
struct Outbox {
    sent: Mutex<Vec<Mail>>,
    failure: Option<String>,
}

impl Sender for Outbox {
    async fn send(&self, from: &Mailbox, mail: Mail) -> Result<()> {
        anyhow::ensure!(
            self.failure.is_none(),
            "{}",
            self.failure.as_deref().unwrap_or("connection refused")
        );
        // What SMTP would send must build, too.
        message(from, mail.clone())?;
        self.sent.lock().unwrap().push(mail);
        Ok(())
    }
}

impl Outbox {
    /// Recipient, subject and body of each mail sent.
    fn mails(&self) -> Vec<(String, String, String)> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|mail| (mail.to.clone(), mail.subject.clone(), mail.body.clone()))
            .collect()
    }
}

fn from() -> Mailbox {
    "Julebord <fest@example.com>".parse().unwrap()
}

async fn guest_with_email(db: &PgPool, email: &str, lang: Lang) -> Rsvp {
    let guest = create_invite(db, crate::db::event::tests::event(db).await.id, "Kari", 1)
        .await
        .unwrap();
    rsvp::set_email(db, guest.id, email, lang, crate::web::PRIVACY_VERSION)
        .await
        .unwrap();
    rsvp::find(db, guest.id).await.unwrap().unwrap()
}

async fn unsent(db: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"select count(*) as "n!" from outbox where sent_at is null"#)
        .fetch_one(db)
        .await
        .unwrap()
}

#[sqlx::test]
async fn sends_confirmation_with_a_working_link(db: PgPool) {
    let config = config();
    let guest = guest_with_email(&db, "kari@example.com", Lang::EN).await;
    outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
        .await
        .unwrap();

    let sender = Outbox::default();
    assert_eq!(
        process_batch(&config, &from(), None, &db, &sender)
            .await
            .unwrap(),
        1
    );
    assert_eq!(unsent(&db).await, 0);

    let [(to, subject, body)] = sender.mails().try_into().unwrap();
    assert_eq!(to, "kari@example.com");
    let expected = crate::i18n::fill(
        &Lang::EN.messages().mail.confirm_email.subject,
        &[("title", &"Christmas party")],
    );
    assert_eq!(subject, expected);
    assert!(!body.contains("Kari"), "the guest's name: {body}");
    let url = body
        .split_whitespace()
        .find(|word| word.starts_with("https://fest.example.com/e/"))
        .unwrap();
    let token = url
        .trim_start_matches("https://fest.example.com/e/")
        .split('?')
        .next()
        .unwrap();
    let link: crate::link::EmailLink = token.parse().unwrap();
    assert!(config.link_key.verify_email(&link, "kari@example.com"));
    assert!(link.expires > chrono::Utc::now() + chrono::Duration::hours(47));
}

#[sqlx::test]
async fn drops_mail_for_a_removed_or_unconfirmed_address(db: PgPool) {
    let config = config();
    let guest = guest_with_email(&db, "kari@example.com", Lang::NB).await;
    // The link only goes to confirmed addresses.
    outbox::enqueue(&db, guest.id, Kind::RsvpLink)
        .await
        .unwrap();
    outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
        .await
        .unwrap();
    rsvp::remove_email(&db, guest.id).await.unwrap();

    let sender = Outbox::default();
    assert_eq!(
        process_batch(&config, &from(), None, &db, &sender)
            .await
            .unwrap(),
        2
    );
    assert!(sender.mails().is_empty());
    assert_eq!(unsent(&db).await, 0);
}

#[sqlx::test]
async fn sends_link_and_reminder_in_the_guests_language(db: PgPool) {
    let config = config();
    let guest = guest_with_email(&db, "kari@example.com", Lang::NB).await;
    rsvp::confirm_email(&db, guest.id, "kari@example.com")
        .await
        .unwrap();
    outbox::enqueue(&db, guest.id, Kind::RsvpLink)
        .await
        .unwrap();
    outbox::enqueue(&db, guest.id, Kind::Reminder)
        .await
        .unwrap();

    let sender = Outbox::default();
    process_batch(&config, &from(), None, &db, &sender)
        .await
        .unwrap();
    let mails = sender.mails();
    assert_eq!(mails.len(), 2);
    let link = config
        .link_key
        .link(crate::link::Purpose::Rsvp, guest.id, guest.link_nonce);
    assert_eq!(mails[0].1, "Lenken din – Julebord");
    assert_eq!(mails[1].1, "Påminnelse: Julebord");
    assert!(
        mails[0]
            .2
            .contains(&format!("https://fest.example.com/r/{link}?lang=nb"))
    );
    assert!(mails[1].2.contains("Ikke svart ennå"));
    assert!(mails[1].2.contains("Storgata 1, Oslo"));
}

#[sqlx::test]
async fn failed_sends_are_retried_later(db: PgPool) {
    let config = config();
    let guest = guest_with_email(&db, "kari@example.com", Lang::NB).await;
    outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
        .await
        .unwrap();

    let failing = Outbox {
        failure: Some("connection refused".into()),
        ..Outbox::default()
    };
    assert_eq!(
        process_batch(&config, &from(), None, &db, &failing)
            .await
            .unwrap(),
        1
    );
    let (attempts, error) = sqlx::query!("select attempts, last_error from outbox")
        .fetch_one(&db)
        .await
        .map(|row| (row.attempts, row.last_error))
        .unwrap();
    assert_eq!(
        (attempts, error.as_deref()),
        (1, Some("connection refused"))
    );
    // Not due again yet.
    assert_eq!(
        process_batch(&config, &from(), None, &db, &Outbox::default())
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test]
async fn failed_guest_and_host_mail_keep_identities_and_provider_details_out_of_logs(db: PgPool) {
    const GUEST: &str = "guest-private@example.com";
    const HOST_NAME: &str = "host-private@example.com";
    const HOST_ADDRESS: &str = "host-address-private@example.com";
    const PROVIDER_ERROR: &str = "provider rejected guest-private@example.com host-address-private@example.com bearer-secret signed-link-secret";

    let config = config();
    let guest = guest_with_email(&db, GUEST, Lang::EN).await;
    outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
        .await
        .unwrap();
    admin_user::create(
        &db,
        HOST_NAME,
        Some("editor"),
        Some("$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaA"),
        Some(HOST_ADDRESS),
    )
    .await
    .unwrap()
    .unwrap();
    outbox::enqueue_for_host(&db, HOST_NAME, Kind::PasswordSet, Lang::EN)
        .await
        .unwrap();

    let logs = crate::testing::logs::Capture::new();
    logs.run(async {
        let failing = Outbox {
            failure: Some(PROVIDER_ERROR.into()),
            ..Outbox::default()
        };
        assert_eq!(
            process_batch(
                &config,
                &from(),
                Some("https://admin.example.com"),
                &db,
                &failing,
            )
            .await
            .unwrap(),
            2
        );
        let failed_logs = logs.text();
        assert_eq!(
            failed_logs.matches("mail failed,").count(),
            2,
            "{failed_logs}"
        );
        assert_eq!(failed_logs.matches("attempt=1").count(), 2, "{failed_logs}");
        assert_eq!(unsent(&db).await, 2);
        let errors =
            sqlx::query_scalar!("select last_error from outbox where last_error is not null")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.as_deref() == Some(PROVIDER_ERROR))
        );

        // Both branches wait before retrying, then use the same success bookkeeping.
        let sender = Outbox::default();
        assert_eq!(
            process_batch(
                &config,
                &from(),
                Some("https://admin.example.com"),
                &db,
                &sender
            )
            .await
            .unwrap(),
            0
        );
        sqlx::query!("update outbox set next_try_at = now() - interval '1 second'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            process_batch(
                &config,
                &from(),
                Some("https://admin.example.com"),
                &db,
                &sender
            )
            .await
            .unwrap(),
            2
        );
        assert_eq!(sender.mails().len(), 2);
        assert_eq!(unsent(&db).await, 0);
        assert_eq!(logs.text().matches("mail sent").count(), 2);

        // Composition failures also finish both kinds of job without sending anything.
        outbox::enqueue(&db, guest.id, Kind::RsvpLink)
            .await
            .unwrap();
        outbox::enqueue_for_host(&db, HOST_NAME, Kind::PasswordSet, Lang::EN)
            .await
            .unwrap();
        assert_eq!(
            process_batch(&config, &from(), None, &db, &sender)
                .await
                .unwrap(),
            2
        );
        assert_eq!(sender.mails().len(), 2);
        assert_eq!(unsent(&db).await, 0);
        assert_eq!(
            logs.text().matches("mail dropped").count(),
            2,
            "{}",
            logs.text()
        );

        let logs = logs.text();
        for private in [
            GUEST,
            HOST_NAME,
            HOST_ADDRESS,
            "bearer-secret",
            "signed-link-secret",
        ] {
            assert!(!logs.contains(private), "{private} appears in logs: {logs}");
        }
        let errors =
            sqlx::query_scalar!("select last_error from outbox where last_error is not null")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.as_deref() != Some(PROVIDER_ERROR))
        );
    })
    .await;
}
