//! Mail jobs, to guests and to hosts made in the admin UI. Whatever causes a mail writes
//! its job in the same transaction, and `innby mailer` sends them.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

use crate::i18n::Lang;

/// After this many failed sends a job is left alone; `last_error` says why.
pub const MAX_ATTEMPTS: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "outbox_kind", rename_all = "snake_case")]
pub enum Kind {
    /// The double opt-in link for a newly added address.
    ConfirmEmail,
    /// Tells an address's owner that someone tried to add it to another RSVP.
    EmailInUse,
    /// The guest's own RSVP link.
    RsvpLink,
    Reminder,
    /// The event's start, end or place changed.
    EventChanged,
    /// To a host made in the admin UI: a link to set a new password, which they asked
    /// for on the login page.
    PasswordReset,
    /// The same, for a first password, which an admin asked for when making them.
    PasswordSet,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    /// The guest it's for, or else the host made in the admin UI, in `lang`.
    pub rsvp_id: Option<Uuid>,
    pub admin_user: Option<String>,
    pub lang: Option<Lang>,
    pub kind: Kind,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
}

pub async fn enqueue(db: impl PgExecutor<'_>, rsvp_id: Uuid, kind: Kind) -> sqlx::Result<()> {
    sqlx::query!(
        "insert into outbox (rsvp_id, kind) values ($1, $2)",
        rsvp_id,
        kind as Kind,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Like `enqueue`, but does nothing while the same mail is still waiting to go out, so
/// repeated clicks don't turn into repeated mails.
pub async fn enqueue_once(
    db: impl PgExecutor<'_>,
    rsvp_id: Uuid,
    kind: Kind,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        insert into outbox (rsvp_id, kind)
        select $1, $2
        where not exists (
            select from outbox
            where rsvp_id = $1 and kind = $2 and sent_at is null and attempts < $3
        )
        "#,
        rsvp_id,
        kind as Kind,
        MAX_ATTEMPTS,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Queues a mail about a host's password, in `lang`, unless the same is still waiting to
/// go out. Returns whether it queued one.
pub async fn enqueue_for_host(
    db: impl PgExecutor<'_>,
    user: &str,
    kind: Kind,
    lang: Lang,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        insert into outbox (admin_user, lang, kind)
        select $1, $3, $2
        where not exists (
            select from outbox
            where admin_user = $1 and kind = $2 and sent_at is null and attempts < $4
        )
        "#,
        user,
        kind as Kind,
        lang as Lang,
        MAX_ATTEMPTS,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Drops the guest's unsent mail, e.g. when they remove their address.
pub async fn cancel_unsent(db: impl PgExecutor<'_>, rsvp_id: Uuid) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "delete from outbox where rsvp_id = $1 and sent_at is null",
        rsvp_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Locks up to `limit` jobs that are due. Other mailers skip them until the transaction
/// ends.
pub async fn claim_due(tx: &mut PgConnection, limit: i64) -> sqlx::Result<Vec<Job>> {
    sqlx::query_as!(
        Job,
        r#"
        select id, rsvp_id, admin_user, lang as "lang: Lang", kind as "kind: Kind", attempts,
            created_at
        from outbox
        where sent_at is null and next_try_at <= now() and attempts < $2
        order by id
        limit $1
        for update skip locked
        "#,
        limit,
        MAX_ATTEMPTS,
    )
    .fetch_all(tx)
    .await
}

/// How many jobs aren't sent, for the metrics: those still to be tried, and those given
/// up on after `MAX_ATTEMPTS`.
pub async fn unsent(db: impl PgExecutor<'_>) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query!(
        r#"
        select
            count(*) filter (where attempts < $1) as "waiting!",
            count(*) filter (where attempts >= $1) as "given_up!"
        from outbox
        where sent_at is null
        "#,
        MAX_ATTEMPTS,
    )
    .fetch_one(db)
    .await?;
    Ok((row.waiting, row.given_up))
}

/// Marks a job done. `note` records why a job was dropped without sending.
pub async fn mark_done(tx: &mut PgConnection, id: i64, note: Option<&str>) -> sqlx::Result<()> {
    sqlx::query!(
        "update outbox set sent_at = now(), last_error = $2 where id = $1",
        id,
        note,
    )
    .execute(tx)
    .await?;
    Ok(())
}

/// Records a failed send and retries with exponential backoff: 1, 2, 4… minutes, at
/// most 6 hours apart.
pub async fn mark_failed(tx: &mut PgConnection, id: i64, error: &str) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        update outbox
        set attempts = attempts + 1,
            last_error = $2,
            next_try_at = now() + least(interval '1 minute' * power(2, attempts), interval '6 hours')
        where id = $1
        "#,
        id,
        error,
    )
    .execute(tx)
    .await?;
    Ok(())
}

/// Queues a reminder for every guest with a confirmed address who hasn't declined and
/// hasn't had one yet. Returns how many were queued.
pub async fn queue_reminders(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        r#"
        insert into outbox (rsvp_id, kind)
        select id, 'reminder'
        from rsvp
        where event_id = $1 and email_status = 'confirmed' and response <> 'declined'
            and not exists (select from outbox where rsvp_id = rsvp.id and kind = 'reminder')
        "#,
        event_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Tells every guest with a confirmed address that the event's start, end or place
/// changed, whatever their answer: a new date may suit one who declined. A guest whose
/// notice is still waiting isn't queued another; it says what the event is when it goes.
/// Returns how many were queued.
pub async fn queue_change_notices(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        r#"
        insert into outbox (rsvp_id, kind)
        select id, 'event_changed'
        from rsvp
        where event_id = $1 and email_status = 'confirmed'
            and not exists (
                select from outbox
                where rsvp_id = rsvp.id and kind = 'event_changed' and sent_at is null
                    and attempts < $2
            )
        "#,
        event_id,
        MAX_ATTEMPTS,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::{
        db::rsvp::{self, Response},
        i18n::Lang,
        web::PRIVACY_VERSION,
    };

    async fn kinds(pool: &PgPool) -> Vec<(Kind, bool)> {
        sqlx::query!(
            r#"select kind as "kind: Kind", sent_at is not null as "sent!" from outbox order by id"#
        )
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.kind, row.sent))
        .collect()
    }

    #[sqlx::test]
    async fn enqueue_once_skips_duplicates_while_unsent(pool: PgPool) -> sqlx::Result<()> {
        let guest = rsvp::create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Kari",
            1,
        )
        .await?;
        assert!(enqueue_once(&pool, guest.id, Kind::RsvpLink).await?);
        assert!(!enqueue_once(&pool, guest.id, Kind::RsvpLink).await?);
        assert!(enqueue_once(&pool, guest.id, Kind::Reminder).await?);

        let mut tx = pool.begin().await?;
        let jobs = claim_due(&mut tx, 10).await?;
        mark_done(&mut tx, jobs[0].id, None).await?;
        tx.commit().await?;
        assert!(enqueue_once(&pool, guest.id, Kind::RsvpLink).await?);
        Ok(())
    }

    #[sqlx::test]
    async fn claimed_jobs_are_skipped_by_others(pool: PgPool) -> sqlx::Result<()> {
        let guest = rsvp::create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Kari",
            1,
        )
        .await?;
        enqueue(&pool, guest.id, Kind::RsvpLink).await?;
        enqueue(&pool, guest.id, Kind::Reminder).await?;

        let mut first = pool.begin().await?;
        assert_eq!(claim_due(&mut first, 1).await?.len(), 1);
        let mut second = pool.begin().await?;
        let others = claim_due(&mut second, 10).await?;
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].kind, Kind::Reminder);
        Ok(())
    }

    #[sqlx::test]
    async fn failures_back_off_and_give_up(pool: PgPool) -> sqlx::Result<()> {
        let guest = rsvp::create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Kari",
            1,
        )
        .await?;
        enqueue(&pool, guest.id, Kind::RsvpLink).await?;
        let mut tx = pool.begin().await?;
        let job = claim_due(&mut tx, 1).await?.remove(0);
        mark_failed(&mut tx, job.id, "connection refused").await?;
        tx.commit().await?;

        let mut tx = pool.begin().await?;
        assert!(claim_due(&mut tx, 10).await?.is_empty(), "not due yet");
        sqlx::query!(
            "update outbox set next_try_at = now(), attempts = $1",
            MAX_ATTEMPTS
        )
        .execute(&mut *tx)
        .await?;
        assert!(claim_due(&mut tx, 10).await?.is_empty(), "given up");
        Ok(())
    }

    #[sqlx::test]
    async fn reminders_go_to_confirmed_guests_once(pool: PgPool) -> sqlx::Result<()> {
        let mut confirmed = Vec::new();
        for (name, email) in [("Kari", "kari@example.com"), ("Ola", "ola@example.com")] {
            let guest = rsvp::create_invite(
                &pool,
                crate::db::event::tests::event(&pool).await.id,
                name,
                1,
            )
            .await?;
            rsvp::set_email(&pool, guest.id, email, Lang::NB, PRIVACY_VERSION).await?;
            rsvp::confirm_email(&pool, guest.id, email).await?;
            confirmed.push(guest);
        }
        rsvp::respond(
            &pool,
            confirmed[1].id,
            &rsvp::Reply {
                response: Response::Declined,
                party_size: 1,
                name_display: None,
                choice: None,
            },
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?;
        let unconfirmed = rsvp::create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Per",
            1,
        )
        .await?;
        rsvp::set_email(
            &pool,
            unconfirmed.id,
            "per@example.com",
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?;

        assert_eq!(
            queue_reminders(&pool, crate::db::event::tests::event(&pool).await.id).await?,
            1
        );
        assert_eq!(
            queue_reminders(&pool, crate::db::event::tests::event(&pool).await.id).await?,
            0
        );
        assert_eq!(kinds(&pool).await, [(Kind::Reminder, false)]);

        cancel_unsent(&pool, confirmed[0].id).await?;
        assert!(kinds(&pool).await.is_empty());
        Ok(())
    }
}
