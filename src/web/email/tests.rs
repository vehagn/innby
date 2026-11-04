use chrono::Duration;
use sqlx::PgPool;

use super::*;
use crate::web::testing::{Site, body, en, escape, location};

async fn queued(site: &Site) -> Vec<(uuid::Uuid, Kind)> {
    sqlx::query!(
        r#"select rsvp_id, kind as "kind: Kind" from outbox where sent_at is null order by id"#
    )
    .fetch_all(&site.db)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.rsvp_id, row.kind))
    .collect()
}

/// Mail that `/lost` queues after it has responded, once it's there.
async fn queued_soon(site: &Site) -> Vec<(uuid::Uuid, Kind)> {
    for _ in 0..100 {
        let queued = queued(site).await;
        if !queued.is_empty() {
            return queued;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("nothing was queued");
}

fn confirm_token(site: &Site, guest: &Rsvp, email: &str) -> String {
    site.config
        .link_key
        .email_link(guest.id, email, Utc::now() + Duration::hours(48))
}

#[sqlx::test]
async fn add_confirm_and_remove(db: PgPool) {
    let site = Site::new(db);
    let (guest, link) = site.invite("Kari", 1).await;

    let response = site
        .post(
            &format!("/r/{link}/email?lang=en"),
            "email=+Kari%40Example.com&consent=yes",
        )
        .await;
    assert_eq!(
        location(&response),
        format!("/r/{link}?lang=en&notice=email_sent")
    );
    let added = site.find(&guest).await.unwrap();
    assert_eq!(added.email.as_deref(), Some("kari@example.com"));
    assert_eq!(
        (added.email_status, added.lang),
        (EmailStatus::Unconfirmed, Lang::EnGb)
    );
    assert_eq!(queued(&site).await, [(guest.id, Kind::ConfirmEmail)]);

    let html = body(
        site.get(&format!("/r/{link}?lang=en&notice=email_sent"))
            .await,
    )
    .await;
    assert!(html.contains(&escape(&en().email_sent_notice)));
    assert!(html.contains("<strong>kari@example.com</strong>"));

    // GET only shows a button.
    let token = confirm_token(&site, &added, "kari@example.com");
    let response = site.get(&format!("/e/{token}?lang=en")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        site.find(&guest).await.unwrap().email_status,
        EmailStatus::Unconfirmed
    );

    let response = site.post(&format!("/e/{token}?lang=en"), "").await;
    assert_eq!(
        location(&response),
        format!("/r/{link}?lang=en&notice=email_confirmed")
    );
    assert_eq!(
        site.find(&guest).await.unwrap().email_status,
        EmailStatus::Confirmed
    );
    assert_eq!(
        queued(&site).await,
        [(guest.id, Kind::ConfirmEmail), (guest.id, Kind::RsvpLink)]
    );
    // A second click finds nothing left to confirm.
    let response = site.post(&format!("/e/{token}"), "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = site.post(&format!("/r/{link}/email/remove"), "").await;
    assert_eq!(
        location(&response),
        format!("/r/{link}?lang=nb&notice=email_removed")
    );
    let removed = site.find(&guest).await.unwrap();
    assert_eq!(
        (removed.email, removed.email_status),
        (None, EmailStatus::None)
    );
    assert!(queued(&site).await.is_empty());
}

#[sqlx::test]
async fn requires_consent_and_a_valid_address(db: PgPool) {
    let site = Site::new(db);
    let (guest, link) = site.invite("Kari", 1).await;
    let uri = format!("/r/{link}/email");

    let response = site.post(&uri, "email=kari%40example.com").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("Kryss av i boksen"));
    let response = site.post(&uri, "email=kari&consent=yes").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(site.find(&guest).await.unwrap().email, None);
    assert!(queued(&site).await.is_empty());
}

#[sqlx::test]
async fn address_in_use_warns_its_owner_and_looks_the_same(db: PgPool) {
    let site = Site::new(db);
    let (owner, owner_link) = site.invite("Kari", 1).await;
    let (other, other_link) = site.invite("Ola", 1).await;
    let form = "email=kari%40example.com&consent=yes";
    site.post(&format!("/r/{owner_link}/email"), form).await;
    rsvp::confirm_email(&site.db, owner.id, "kari@example.com")
        .await
        .unwrap();
    sqlx::query!("delete from outbox")
        .execute(&site.db)
        .await
        .unwrap();

    let response = site.post(&format!("/r/{other_link}/email"), form).await;
    let page = location(&response).to_owned();
    assert_eq!(page, format!("/r/{other_link}?lang=nb&notice=email_sent"));
    // The page shows the address waiting for confirmation, as for a new address.
    assert!(
        body(site.get(&page).await)
            .await
            .contains("<strong>kari@example.com</strong>")
    );
    let stored = site.find(&other).await.unwrap();
    assert_eq!(
        (stored.email.as_deref(), stored.email_status),
        (Some("kari@example.com"), EmailStatus::Unconfirmed)
    );
    assert_eq!(queued(&site).await, [(owner.id, Kind::EmailInUse)]);
    // Only the owner's confirmation counts.
    let token = confirm_token(&site, &stored, "kari@example.com");
    let response = site.post(&format!("/e/{token}"), "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn unconfirmed_address_can_be_added_twice(db: PgPool) {
    let site = Site::new(db);
    let (first, first_link) = site.invite("Kari", 1).await;
    let (second, second_link) = site.invite("Ola", 1).await;
    let form = "email=kari%40example.com&consent=yes";
    site.post(&format!("/r/{first_link}/email"), form).await;
    site.post(&format!("/r/{second_link}/email"), form).await;
    assert_eq!(
        queued(&site).await,
        [
            (first.id, Kind::ConfirmEmail),
            (second.id, Kind::ConfirmEmail)
        ]
    );

    let first_token = confirm_token(&site, &first, "kari@example.com");
    let second_token = confirm_token(&site, &second, "kari@example.com");
    let response = site.post(&format!("/e/{second_token}"), "").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = site.post(&format!("/e/{first_token}"), "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn changing_the_address_cancels_old_mail(db: PgPool) {
    let site = Site::new(db);
    let (guest, link) = site.invite("Kari", 1).await;
    let uri = format!("/r/{link}/email");
    site.post(&uri, "email=kari%40example.com&consent=yes")
        .await;
    let old_token = confirm_token(&site, &guest, "kari@example.com");
    site.post(&uri, "email=kari%40example.org&consent=yes")
        .await;

    assert_eq!(queued(&site).await, [(guest.id, Kind::ConfirmEmail)]);
    // The old confirmation link was for the old address.
    let response = site.post(&format!("/e/{old_token}"), "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        site.find(&guest).await.unwrap().email.as_deref(),
        Some("kari@example.org")
    );
}

#[sqlx::test]
async fn expired_or_forged_confirmations_fail(db: PgPool) {
    let site = Site::new(db);
    let (guest, link) = site.invite("Kari", 1).await;
    site.post(
        &format!("/r/{link}/email"),
        "email=kari%40example.com&consent=yes",
    )
    .await;

    let expired = site.config.link_key.email_link(
        guest.id,
        "kari@example.com",
        Utc::now() - Duration::seconds(1),
    );
    let forged = confirm_token(&site, &guest, "someone@example.com");
    for token in [expired.as_str(), &forged, "garbage"] {
        let response = site.get(&format!("/e/{token}?lang=en")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{token}");
        assert!(body(response).await.contains("has expired"));
        assert_eq!(
            site.post(&format!("/e/{token}"), "").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        site.find(&guest).await.unwrap().email_status,
        EmailStatus::Unconfirmed
    );
}

#[sqlx::test]
async fn lost_link_only_goes_to_confirmed_addresses(db: PgPool) {
    let site = Site::new(db);
    let (guest, link) = site.invite("Kari", 1).await;
    site.post(
        &format!("/r/{link}/email"),
        "email=kari%40example.com&consent=yes",
    )
    .await;
    rsvp::confirm_email(&site.db, guest.id, "kari@example.com")
        .await
        .unwrap();
    sqlx::query!("delete from outbox")
        .execute(&site.db)
        .await
        .unwrap();

    let answers = [
        site.post("/lost?lang=en", "email=nobody%40example.com")
            .await,
        site.post("/lost?lang=en", "email=KARI%40example.com").await,
    ];
    let [unknown, known] = answers;
    assert_eq!(unknown.status(), known.status());
    assert_eq!(body(unknown).await, body(known).await);
    assert_eq!(queued_soon(&site).await, [(guest.id, Kind::RsvpLink)]);

    // Asking again while the mail waits doesn't queue another.
    mail_link(&site.db, "kari@example.com").await.unwrap();
    assert_eq!(queued(&site).await.len(), 1);
}

#[sqlx::test]
async fn mail_requests_are_rate_limited(db: PgPool) {
    let site = Site::new(db);
    for _ in 0..5 {
        site.post("/lost", "email=someone%40example.com").await;
    }
    let response = site.post("/lost", "email=someone%40example.com").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}
