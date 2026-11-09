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
