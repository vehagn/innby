//! Groups: the sign-up links the host makes, one per group of people.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Group {
    pub id: Uuid,
    pub event_id: Uuid,
    /// What the group is, e.g. "Climbing club". Shown on its sign-up page.
    pub name: String,
    /// How many people each sign-up may cover.
    pub max_party_size: i32,
    pub link_nonce: i32,
    pub created_at: DateTime<Utc>,
}

pub async fn create(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<Group> {
    sqlx::query_as!(
        Group,
        "insert into invite_group (event_id, name, max_party_size) values ($1, $2, $3) \
         returning id, event_id, name, max_party_size, link_nonce, created_at",
        event_id,
        name,
        max_party_size,
    )
    .fetch_one(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Group>> {
    sqlx::query_as!(
        Group,
        "select id, event_id, name, max_party_size, link_nonce, created_at \
         from invite_group where id = $1",
        id
    )
    .fetch_optional(db)
    .await
}

/// The event's groups, oldest first.
pub async fn list(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Vec<Group>> {
    sqlx::query_as!(
        Group,
        "select id, event_id, name, max_party_size, link_nonce, created_at \
         from invite_group where event_id = $1 order by id",
        event_id
    )
    .fetch_all(db)
    .await
}

/// Renames the group and changes how many people later sign-ups may cover.
pub async fn update(
    db: impl PgExecutor<'_>,
    id: Uuid,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update invite_group set name = $2, max_party_size = $3 where id = $1",
        id,
        name,
        max_party_size,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Invalidates the group's link by bumping its nonce. Returns the new nonce.
pub async fn revoke_link(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!(
        "update invite_group set link_nonce = link_nonce + 1 where id = $1 returning link_nonce",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Deletes the group if nobody has signed up through it.
pub async fn delete_unused(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "delete from invite_group where id = $1 \
         and not exists (select from rsvp where group_id = $1)",
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::{
        db::rsvp::{self, NameDisplay},
        i18n::Lang,
        web::PRIVACY_VERSION,
    };

    async fn event(pool: &PgPool) -> Uuid {
        crate::db::event::tests::event(pool).await.id
    }

    #[sqlx::test]
    async fn create_update_revoke(pool: PgPool) -> sqlx::Result<()> {
        let event = event(&pool).await;
        let group = create(&pool, event, "Climbing club", 2).await?;
        assert_eq!(
            (group.name.as_str(), group.link_nonce),
            ("Climbing club", 0)
        );

        assert!(update(&pool, group.id, "Climbing friends", 3).await?);
        assert_eq!(revoke_link(&pool, group.id).await?, Some(1));
        let updated = find(&pool, group.id).await?.unwrap();
        assert_eq!(
            (updated.name.as_str(), updated.max_party_size),
            ("Climbing friends", 3)
        );
        assert_eq!(list(&pool, event).await?.len(), 1);
        Ok(())
    }

    #[sqlx::test]
    async fn only_unused_groups_can_be_deleted(pool: PgPool) -> sqlx::Result<()> {
        let event = event(&pool).await;
        let used = create(&pool, event, "Climbing club", 2).await?;
        let unused = create(&pool, event, "Neighbours", 2).await?;
        let sign_up = rsvp::SignUp {
            group: &used,
            name: "Per",
            party_size: 1,
            name_display: NameDisplay::Full,
            lang: Lang::Nb,
            choice: None,
        };
        rsvp::create_open(&pool, &sign_up, PRIVACY_VERSION).await?;

        assert!(!delete_unused(&pool, used.id).await?);
        assert!(delete_unused(&pool, unused.id).await?);
        assert_eq!(list(&pool, event).await?.len(), 1);
        Ok(())
    }
}
