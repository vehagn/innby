//! What a guest sees of the others, and the host's headcount: the guest list of an
//! event (who's coming and who hasn't answered), and how many are coming, haven't
//! answered, or aren't coming.

use std::collections::HashMap;

use sqlx::PgExecutor;
use uuid::Uuid;

use super::{NameDisplay, Response};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Headcount {
    /// Invitations and sign-ups that answered "attending".
    pub attending_parties: i64,
    /// The sum of their party sizes: the number of people coming.
    pub attending_people: i64,
    pub declined: i64,
    pub pending: i64,
}

/// What the guest list needs about one guest.
pub struct Listing {
    pub id: Uuid,
    pub name: String,
    pub response: Response,
    pub party_size: i32,
    pub name_display: Option<NameDisplay>,
}

/// Everyone invited to the event who hasn't declined, for its guest list.
pub async fn guest_list(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Vec<Listing>> {
    sqlx::query_as!(
        Listing,
        r#"
        select id, name, response as "response: Response", party_size,
            name_display as "name_display: NameDisplay"
        from rsvp
        where event_id = $1 and response <> 'declined'
        "#,
        event_id,
    )
    .fetch_all(db)
    .await
}

pub async fn headcount(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Headcount> {
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
        where event_id = $1
        "#,
        event_id,
    )
    .fetch_one(db)
    .await
}

/// Every event's headcount at once, for the list of events: one query rather than one
/// an event. An event nobody is invited to yet isn't in it.
pub async fn headcounts(db: impl PgExecutor<'_>) -> sqlx::Result<HashMap<Uuid, Headcount>> {
    let rows = sqlx::query!(
        r#"
        select
            event_id,
            count(*) filter (where response = 'attending') as "attending_parties!",
            coalesce(sum(party_size) filter (where response = 'attending'), 0)
                as "attending_people!",
            count(*) filter (where response = 'declined') as "declined!",
            count(*) filter (where response = 'pending') as "pending!"
        from rsvp
        group by event_id
        "#,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let headcount = Headcount {
                attending_parties: row.attending_parties,
                attending_people: row.attending_people,
                declined: row.declined,
                pending: row.pending,
            };
            (row.event_id, headcount)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::{
        db::rsvp::{Reply, create_invite, respond, set_name_display, tests::sign_up},
        i18n::Lang,
        web::PRIVACY_VERSION,
    };

    #[sqlx::test]
    async fn guest_list_leaves_out_declined_guests(pool: PgPool) -> sqlx::Result<()> {
        let kari = create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Kari Nordmann",
            1,
        )
        .await?;
        let ola = create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Ola",
            1,
        )
        .await?;
        respond(
            &pool,
            ola.id,
            &Reply {
                response: Response::Declined,
                party_size: 1,
                name_display: Some(NameDisplay::Full),
                choice: None,
            },
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?;
        assert!(set_name_display(&pool, kari.id, NameDisplay::Hidden).await?);

        let [listed] = guest_list(&pool, crate::db::event::tests::event(&pool).await.id)
            .await?
            .try_into()
            .ok()
            .unwrap();
        assert_eq!(
            (listed.id, listed.name_display),
            (kari.id, Some(NameDisplay::Hidden))
        );
        // Answering without a choice keeps the one made before.
        let answered = respond(
            &pool,
            kari.id,
            &Reply {
                response: Response::Attending,
                party_size: 1,
                name_display: None,
                choice: None,
            },
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?
        .unwrap();
        assert_eq!(answered.listed(), NameDisplay::Hidden);
        Ok(())
    }

    #[sqlx::test]
    async fn headcount_sums_attending_parties(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(
            headcount(&pool, crate::db::event::tests::event(&pool).await.id).await?,
            Headcount {
                attending_parties: 0,
                attending_people: 0,
                declined: 0,
                pending: 0,
            }
        );

        let family = create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Familien Hansen",
            4,
        )
        .await?;
        respond(
            &pool,
            family.id,
            &Reply {
                response: Response::Attending,
                party_size: 3,
                name_display: None,
                choice: None,
            },
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?;
        let busy = create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Ola",
            1,
        )
        .await?;
        respond(
            &pool,
            busy.id,
            &Reply {
                response: Response::Declined,
                party_size: 1,
                name_display: None,
                choice: None,
            },
            Lang::NB,
            PRIVACY_VERSION,
        )
        .await?;
        create_invite(
            &pool,
            crate::db::event::tests::event(&pool).await.id,
            "Kari",
            1,
        )
        .await?;
        sign_up(&pool, "Per", 2).await?;

        let event = crate::db::event::tests::event(&pool).await.id;
        let expected = Headcount {
            attending_parties: 2,
            attending_people: 5,
            declined: 1,
            pending: 1,
        };
        assert_eq!(headcount(&pool, event).await?, expected);
        // All the events' at once give the same, and leave out events with no guests.
        let empty = crate::db::event::create(&pool, &crate::db::event::tests::details()).await?;
        let all = headcounts(&pool).await?;
        assert_eq!(all.get(&event), Some(&expected));
        assert!(!all.contains_key(&empty.id));
        Ok(())
    }
}
