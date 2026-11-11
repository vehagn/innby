//! `innby mailer`: sends the mail jobs in the outbox. The only workload with SMTP
//! credentials and a way out to the internet.

pub mod compose;

pub use compose::Mail;

use std::time::Duration;

use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use sqlx::PgPool;

use crate::{
    config::{Config, MailConfig},
    db::{event, outbox, rsvp},
    metrics::MAILS,
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH_SIZE: i64 = 10;

/// Something that delivers a mail: SMTP in production, a list in tests.
pub trait Sender: Send + Sync {
    fn send(&self, from: &Mailbox, mail: Mail) -> impl Future<Output = Result<()>> + Send;
}

fn message(from: &Mailbox, mail: Mail) -> Result<Message> {
    let builder = Message::builder()
        .from(from.clone())
        .to(mail.to.parse()?)
        .subject(mail.subject);
    let Some(calendar) = mail.calendar else {
        return Ok(builder.header(ContentType::TEXT_PLAIN).body(mail.body)?);
    };
    // The text, and the event for the guest's calendar beside it.
    let file = Attachment::new("event.ics".into())
        .body(calendar, ContentType::parse(crate::calendar::CONTENT_TYPE)?);
    Ok(builder.multipart(
        MultiPart::mixed()
            .singlepart(SinglePart::plain(mail.body))
            .singlepart(file),
    )?)
}

struct Smtp(AsyncSmtpTransport<Tokio1Executor>);

impl Smtp {
    fn new(mail: &MailConfig) -> Result<Self> {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::from_url(&mail.smtp_url)
            .context("INNBY_SMTP_URL")?
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
    let shutdown = crate::http::shutdown_signal();
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
        // The event cascades too.
        let Some(event) = event::find(&mut *tx, guest.event_id).await? else {
            continue;
        };
        let mail = match compose::compose(config, job.kind, &guest, &event) {
            Ok(mail) => mail,
            Err(reason) => {
                tracing::info!(job = job.id, rsvp = %guest.id, kind = ?job.kind, reason, "mail dropped");
                MAILS.inc(&[("outcome", "dropped".into())]);
                outbox::mark_done(&mut tx, job.id, Some(reason)).await?;
                continue;
            }
        };
        match sender.send(from, mail).await {
            Ok(()) => {
                tracing::info!(job = job.id, rsvp = %guest.id, kind = ?job.kind, "mail sent");
                MAILS.inc(&[("outcome", "sent".into())]);
                outbox::mark_done(&mut tx, job.id, None).await?;
            }
            Err(err) => {
                // SMTP errors often quote the address, so the details stay in the
                // database (deleted with the guest) rather than in the logs.
                tracing::warn!(job = job.id, rsvp = %guest.id, kind = ?job.kind, attempt = job.attempts + 1, "mail failed, see outbox.last_error");
                MAILS.inc(&[("outcome", "failed".into())]);
                outbox::mark_failed(&mut tx, job.id, &format!("{err:#}")).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(jobs.len())
}

#[cfg(test)]
mod tests;
