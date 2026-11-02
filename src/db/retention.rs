//! Deleting what we no longer need. `innby janitor` runs these every hour.

use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};

use super::outbox::MAX_ATTEMPTS;

/// Clears addresses that were never confirmed since before `cutoff`, along with their
/// unsent mail. Returns how many were cleared.
pub async fn clear_unconfirmed_emails(db: &PgPool, cutoff: DateTime<Utc>) -> sqlx::Result<u64> {
    let mut tx = db.begin().await?;
    let cleared = sqlx::query_scalar!(
        r#"
        update rsvp set email = null, email_status = 'none', email_consent_at = null
        where email_status = 'unconfirmed' and email_consent_at < $1
        returning id
        "#,
        cutoff,
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query!(
        "delete from outbox where rsvp_id = any($1) and sent_at is null",
        &cleared,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(cleared.len() as u64)
}

/// Deletes mail jobs that were sent, or given up on, before `cutoff`.
pub async fn delete_old_mail(db: impl PgExecutor<'_>, cutoff: DateTime<Utc>) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        r#"
        delete from outbox
        where sent_at < $1 or (sent_at is null and attempts >= $2 and created_at < $1)
        "#,
        cutoff,
        MAX_ATTEMPTS,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Deletes every RSVP, and through the cascade all their mail, then the groups.
pub async fn delete_all_rsvps(db: &PgPool) -> sqlx::Result<u64> {
    let mut tx = db.begin().await?;
    let result = sqlx::query!("delete from rsvp").execute(&mut *tx).await?;
    sqlx::query!("delete from invite_group")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}
