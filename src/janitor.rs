//! `innby janitor`: enforces the retention promised in the privacy notice. Runs once and
//! exits; a Kubernetes CronJob runs it every hour.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use crate::{
    db::{event, retention},
    mailer::compose::CONFIRM_WITHIN,
};

/// How long a sent mail's record is kept, for troubleshooting delivery.
pub const KEEP_SENT_MAIL: Duration = Duration::days(7);
/// Each event, and every guest's data with it, is deleted this long after it starts.
pub const KEEP_AFTER_EVENT: Duration = Duration::days(30);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub emails_cleared: u64,
    pub mails_deleted: u64,
    pub rsvps_deleted: u64,
}

pub async fn run(db: &PgPool, now: DateTime<Utc>) -> anyhow::Result<Report> {
    let report = Report {
        emails_cleared: retention::clear_unconfirmed_emails(db, now - CONFIRM_WITHIN).await?,
        mails_deleted: retention::delete_old_mail(db, now - KEEP_SENT_MAIL).await?,
        rsvps_deleted: event::delete_started_by(db, now - KEEP_AFTER_EVENT).await?,
    };
    tracing::info!(
        emails_cleared = report.emails_cleared,
        mails_deleted = report.mails_deleted,
        rsvps_deleted = report.rsvps_deleted,
        "janitor done"
    );
    Ok(report)
}

#[cfg(test)]
mod tests;
