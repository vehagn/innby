//! `fest mailer`: sends the mail jobs in the outbox. The only workload with SMTP
//! credentials and a way out to the internet.

pub mod compose;

pub use compose::Mail;

use std::time::Duration;

use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use sqlx::PgPool;

use crate::{
    config::{Config, MailConfig},
    db::{outbox, rsvp},
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH_SIZE: i64 = 10;

/// Something that delivers a mail: SMTP in production, a list in tests.
pub trait Sender: Send + Sync {
    fn send(&self, from: &Mailbox, mail: Mail) -> impl Future<Output = Result<()>> + Send;
}

fn message(from: &Mailbox, mail: Mail) -> Result<Message> {
    Ok(Message::builder()
        .from(from.clone())
        .to(mail.to.parse()?)
        .subject(mail.subject)
        .header(ContentType::TEXT_PLAIN)
        .body(mail.body)?)
}

struct Smtp(AsyncSmtpTransport<Tokio1Executor>);

impl Smtp {
    fn new(mail: &MailConfig) -> Result<Self> {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::from_url(&mail.smtp_url)
            .context("FEST_SMTP_URL")?
            .timeout(Some(Duration::from_secs(30)));
        if let (Some(user), Some(password)) = (&mail.smtp_user, &mail.smtp_password) {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        Ok(Self(builder.build()))
    }
}

impl Sender for Smtp {
    async fn send(&self, from: &Mailbox, mail: Mail) -> Result<()> {
        self.0.send(message(from, mail)?).await?;
        Ok(())
    }
}

/// Sends due mail every few seconds until SIGTERM or Ctrl-C.
pub async fn run(config: Config, mail: MailConfig, db: PgPool) -> Result<()> {
    let smtp = Smtp::new(&mail)?;
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let shutdown = crate::web::shutdown_signal();
    tokio::pin!(shutdown);
    tracing::info!("mailer started");
    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            _ = interval.tick() => {}
        }
        // Keep going while there are full batches; errors wait for the next tick.
        loop {
            match process_batch(&config, &mail.from, &db, &smtp).await {
                Ok(sent) if sent == BATCH_SIZE as usize => continue,
                Ok(_) => break,
                Err(err) => {
                    tracing::warn!(error = format!("{err:#}"), "mailer batch failed");
                    break;
                }
            }
        }
    }
}

/// Sends one batch of due mail and returns how many jobs it handled.
pub async fn process_batch(
    config: &Config,
    from: &Mailbox,
    db: &PgPool,
    sender: &impl Sender,
) -> Result<usize> {
    let mut tx = db.begin().await?;
    let jobs = outbox::claim_due(&mut tx, BATCH_SIZE).await?;
    for job in &jobs {
        // The foreign key cascades, so the guest is still there.
        let Some(guest) = rsvp::find(&mut *tx, job.rsvp_id).await? else {
            continue;
        };
        let mail = match compose::compose(config, job.kind, &guest) {
            Ok(mail) => mail,
            Err(reason) => {
                tracing::info!(job = job.id, rsvp = %guest.id, kind = ?job.kind, reason, "mail dropped");
                outbox::mark_done(&mut tx, job.id, Some(reason)).await?;
                continue;
            }
        };
        match sender.send(from, mail).await {
            Ok(()) => {
                tracing::info!(job = job.id, rsvp = %guest.id, kind = ?job.kind, "mail sent");
                outbox::mark_done(&mut tx, job.id, None).await?;
            }
            Err(err) => {
                // SMTP errors often quote the address, so the details stay in the
                // database (deleted with the guest) rather than in the logs.
                tracing::warn!(job = job.id, rsvp = %guest.id, kind = ?job.kind, attempt = job.attempts + 1, "mail failed, see outbox.last_error");
                outbox::mark_failed(&mut tx, job.id, &format!("{err:#}")).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(jobs.len())
}

#[cfg(test)]
mod tests {
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
        };
        assert!(message(&from(), mail("kari@example.com")).is_ok());
        assert!(message(&from(), mail("not an address")).is_err());
    }

    /// Collects mails instead of sending them, or fails every send.
    #[derive(Default)]
    struct Outbox {
        sent: Mutex<Vec<Mail>>,
        fail: bool,
    }

    impl Sender for Outbox {
        async fn send(&self, from: &Mailbox, mail: Mail) -> Result<()> {
            anyhow::ensure!(!self.fail, "connection refused");
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
        let guest = create_invite(db, "Kari", 1).await.unwrap();
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
        let guest = guest_with_email(&db, "kari@example.com", Lang::En).await;
        outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
            .await
            .unwrap();

        let sender = Outbox::default();
        assert_eq!(
            process_batch(&config, &from(), &db, &sender).await.unwrap(),
            1
        );
        assert_eq!(unsent(&db).await, 0);

        let [(to, subject, body)] = sender.mails().try_into().unwrap();
        assert_eq!(to, "kari@example.com");
        assert_eq!(subject, "Confirm your e-mail address – Christmas party");
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
        let guest = guest_with_email(&db, "kari@example.com", Lang::Nb).await;
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
            process_batch(&config, &from(), &db, &sender).await.unwrap(),
            2
        );
        assert!(sender.mails().is_empty());
        assert_eq!(unsent(&db).await, 0);
    }

    #[sqlx::test]
    async fn sends_link_and_reminder_in_the_guests_language(db: PgPool) {
        let config = config();
        let guest = guest_with_email(&db, "kari@example.com", Lang::Nb).await;
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
        process_batch(&config, &from(), &db, &sender).await.unwrap();
        let mails = sender.mails();
        assert_eq!(mails.len(), 2);
        let link = config.link_key.rsvp_link(guest.id, guest.link_nonce);
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
        let guest = guest_with_email(&db, "kari@example.com", Lang::Nb).await;
        outbox::enqueue(&db, guest.id, Kind::ConfirmEmail)
            .await
            .unwrap();

        let failing = Outbox {
            fail: true,
            ..Outbox::default()
        };
        assert_eq!(
            process_batch(&config, &from(), &db, &failing)
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
            process_batch(&config, &from(), &db, &Outbox::default())
                .await
                .unwrap(),
            0
        );
    }
}
