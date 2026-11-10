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

/// Deletes what the logins and the admin's forms leave behind once it's no use: refresh
/// tokens and logged-out JWTs that have expired, and form tokens from more than a day
/// ago. A login, a logout or a form sweeps its own table as it writes; this does when
/// nobody has come by. Returns how many rows went.
pub async fn delete_expired_tokens(db: &PgPool, now: DateTime<Utc>) -> sqlx::Result<u64> {
    let deleted = sqlx::query_scalar!(
        r#"
        with tokens as (delete from admin_refresh_token where expires_at <= $1 returning 1),
            jwts as (delete from admin_revoked_jwt where expires_at <= $1 returning 1),
            forms as (
                delete from admin_form_sent where sent_at <= $1 - interval '1 day' returning 1
            )
        select (select count(*) from tokens) + (select count(*) from jwts)
            + (select count(*) from forms) as "deleted!"
        "#,
        now,
    )
    .fetch_one(db)
    .await?;
    Ok(deleted as u64)
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
