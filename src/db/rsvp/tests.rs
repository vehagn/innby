use sqlx::PgPool;

use super::*;
use crate::{db::group, web::PRIVACY_VERSION};

/// Signs `name` up, in English, through a new group link for up to two people.
pub async fn sign_up(pool: &PgPool, name: &str, party_size: i32) -> sqlx::Result<Rsvp> {
    let group = group::create(
        pool,
        crate::db::event::tests::event(pool).await.id,
        "Climbing club",
        2,
    )
    .await?;
    let sign_up = SignUp {
        group: &group,
        name,
        party_size,
        name_display: NameDisplay::Full,
        lang: Lang::EN,
        choice: None,
        token: None,
    };
    create_open(pool, &sign_up, PRIVACY_VERSION)
        .await
        .map(|(made, _)| made)
}
const CHECK_VIOLATION: &str = "23514";

fn is_check_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|err| err.code())
        .is_some_and(|code| code == CHECK_VIOLATION)
}

#[sqlx::test]
async fn invitation_starts_pending(pool: PgPool) -> sqlx::Result<()> {
    let rsvp = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari & Ola",
        2,
    )
    .await?;
    assert!(rsvp.is_invitation());
    assert_eq!(rsvp.response, Response::Pending);
    assert_eq!((rsvp.party_size, rsvp.max_party_size), (1, 2));
    assert_eq!((rsvp.link_nonce, rsvp.lang), (0, Lang::NB));
    assert_eq!(rsvp.email_status, EmailStatus::None);
    assert_eq!(rsvp.id.get_version_num(), 7);
    assert!(rsvp.responded_at.is_none());
    Ok(())
}

#[sqlx::test]
async fn a_sign_up_belongs_to_its_groups_event(pool: PgPool) -> sqlx::Result<()> {
    let rsvp = sign_up(&pool, "Per", 1).await?;
    let other = crate::db::event::create(&pool, &crate::db::event::tests::details()).await?;
    // Moving the sign-up to another event, its group left behind, is refused.
    let moved = sqlx::query("update rsvp set event_id = $1 where id = $2")
        .bind(other.id)
        .bind(rsvp.id)
        .execute(&pool)
        .await;
    assert!(moved.is_err(), "{moved:?}");
    Ok(())
}

#[sqlx::test]
async fn open_rsvp_is_attending_immediately(pool: PgPool) -> sqlx::Result<()> {
    let rsvp = sign_up(&pool, "Per", 2).await?;
    assert!(!rsvp.is_invitation());
    assert!(rsvp.group_id.is_some());
    assert_eq!(rsvp.name_display, Some(NameDisplay::Full));
    assert_eq!(rsvp.response, Response::Attending);
    assert_eq!((rsvp.party_size, rsvp.lang), (2, Lang::EN));
    assert_eq!(rsvp.privacy_version.as_deref(), Some(PRIVACY_VERSION));
    assert!(rsvp.responded_at.is_some());
    Ok(())
}

#[sqlx::test]
async fn respond_records_the_answer(pool: PgPool) -> sqlx::Result<()> {
    let invite = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari & Ola",
        2,
    )
    .await?;
    let answered = respond(
        &pool,
        invite.id,
        &Reply {
            response: Response::Attending,
            party_size: 2,
            name_display: None,
            choice: None,
        },
        Lang::EN,
        PRIVACY_VERSION,
    )
    .await?
    .expect("rsvp exists");
    assert_eq!(answered.response, Response::Attending);
    assert_eq!((answered.party_size, answered.lang), (2, Lang::EN));
    assert!(answered.responded_at.is_some());

    let reread = find(&pool, invite.id).await?.expect("rsvp exists");
    assert_eq!(reread.response, Response::Attending);
    Ok(())
}

#[sqlx::test]
async fn respond_rejects_party_larger_than_invited(pool: PgPool) -> sqlx::Result<()> {
    let invite = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari & Ola",
        2,
    )
    .await?;
    let err = respond(
        &pool,
        invite.id,
        &Reply {
            response: Response::Attending,
            party_size: 3,
            name_display: None,
            choice: None,
        },
        Lang::NB,
        PRIVACY_VERSION,
    )
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
        respond(
            &pool,
            id,
            &Reply {
                response: Response::Declined,
                party_size: 1,
                name_display: None,
                choice: None
            },
            Lang::NB,
            PRIVACY_VERSION
        )
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
    let invite = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari",
        1,
    )
    .await?;
    assert!(rename(&pool, invite.id, "Kari Nordmann").await?);
    assert_eq!(find(&pool, invite.id).await?.unwrap().name, "Kari Nordmann");

    let err = rename(&pool, invite.id, "").await.unwrap_err();
    assert!(is_check_violation(&err), "{err}");
    Ok(())
}

