use jiff::SignedDuration;

use super::*;
use crate::db::{access, event};

const HASH: &str = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaA";

#[sqlx::test]
async fn users_come_and_go_with_their_roles(db: PgPool) {
    assert!(!any(&db).await.unwrap());
    assert_eq!(
        create(&db, "kari", Some("editor"), Some(HASH), None)
            .await
            .unwrap(),
        Ok(())
    );
    assert_eq!(
        create(&db, "kari", None, Some(HASH), None).await.unwrap(),
        Err(Taken::Name)
    );
    assert!(any(&db).await.unwrap());
    assert!(set_role(&db, "kari", None).await.unwrap());
    let credential = find(&db, "kari").await.unwrap().unwrap().credential();
    assert_eq!(
        change_password(&db, credential, HASH).await.unwrap(),
        Some(AccountCredential {
            id: credential.id,
            version: credential.version + 1,
        })
    );
    let kari = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!((kari.role, kari.version), (None, 1));

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
async fn a_recreated_name_gets_a_new_account_identity(db: PgPool) {
    create(
        &db,
        "kari",
        Some("editor"),
        Some(HASH),
        Some("kari@example.com"),
    )
    .await
    .unwrap()
    .unwrap();
    let predecessor = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!(predecessor.version, 0);

    assert!(delete(&db, "kari").await.unwrap());
    create(
        &db,
        "kari",
        Some("editor"),
        Some(HASH),
        Some("kari@example.com"),
    )
    .await
    .unwrap()
    .unwrap();
    let replacement = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!(replacement.version, predecessor.version);
    assert_ne!(replacement.id, predecessor.id);
    assert_eq!(
        change_password(&db, predecessor.credential(), HASH)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        set_email_for_account(&db, predecessor.credential(), Some("other@example.com"))
            .await
            .unwrap(),
        Ok(false)
    );
    assert_eq!(
        consume_password_reset(
            &db,
            predecessor.credential(),
            "kari@example.com",
            Timestamp::now() + SignedDuration::from_hours(1),
            HASH,
        )
        .await
        .unwrap(),
        None
    );
}

#[sqlx::test]
async fn only_one_password_write_from_a_verified_snapshot_wins(db: PgPool) {
    create(&db, "kari", None, Some(HASH), Some("kari@example.com"))
        .await
        .unwrap()
        .unwrap();
    let expected = find(&db, "kari").await.unwrap().unwrap().credential();
    let changed_hash = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$Y2hhbmdlZA";
    let reset_hash = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$cmVzZXQ";

    let (changed, reset) = tokio::join!(
        change_password(&db, expected, changed_hash),
        consume_password_reset(
            &db,
            expected,
            "kari@example.com",
            Timestamp::now() + SignedDuration::from_hours(1),
            reset_hash,
        ),
    );
    let changed = changed.unwrap();
    let reset = reset.unwrap();
    assert_eq!(
        usize::from(changed.is_some()) + usize::from(reset.is_some()),
        1
    );

    let winner = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!(winner.version, expected.version + 1);
    assert!(
        winner.password_hash.as_deref() == Some(changed_hash)
            || winner.password_hash.as_deref() == Some(reset_hash)
    );
}

#[sqlx::test]
async fn an_admins_new_password_stands_whatever_the_old_one_was(db: PgPool) {
    create(&db, "kari", None, Some(HASH), None)
        .await
        .unwrap()
        .unwrap();
    let kari = find(&db, "kari").await.unwrap().unwrap();
    let hers = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$Y2hhbmdlZA";
    let admins = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$cmVzZXQ";

    // Kari changes hers just before the admin's lands: the admin's replaces it, and
    // logs her out of the session she got with hers.
    let changed = change_password(&db, kari.credential(), hers).await.unwrap();
    assert_eq!(changed.map(|credential| credential.version), Some(1));
    let set = set_password(&db, kari.id, admins).await.unwrap();
    assert_eq!(set.map(|credential| credential.version), Some(2));
    let now = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!(now.password_hash.as_deref(), Some(admins));

    // Not for another account that has taken the name since.
    assert!(delete(&db, "kari").await.unwrap());
    create(&db, "kari", None, Some(HASH), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set_password(&db, kari.id, admins).await.unwrap(), None);
    let replacement = find(&db, "kari").await.unwrap().unwrap();
    assert_eq!(replacement.password_hash.as_deref(), Some(HASH));
}

#[sqlx::test]
async fn a_reset_write_rechecks_email_and_expiry(db: PgPool) {
    create(&db, "kari", None, Some(HASH), Some("kari@example.com"))
        .await
        .unwrap()
        .unwrap();
    let expected = find(&db, "kari").await.unwrap().unwrap().credential();
    assert_eq!(
        set_email(&db, "kari", Some("new@example.com"))
            .await
            .unwrap(),
        Ok(true)
    );
    assert_eq!(
        consume_password_reset(
            &db,
            expected,
            "kari@example.com",
            Timestamp::now() + SignedDuration::from_hours(1),
            HASH,
        )
        .await
        .unwrap(),
        None
    );
    let current = find(&db, "kari").await.unwrap().unwrap().credential();
    assert_eq!(
        consume_password_reset(
            &db,
            current,
            "new@example.com",
            Timestamp::now() - SignedDuration::from_secs(1),
            HASH,
        )
        .await
        .unwrap(),
        None
    );
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
            create(&db, name, role, Some(hash), None).await.is_err(),
            "{name} {role:?} {hash}"
        );
    }
}

#[sqlx::test]
async fn an_address_is_one_users_and_finds_them(db: PgPool) {
    create(&db, "kari", None, None, Some("kari@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        create(&db, "ola", None, Some(HASH), Some("kari@example.com"))
            .await
            .unwrap(),
        Err(Taken::Email)
    );
    create(&db, "ola", None, Some(HASH), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        set_email(&db, "ola", Some("kari@example.com"))
            .await
            .unwrap(),
        Err(Taken::Email)
    );
    for typed in ["kari", "Kari@Example.com"] {
        let found = find_by_name_or_email(&db, typed).await.unwrap().unwrap();
        assert_eq!(found.name, "kari", "{typed}");
        assert_eq!(found.password_hash, None);
    }
    assert!(
        find_by_name_or_email(&db, "nobody")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        create(&db, "per", None, None, Some("Per@Example.com"))
            .await
            .is_err(),
        "addresses are lower case"
    );
}
