//! The host's logins: refresh tokens, and JWTs logged out before they expired. Each write
//! also forgets the rows that have expired, so the tables stay small.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

pub async fn save_refresh_token(
    db: impl PgExecutor<'_>,
    digest: &[u8],
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "with expired as (delete from admin_refresh_token where expires_at <= now()) \
         insert into admin_refresh_token (digest, expires_at) values ($1, $2)",
        digest,
        expires_at,
    )
    .execute(db)
    .await
    .map(drop)
}

pub async fn has_refresh_token(db: impl PgExecutor<'_>, digest: &[u8]) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"select exists (
            select from admin_refresh_token where digest = $1 and expires_at > now()
        ) as "exists!""#,
        digest,
    )
    .fetch_one(db)
    .await
}

pub async fn delete_refresh_token(db: impl PgExecutor<'_>, digest: &[u8]) -> sqlx::Result<()> {
    sqlx::query!("delete from admin_refresh_token where digest = $1", digest)
        .execute(db)
        .await
        .map(drop)
}

pub async fn revoke_jwt(
    db: impl PgExecutor<'_>,
    jti: Uuid,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "with expired as (delete from admin_revoked_jwt where expires_at <= now()) \
         insert into admin_revoked_jwt (jti, expires_at) values ($1, $2) \
         on conflict do nothing",
        jti,
        expires_at,
    )
    .execute(db)
    .await
    .map(drop)
}

pub async fn is_revoked(db: impl PgExecutor<'_>, jti: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"select exists (select from admin_revoked_jwt where jti = $1) as "exists!""#,
        jti,
    )
    .fetch_one(db)
    .await
}