#[sqlx::test]
async fn email_lifecycle(pool: PgPool) -> sqlx::Result<()> {
    let rsvp = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari",
        1,
    )
    .await?;
    assert!(
        set_email(
            &pool,
            rsvp.id,
            "kari@example.com",
            Lang::EN,
            PRIVACY_VERSION
        )
        .await?
    );
    let stored = find(&pool, rsvp.id).await?.unwrap();
    assert_eq!(stored.email.as_deref(), Some("kari@example.com"));
    assert_eq!(stored.email_status, EmailStatus::Unconfirmed);
    assert_eq!(stored.lang, Lang::EN);
    assert!(stored.email_consent_at.is_some());
    assert!(
        find_by_confirmed_email(&pool, rsvp.event_id, "kari@example.com")
            .await?
            .is_none()
    );

    // Until it's confirmed, another RSVP may hold the address too.
    let other = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Ola",
        1,
    )
    .await?;
    assert!(
        set_email(
            &pool,
            other.id,
            "kari@example.com",
            Lang::NB,
            PRIVACY_VERSION
        )
        .await?
    );

    assert!(!confirm_email(&pool, rsvp.id, "other@example.com").await?);
    assert!(confirm_email(&pool, rsvp.id, "kari@example.com").await?);
    assert!(!confirm_email(&pool, rsvp.id, "kari@example.com").await?);
    let confirmed = find_by_confirmed_email(&pool, rsvp.event_id, "kari@example.com")
        .await?
        .unwrap();
    assert_eq!(confirmed.id, rsvp.id);
    // The first to confirm keeps it, and the database refuses a second.
    assert!(!confirm_email(&pool, other.id, "kari@example.com").await?);
    let err = sqlx::query("update rsvp set email_status = 'confirmed' where id = $1")
        .bind(other.id)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        err.as_database_error()
            .is_some_and(|err| err.is_unique_violation())
    );

    assert!(remove_email(&pool, rsvp.id).await?);
    assert!(!remove_email(&pool, rsvp.id).await?);
    let removed = find(&pool, rsvp.id).await?.unwrap();
    assert_eq!(
        (removed.email, removed.email_status),
        (None, EmailStatus::None)
    );
    assert!(removed.email_consent_at.is_none());
    Ok(())
}

#[sqlx::test]
async fn shrinking_an_invitation_shrinks_the_party(pool: PgPool) -> sqlx::Result<()> {
    let invite = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Familien Hansen",
        4,
    )
    .await?;
    respond(
        &pool,
        invite.id,
        &Reply {
            response: Response::Attending,
            party_size: 4,
            name_display: None,
            choice: None,
        },
        Lang::NB,
        PRIVACY_VERSION,
    )
    .await?;
    assert!(update_invite(&pool, invite.id, "Hansen", 2).await?);
    let rsvp = find(&pool, invite.id).await?.unwrap();
    assert_eq!(
        (rsvp.name.as_str(), rsvp.party_size, rsvp.max_party_size),
        ("Hansen", 2, 2)
    );
    assert!(!update_invite(&pool, Uuid::max(), "Nobody", 1).await?);
    Ok(())
}

#[sqlx::test]
async fn revoke_link_bumps_nonce(pool: PgPool) -> sqlx::Result<()> {
    let invite = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari",
        1,
    )
    .await?;
    assert_eq!(revoke_link(&pool, invite.id).await?, Some(1));
    assert_eq!(revoke_link(&pool, invite.id).await?, Some(2));
    Ok(())
}

#[sqlx::test]
async fn delete_removes_pending_mail(pool: PgPool) -> sqlx::Result<()> {
    let rsvp = sign_up(&pool, "Per", 1).await?;
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
    let rsvp = create_invite(
        &pool,
        crate::db::event::tests::event(&pool).await.id,
        "Kari",
        1,
    )
    .await?;
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
