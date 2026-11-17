//! Hosts who log in with a password, made by an admin in the admin UI (`admin::users`),
//! beside those in the config. Their role is read on each request, so a new one counts
//! at once; their UUID and password version bind sessions and links to this account and
//! credential. The UUID is deleted with the account. One may have an e-mail address, for
//! a link to set a password, and have none until they set one from the first such link.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCredential {
    pub id: Uuid,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser {
    pub id: Uuid,
    pub name: String,
    /// `admin`, `creator`, `editor`, `organiser` or `viewer`, or none of their own.
    pub role: Option<String>,
    /// None until they set one from a link.
    pub password_hash: Option<String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub email: Option<String>,
}

impl AdminUser {
    pub fn credential(&self) -> AccountCredential {
        AccountCredential {
            id: self.id,
            version: self.version,
        }
    }
}

/// Why a user couldn't be made or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Name,
    Email,
}

/// Whether a database error is a unique index's, and which.
fn taken(err: &sqlx::Error) -> Option<Taken> {
    let err = err.as_database_error()?;
    match err.constraint()? {
        "admin_user_pkey" => Some(Taken::Name),
        "admin_user_email_key" => Some(Taken::Email),
        _ => None,
    }
}

pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<AdminUser>> {
    sqlx::query_as!(
        AdminUser,
        "select id, name, role, password_hash, version, created_at, email from admin_user order by name"
    )
    .fetch_all(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, name: &str) -> sqlx::Result<Option<AdminUser>> {
    sqlx::query_as!(
        AdminUser,
        "select id, name, role, password_hash, version, created_at, email from admin_user where name = $1",
        name
    )
    .fetch_optional(db)
    .await
}

/// The user with this name, or else this address, as someone asking for a new password
/// types either.
pub async fn find_by_name_or_email(
    db: impl PgExecutor<'_>,
    typed: &str,
) -> sqlx::Result<Option<AdminUser>> {
    sqlx::query_as!(
        AdminUser,
        "select id, name, role, password_hash, version, created_at, email from admin_user
         where name = $1 or email = lower($1)
         order by name = $1 desc limit 1",
        typed
    )
    .fetch_optional(db)
    .await
}

/// Whether anyone may log in with a password from the database.
pub async fn any(db: impl PgExecutor<'_>) -> sqlx::Result<bool> {
    sqlx::query_scalar!(r#"select exists (select from admin_user) as "any!""#)
        .fetch_one(db)
        .await
}

/// Makes a user, with a password, or an address for a link to set one, or both.
/// `email` is normalised (`rsvp::normalize_email`).
pub async fn create(
    db: impl PgExecutor<'_>,
    name: &str,
    role: Option<&str>,
    password_hash: Option<&str>,
    email: Option<&str>,
) -> sqlx::Result<Result<(), Taken>> {
    let result = sqlx::query!(
        "insert into admin_user (name, role, password_hash, email) values ($1, $2, $3, $4)",
        name,
        role,
        password_hash,
        email,
    )
    .execute(db)
    .await;
    match result {
        Ok(_) => Ok(Ok(())),
        Err(err) => taken(&err).map(Err).ok_or(err),
    }
}

/// Sets or clears the user's address, normalised. Returns whether there's such a user,
/// or that another has the address.
pub async fn set_email(
    db: impl PgExecutor<'_>,
    name: &str,
    email: Option<&str>,
) -> sqlx::Result<Result<bool, Taken>> {
    let result = sqlx::query!(
        "update admin_user set email = $2 where name = $1",
        name,
        email
    )
    .execute(db)
    .await;
    match result {
        Ok(done) => Ok(Ok(done.rows_affected() == 1)),
        Err(err) => taken(&err).map(Err).ok_or(err),
    }
}

/// Sets the address only for the account credential that authenticated the request.
pub async fn set_email_for_account(
    db: impl PgExecutor<'_>,
    expected: AccountCredential,
    email: Option<&str>,
) -> sqlx::Result<Result<bool, Taken>> {
    let result = sqlx::query!(
        "update admin_user set email = $3 where id = $1 and version = $2",
        expected.id,
        expected.version,
        email
    )
    .execute(db)
    .await;
    match result {
        Ok(done) => Ok(Ok(done.rows_affected() == 1)),
        Err(err) => taken(&err).map(Err).ok_or(err),
    }
}

pub async fn set_role(
    db: impl PgExecutor<'_>,
    name: &str,
    role: Option<&str>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update admin_user set role = $2 where name = $1",
        name,
        role
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Changes a password if the account and version are still those verified by the caller.
pub async fn change_password(
    db: impl PgExecutor<'_>,
    expected: AccountCredential,
    password_hash: &str,
) -> sqlx::Result<Option<AccountCredential>> {
    sqlx::query_as!(
        AccountCredential,
        "update admin_user set password_hash = $3, version = version + 1
         where id = $1 and version = $2
         returning id, version",
        expected.id,
        expected.version,
        password_hash,
    )
    .fetch_optional(db)
    .await
}

/// Consumes a verified, unexpired reset link if the account still has its signed address
/// and credential. The update both establishes and returns the new credential.
pub async fn consume_password_reset(
    db: impl PgExecutor<'_>,
    expected: AccountCredential,
    email: &str,
    expires_at: DateTime<Utc>,
    password_hash: &str,
) -> sqlx::Result<Option<AccountCredential>> {
    sqlx::query_as!(
        AccountCredential,
        "update admin_user set password_hash = $5, version = version + 1
         where id = $1 and version = $2 and email = $3 and $4 > clock_timestamp()
         returning id, version",
        expected.id,
        expected.version,
        email,
        expires_at,
        password_hash,
    )
    .fetch_optional(db)
    .await
}

/// Deletes the user, the roles events gave them by name, and their name on the events
/// they made, none of which another login of the same name may inherit.
pub async fn delete(db: &PgPool, name: &str) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    sqlx::query!("delete from event_access where user_name = $1", name)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "update event set created_by = null where created_by = $1",
        name
    )
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query!("delete from admin_user where name = $1", name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests;
