// The RSVP pages and the admin UI (milestones 3–5) are the callers; until then only the
// tests use these.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "rsvp_source", rename_all = "lowercase")]
pub enum Source {
    Invite,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "rsvp_response", rename_all = "lowercase")]
pub enum Response {
    Pending,
    Attending,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "email_status", rename_all = "lowercase")]
pub enum EmailStatus {
    None,
    Unconfirmed,
    Confirmed,
}

#[derive(Debug, Clone)]
pub struct Rsvp {
    pub id: Uuid,
    pub source: Source,
    pub name: String,
    pub response: Response,
    pub party_size: i32,
    pub max_party_size: i32,
    pub link_nonce: i32,
    pub lang: Lang,
    pub email: Option<String>,
    pub email_status: EmailStatus,
    pub email_consent_at: Option<DateTime<Utc>>,
    pub privacy_version: Option<String>,
    pub responded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Headcount {
    /// Invitations and open RSVPs that answered "attending".
    pub attending_parties: i64,
    /// The sum of their party sizes: the number of people coming.
    pub attending_people: i64,
    pub declined: i64,
    pub pending: i64,
}

// The `query_as!` macros need string literals, so every query repeats the column list
// in `returning` / `select`, with type overrides for the enum columns.

/// Creates an invitation that the guest answers later through their link.
pub async fn create_invite(
    db: impl PgExecutor<'_>,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<Rsvp> {
    sqlx::query_as!(
        Rsvp,
        r#"
        insert into rsvp (source, name, max_party_size)
        values ('invite', $1, $2)
        returning id, source as "source: Source", name, response as "response: Response",
            party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
            email_status as "email_status: EmailStatus", email_consent_at, privacy_version,
            responded_at, created_at
        "#,
        name,
        max_party_size,
    )
    .fetch_one(db)
    .await
}

/// Creates an open RSVP, which is "attending" from the start.
pub async fn create_open(
    db: impl PgExecutor<'_>,
    name: &str,
    party_size: i32,
    max_party_size: i32,
    lang: Lang,
    privacy_version: &str,
) -> sqlx::Result<Rsvp> {
    sqlx::query_as!(
        Rsvp,
        r#"
        insert into rsvp (source, name, response, party_size, max_party_size, lang,
            privacy_version, responded_at)
        values ('open', $1, 'attending', $2, $3, $4, $5, now())
        returning id, source as "source: Source", name, response as "response: Response",
            party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
            email_status as "email_status: EmailStatus", email_consent_at, privacy_version,
            responded_at, created_at
        "#,
        name,
        party_size,
        max_party_size,
        lang as Lang,
        privacy_version,
    )
    .fetch_one(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Rsvp>> {
    sqlx::query_as!(
        Rsvp,
        r#"
        select id, source as "source: Source", name, response as "response: Response",
            party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
            email_status as "email_status: EmailStatus", email_consent_at, privacy_version,
            responded_at, created_at
        from rsvp
        where id = $1
        "#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// All RSVPs, oldest first (the ids are UUIDv7, so they sort by creation time).
pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Rsvp>> {
    sqlx::query_as!(
        Rsvp,
        r#"
        select id, source as "source: Source", name, response as "response: Response",
            party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
            email_status as "email_status: EmailStatus", email_consent_at, privacy_version,
            responded_at, created_at
        from rsvp
        order by id
        "#,
    )
    .fetch_all(db)
    .await
}

/// Records the guest's answer. A party size outside `1..=max_party_size` violates a
/// check constraint, so callers validate it first.
pub async fn respond(
    db: impl PgExecutor<'_>,
    id: Uuid,
    response: Response,
    party_size: i32,
    lang: Lang,
    privacy_version: &str,
) -> sqlx::Result<Option<Rsvp>> {
    sqlx::query_as!(
        Rsvp,
        r#"
        update rsvp
        set response = $2, party_size = $3, lang = $4, privacy_version = $5, responded_at = now()
        where id = $1
        returning id, source as "source: Source", name, response as "response: Response",
            party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
            email_status as "email_status: EmailStatus", email_consent_at, privacy_version,
            responded_at, created_at
        "#,
        id,
        response as Response,
        party_size,
        lang as Lang,
        privacy_version,
    )
    .fetch_optional(db)
    .await
}

pub async fn rename(db: impl PgExecutor<'_>, id: Uuid, name: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!("update rsvp set name = $2 where id = $1", id, name)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Invalidates the guest's link by bumping its nonce. Returns the new nonce.
pub async fn revoke_link(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!(
        "update rsvp set link_nonce = link_nonce + 1 where id = $1 returning link_nonce",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Deletes the RSVP and, through `on delete cascade`, its pending mail.
pub async fn delete(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!("delete from rsvp where id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn headcount(db: impl PgExecutor<'_>) -> sqlx::Result<Headcount> {
    sqlx::query_as!(
        Headcount,
        r#"
        select
            count(*) filter (where response = 'attending') as "attending_parties!",
            coalesce(sum(party_size) filter (where response = 'attending'), 0)
                as "attending_people!",
            count(*) filter (where response = 'declined') as "declined!",
            count(*) filter (where response = 'pending') as "pending!"
        from rsvp
        "#,
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    const PRIVACY: &str = "2026-11-01";
    const CHECK_VIOLATION: &str = "23514";

    fn is_check_violation(err: &sqlx::Error) -> bool {
        err.as_database_error()
            .and_then(|err| err.code())
            .is_some_and(|code| code == CHECK_VIOLATION)
    }

    #[sqlx::test]
    async fn invitation_starts_pending(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_invite(&pool, "Kari & Ola", 2).await?;
        assert_eq!(rsvp.source, Source::Invite);
        assert_eq!(rsvp.response, Response::Pending);
        assert_eq!((rsvp.party_size, rsvp.max_party_size), (1, 2));
        assert_eq!((rsvp.link_nonce, rsvp.lang), (0, Lang::Nb));
        assert_eq!(rsvp.email_status, EmailStatus::None);
        assert_eq!(rsvp.id.get_version_num(), 7);
        assert!(rsvp.responded_at.is_none());
        Ok(())
    }

    #[sqlx::test]
    async fn open_rsvp_is_attending_immediately(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_open(&pool, "Per", 2, 2, Lang::En, PRIVACY).await?;
        assert_eq!(rsvp.source, Source::Open);
        assert_eq!(rsvp.response, Response::Attending);
        assert_eq!((rsvp.party_size, rsvp.lang), (2, Lang::En));
        assert_eq!(rsvp.privacy_version.as_deref(), Some(PRIVACY));
        assert!(rsvp.responded_at.is_some());
        Ok(())
    }

    #[sqlx::test]
    async fn respond_records_the_answer(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari & Ola", 2).await?;
        let answered = respond(&pool, invite.id, Response::Attending, 2, Lang::En, PRIVACY)
            .await?
            .expect("rsvp exists");
        assert_eq!(answered.response, Response::Attending);
        assert_eq!((answered.party_size, answered.lang), (2, Lang::En));
        assert!(answered.responded_at.is_some());

        let reread = find(&pool, invite.id).await?.expect("rsvp exists");
        assert_eq!(reread.response, Response::Attending);
        Ok(())
    }

    #[sqlx::test]
    async fn respond_rejects_party_larger_than_invited(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari & Ola", 2).await?;
        let err = respond(&pool, invite.id, Response::Attending, 3, Lang::Nb, PRIVACY)
            .await
            .unwrap_err();
        assert!(is_check_violation(&err), "{err}");
        Ok(())
    }

    #[sqlx::test]
    async fn unknown_id_is_not_found(pool: PgPool) -> sqlx::Result<()> {
        let id = Uuid::max();
        assert!(find(&pool, id).await?.is_none());
        assert!(
            respond(&pool, id, Response::Declined, 1, Lang::Nb, PRIVACY)
                .await?
                .is_none()
        );
        assert!(!rename(&pool, id, "Nobody").await?);
        assert!(revoke_link(&pool, id).await?.is_none());
        assert!(!delete(&pool, id).await?);
        Ok(())
    }

    #[sqlx::test]
    async fn rename_enforces_name_length(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari", 1).await?;
        assert!(rename(&pool, invite.id, "Kari Nordmann").await?);
        assert_eq!(find(&pool, invite.id).await?.unwrap().name, "Kari Nordmann");

        let err = rename(&pool, invite.id, "").await.unwrap_err();
        assert!(is_check_violation(&err), "{err}");
        Ok(())
    }

    #[sqlx::test]
    async fn revoke_link_bumps_nonce(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari", 1).await?;
        assert_eq!(revoke_link(&pool, invite.id).await?, Some(1));
        assert_eq!(revoke_link(&pool, invite.id).await?, Some(2));
        Ok(())
    }

    #[sqlx::test]
    async fn delete_removes_pending_mail(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_open(&pool, "Per", 1, 2, Lang::Nb, PRIVACY).await?;
        sqlx::query("insert into outbox (rsvp_id, kind) values ($1, 'rsvp_link')")
            .bind(rsvp.id)
            .execute(&pool)
            .await?;

        assert!(delete(&pool, rsvp.id).await?);
        let mails: i64 = sqlx::query_scalar("select count(*) from outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(mails, 0);
        Ok(())
    }

    #[sqlx::test]
    async fn email_requires_consent_and_status(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_invite(&pool, "Kari", 1).await?;
        let err = sqlx::query("update rsvp set email = 'kari@example.com' where id = $1")
            .bind(rsvp.id)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(is_check_violation(&err), "{err}");

        sqlx::query(
            "update rsvp set email = 'kari@example.com', email_status = 'unconfirmed', \
             email_consent_at = now() where id = $1",
        )
        .bind(rsvp.id)
        .execute(&pool)
        .await?;
        let rsvp = find(&pool, rsvp.id).await?.unwrap();
        assert_eq!(rsvp.email_status, EmailStatus::Unconfirmed);
        Ok(())
    }

    #[sqlx::test]
    async fn headcount_sums_attending_parties(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(
            headcount(&pool).await?,
            Headcount {
                attending_parties: 0,
                attending_people: 0,
                declined: 0,
                pending: 0,
            }
        );

        let family = create_invite(&pool, "Familien Hansen", 4).await?;
        respond(&pool, family.id, Response::Attending, 3, Lang::Nb, PRIVACY).await?;
        let busy = create_invite(&pool, "Ola", 1).await?;
        respond(&pool, busy.id, Response::Declined, 1, Lang::Nb, PRIVACY).await?;
        create_invite(&pool, "Kari", 1).await?;
        create_open(&pool, "Per", 2, 2, Lang::En, PRIVACY).await?;

        assert_eq!(
            headcount(&pool).await?,
            Headcount {
                attending_parties: 2,
                attending_people: 5,
                declined: 1,
                pending: 1,
            }
        );
        Ok(())
    }
}
