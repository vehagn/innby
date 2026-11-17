use axum::http::{StatusCode, header};
use sqlx::PgPool;

use crate::web::testing::{Site, body, en, escape};

#[sqlx::test]
async fn guest_can_rename(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;

    let response = site
        .post(&format!("/r/{link}/name"), "name=++Kari++Nordmann+")
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(site.find(&invite).await.unwrap().name, "Kari Nordmann");

    let response = site.post(&format!("/r/{link}/name"), "name=+++").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(site.find(&invite).await.unwrap().name, "Kari Nordmann");
}

#[sqlx::test]
async fn a_guest_changes_their_name_once_an_hour(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;
    let (other, other_link) = site.invite("Ola", 1).await;
    let site = &site;
    let rename = |link: &str, name: &str| {
        let (link, form) = (format!("/r/{link}/name?lang=en"), format!("name={name}"));
        async move { site.post(&link, &form).await }
    };

    let html = body(site.get(&format!("/r/{link}/you?lang=en")).await).await;
    assert!(html.contains(&escape(&en().name_once_an_hour)), "{html}");
    let response = rename(&link, "Kari+Nordmann").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    // The same name again changes nothing, so it's no change.
    let response = rename(&link, "Kari+Nordmann").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    // Another within the hour is refused, and the name stays.
    let response = rename(&link, "Kari+Hansen").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let html = body(response).await;
    assert!(html.contains(&escape(&en().name_error_busy)), "{html}");
    assert_eq!(site.find(&invite).await.unwrap().name, "Kari Nordmann");

    // Each guest has an hour of their own.
    let response = rename(&other_link, "Ola+Hansen").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(site.find(&other).await.unwrap().name, "Ola Hansen");
}

#[sqlx::test]
async fn guest_can_export_their_data(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;

    let response = site.get(&format!("/r/{link}/export")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    let json: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(json["id"], invite.id.to_string());
    assert_eq!(json["name"], "Kari");
    assert_eq!(json["response"], "pending");
    assert_eq!(json["email"], serde_json::Value::Null);
    assert_eq!(json["name_display"], serde_json::Value::Null);
}

#[sqlx::test]
async fn guest_can_delete_everything(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;

    let response = site.post(&format!("/r/{link}/delete"), "").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(site.find(&invite).await.is_some());

    let response = site
        .post(&format!("/r/{link}/delete?lang=en"), "confirm=yes")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body(response).await.contains(&escape(&en().deleted_title)));
    assert!(site.find(&invite).await.is_none());
    assert_eq!(
        site.get(&format!("/r/{link}")).await.status(),
        StatusCode::NOT_FOUND
    );
}
