use super::*;
use crate::{
    db::{
        outbox::{self, Kind},
        rsvp::{self, EmailStatus},
    },
    i18n::Lang,
    web::PRIVACY_VERSION,
};

fn event_start() -> DateTime<Utc> {
    "2026-12-12T18:00:00Z".parse().unwrap()
}

async fn the_event(db: &PgPool) -> uuid::Uuid {
    crate::db::event::tests::event(db).await.id
}

/// Moves every recorded consent `hours` into the past.
async fn backdate_consent(db: &PgPool, hours: i32) {
    sqlx::query!(
        "update rsvp set email_consent_at = email_consent_at - make_interval(hours => $1)",
        hours,
    )
    .execute(db)
    .await
    .unwrap();
}

#[sqlx::test]
async fn clears_addresses_left_unconfirmed(db: PgPool) {
    let stale = rsvp::create_invite(&db, the_event(&db).await, "Kari", 1)
        .await
        .unwrap();
    rsvp::set_email(&db, stale.id, "kari@example.com", Lang::Nb, PRIVACY_VERSION)
        .await
        .unwrap();
    outbox::enqueue(&db, stale.id, Kind::ConfirmEmail)
        .await
        .unwrap();
    backdate_consent(&db, 49).await;

    let fresh = rsvp::create_invite(&db, the_event(&db).await, "Ola", 1)
        .await
        .unwrap();
    rsvp::set_email(&db, fresh.id, "ola@example.com", Lang::Nb, PRIVACY_VERSION)
        .await
        .unwrap();
    let confirmed = rsvp::create_invite(&db, the_event(&db).await, "Per", 1)
        .await
        .unwrap();
    rsvp::set_email(
        &db,
        confirmed.id,
        "per@example.com",
        Lang::Nb,
        PRIVACY_VERSION,
    )
    .await
    .unwrap();
    rsvp::confirm_email(&db, confirmed.id, "per@example.com")
        .await
        .unwrap();
    backdate_consent(&db, 240).await;
    // Ola's consent is now 10 days old too, so give it back a fresh one.
    sqlx::query!(
        "update rsvp set email_consent_at = now() where id = $1",
        fresh.id
    )
    .execute(&db)
    .await
    .unwrap();

    let report = run(&db, Utc::now()).await.unwrap();
    assert_eq!(report.emails_cleared, 1);

    let stale = rsvp::find(&db, stale.id).await.unwrap().unwrap();
    assert_eq!((stale.email, stale.email_status), (None, EmailStatus::None));
    let unsent: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from outbox"#)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(unsent, 0);
    assert!(
        rsvp::find(&db, fresh.id)
            .await
            .unwrap()
            .unwrap()
            .email
            .is_some()
    );
    assert!(
        rsvp::find(&db, confirmed.id)
            .await
            .unwrap()
            .unwrap()
            .email
            .is_some()
    );
}

#[sqlx::test]
async fn deletes_old_sent_and_abandoned_mail(db: PgPool) {
    let guest = rsvp::create_invite(&db, the_event(&db).await, "Kari", 1)
        .await
        .unwrap();
    for kind in [Kind::RsvpLink, Kind::Reminder, Kind::ConfirmEmail] {
        outbox::enqueue(&db, guest.id, kind).await.unwrap();
    }
    sqlx::query!(
        r#"
        update outbox set
            sent_at = case when kind = 'rsvp_link' then now() - interval '8 days' end,
            attempts = case when kind = 'reminder' then $1 else 0 end,
            created_at = now() - interval '8 days'
        "#,
        outbox::MAX_ATTEMPTS,
    )
    .execute(&db)
    .await
    .unwrap();

    let report = run(&db, Utc::now()).await.unwrap();
    assert_eq!(report.mails_deleted, 2);
    let left: Vec<Kind> = sqlx::query_scalar!(r#"select kind as "kind: Kind" from outbox"#)
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(left, [Kind::ConfirmEmail]);
}

#[sqlx::test]
async fn deletes_everything_30_days_after_the_event(db: PgPool) {
    let guest = rsvp::tests::sign_up(&db, "Kari", 1).await.unwrap();
    outbox::enqueue(&db, guest.id, Kind::RsvpLink)
        .await
        .unwrap();

    let almost = event_start() + KEEP_AFTER_EVENT - Duration::seconds(1);
    assert_eq!(run(&db, almost).await.unwrap().rsvps_deleted, 0);

    let due = event_start() + KEEP_AFTER_EVENT;
    assert_eq!(run(&db, due).await.unwrap().rsvps_deleted, 1);
    assert!(crate::db::event::list(&db).await.unwrap().is_empty());
    assert!(rsvp::find(&db, guest.id).await.unwrap().is_none());
    let mails: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from outbox"#)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(mails, 0);
}
