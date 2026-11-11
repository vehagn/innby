//! Who may see or run one event, by the name they log in with: a role for that event
//! alone, which an admin gives on its Access tab. A host's own role, from the config or
//! their identity provider, is for every event; this gives others one event, or more
//! than their own role for it. It goes with the event. So does the name of the host who
//! made it, which lets a creator delete the events they made and give roles for them.

use serde::Deserialize;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The longest name the Access tab takes: an e-mail address can be this long, and some
/// identity providers name users by theirs.
pub const MAX_USER_LEN: usize = 254;

/// A role for one event. An admin's is for every event, so it's not among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type, Deserialize)]
#[sqlx(type_name = "event_role", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum EventRole {
    Viewer,
    Organiser,
    Editor,
}

impl EventRole {
    pub const ALL: [Self; 3] = [Self::Viewer, Self::Organiser, Self::Editor];

    pub fn code(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Organiser => "organiser",
            Self::Editor => "editor",
        }
    }
}

/// Someone's role for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub user: String,
    pub role: EventRole,
}

/// What an admin page is about: an event, or one of its guests or groups.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Event(Uuid),
    Guest(Uuid),
    Group(Uuid),
}

/// The event's hosts with a role for it alone, by name.
pub async fn list(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Vec<Access>> {
    sqlx::query_as!(
        Access,
        r#"select user_name as "user", role as "role: EventRole" from event_access
         where event_id = $1 order by user_name"#,
        event_id
    )
    .fetch_all(db)
    .await
}

/// Gives the user this role for the event, in place of any they had for it.
pub async fn give(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    user: &str,
    role: EventRole,
) -> sqlx::Result<()> {
    sqlx::query!(
        "insert into event_access (event_id, user_name, role) values ($1, $2, $3)
         on conflict (event_id, user_name) do update set role = excluded.role",
        event_id,
        user,
        role as _,
    )
    .execute(db)
    .await
    .map(drop)
}

/// Takes the user's role for the event away. Returns whether they had one.
pub async fn take(db: impl PgExecutor<'_>, event_id: Uuid, user: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "delete from event_access where event_id = $1 and user_name = $2",
        event_id,
        user,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The user's role for the event the page is about, if they have one for it.
pub async fn role(
    db: impl PgExecutor<'_>,
    scope: Scope,
    user: &str,
) -> sqlx::Result<Option<EventRole>> {
    match scope {
        Scope::Event(id) => {
            sqlx::query_scalar!(
                r#"select role as "role: EventRole" from event_access
                 where event_id = $1 and user_name = $2"#,
                id,
                user
            )
            .fetch_optional(db)
            .await
        }
        Scope::Guest(id) => {
            sqlx::query_scalar!(
                r#"select a.role as "role: EventRole"
                 from rsvp r join event_access a on a.event_id = r.event_id
                 where r.id = $1 and a.user_name = $2"#,
                id,
                user
            )
            .fetch_optional(db)
            .await
        }
        Scope::Group(id) => {
            sqlx::query_scalar!(
                r#"select a.role as "role: EventRole"
                 from invite_group g join event_access a on a.event_id = g.event_id
                 where g.id = $1 and a.user_name = $2"#,
                id,
                user
            )
            .fetch_optional(db)
            .await
        }
    }
}

/// Notes that the user made the event, so that, as a creator, they may delete it and
/// give roles for it.
pub async fn set_maker(db: impl PgExecutor<'_>, event_id: Uuid, user: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "update event set created_by = $2 where id = $1",
        event_id,
        user
    )
    .execute(db)
    .await
    .map(drop)
}

/// Whether the user made the event.
pub async fn made(db: impl PgExecutor<'_>, event_id: Uuid, user: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"select exists (select from event where id = $1 and created_by = $2) as "made!""#,
        event_id,
        user
    )
    .fetch_one(db)
    .await
}

/// Whether the user has a role for any event, so may log in without one of their own.
pub async fn has_any(db: impl PgExecutor<'_>, user: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"select exists (select from event_access where user_name = $1) as "exists!""#,
        user
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::db::{event, group, rsvp};

    #[sqlx::test]
    async fn a_role_for_one_event_reaches_its_guests_and_groups(db: PgPool) {
        let ours = event::tests::event(&db).await;
        let other = event::create(&db, &event::tests::details()).await.unwrap();
        let guest = rsvp::create_invite(&db, ours.id, "Kari", 1).await.unwrap();
        let club = group::create(&db, ours.id, "Climbers", 2).await.unwrap();
        let elsewhere = rsvp::create_invite(&db, other.id, "Per", 1).await.unwrap();

        assert!(!has_any(&db, "ola").await.unwrap());
        give(&db, ours.id, "ola", EventRole::Viewer).await.unwrap();
        give(&db, ours.id, "ola", EventRole::Editor).await.unwrap();
        give(&db, other.id, "kari@example.com", EventRole::Viewer)
            .await
            .unwrap();
        assert!(has_any(&db, "ola").await.unwrap());
        let editor = Some(EventRole::Editor);
        for scope in [
            Scope::Event(ours.id),
            Scope::Guest(guest.id),
            Scope::Group(club.id),
        ] {
            assert_eq!(role(&db, scope, "ola").await.unwrap(), editor, "{scope:?}");
            // Names are as given, exactly.
            assert_eq!(role(&db, scope, "Ola").await.unwrap(), None, "{scope:?}");
        }
        for scope in [Scope::Event(other.id), Scope::Guest(elsewhere.id)] {
            assert_eq!(role(&db, scope, "ola").await.unwrap(), None, "{scope:?}");
        }
        assert_eq!(
            list(&db, ours.id).await.unwrap(),
            [Access {
                user: "ola".into(),
                role: EventRole::Editor
            }]
        );

        assert!(take(&db, ours.id, "ola").await.unwrap());
        assert!(!take(&db, ours.id, "ola").await.unwrap());
        assert!(!has_any(&db, "ola").await.unwrap());
        // It goes with the event.
        event::delete(&db, other.id).await.unwrap();
        assert!(!has_any(&db, "kari@example.com").await.unwrap());
    }

    #[sqlx::test]
    async fn the_database_refuses_a_name_the_form_would(db: PgPool) {
        let event = event::tests::event(&db).await;
        for user in [String::new(), "ola\n".into(), "o".repeat(MAX_USER_LEN + 1)] {
            let result = give(&db, event.id, &user, EventRole::Viewer).await;
            assert!(result.is_err(), "{user:?}");
        }
    }
}
