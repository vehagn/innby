//! Hosts who log in with a password, made by an admin in the admin UI (`admin::users`),
//! beside those in the config. Their role is read on each request, so a new one counts
//! at once; a new password bumps their version, which their sessions carry, so it logs
//! them out everywhere.

use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser {
    pub name: String,
    /// `admin`, `creator`, `editor` or `viewer`, or none of their own.
    pub role: Option<String>,
    pub password_hash: String,
    pub version: i32,
    pub created_at: DateTime<Utc>,
}

pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<AdminUser>> {
    sqlx::query_as!(
        AdminUser,
        "select name, role, password_hash, version, created_at from admin_user order by name"
    )
    .fetch_all(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, name: &str) -> sqlx::Result<Option<AdminUser>> {
    sqlx::query_as!(
        AdminUser,
        "select name, role, password_hash, version, created_at from admin_user where name = $1",
        name
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

/// Makes a user. Returns false if there's one by the name already.
pub async fn create(
    db: impl PgExecutor<'_>,
    name: &str,
    role: Option<&str>,
    password_hash: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "insert into admin_user (name, role, password_hash) values ($1, $2, $3)
         on conflict (name) do nothing",
        name,
        role,
        password_hash,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
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

/// A new password, which logs the user out everywhere.
pub async fn set_password(
    db: impl PgExecutor<'_>,
    name: &str,
    password_hash: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update admin_user set password_hash = $2, version = version + 1 where name = $1",
        name,
        password_hash,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
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
mod tests {
    use super::*;
    use crate::db::{access, event};

    const HASH: &str = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaA";

    #[sqlx::test]
    async fn users_come_and_go_with_their_roles(db: PgPool) {
        assert!(!any(&db).await.unwrap());
        assert!(create(&db, "kari", Some("editor"), HASH).await.unwrap());
        assert!(!create(&db, "kari", None, HASH).await.unwrap());
        assert!(any(&db).await.unwrap());
        assert!(set_role(&db, "kari", None).await.unwrap());
        assert!(set_password(&db, "kari", HASH).await.unwrap());
        let kari = find(&db, "kari").await.unwrap().unwrap();
        assert_eq!((kari.role, kari.version), (None, 1));

        // Deleting them takes the roles events gave them too.
        let party = event::tests::event(&db).await;
        access::give(&db, party.id, "kari", access::EventRole::Viewer)
            .await
            .unwrap();
        assert!(delete(&db, "kari").await.unwrap());
        assert!(!access::has_any(&db, "kari").await.unwrap());
        assert!(find(&db, "kari").await.unwrap().is_none());
        assert!(!delete(&db, "kari").await.unwrap());
    }

    #[sqlx::test]
    async fn the_database_refuses_what_the_form_would(db: PgPool) {
        for (name, role, hash) in [
            ("kåre", None, HASH),
            ("", None, HASH),
            ("ola", Some("owner"), HASH),
            ("ola", None, "hunter2"),
        ] {
            assert!(
                create(&db, name, role, hash).await.is_err(),
                "{name} {role:?} {hash}"
            );
        }
    }
}
