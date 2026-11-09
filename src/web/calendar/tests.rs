use axum::http::{StatusCode, header};
use sqlx::PgPool;

use crate::{
    db::rsvp,
    i18n::Lang,
    web::testing::{Site, body, en, escape},
};

#[sqlx::test]
async fn a_guest_who_is_coming_can_add_the_event_to_their_calendar(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari Nordmann", 2).await;
    let t = en();
    let page = format!("/r/{link}?lang=en");
    let calendar = format!("/r/{link}/event.ics?lang=en");

    // Not before they say they're coming.
    let html = body(site.get(&page).await).await;
    assert!(!html.contains(&escape(&t.add_to_calendar)));

    rsvp::respond(
        &site.db,
        invite.id,
        &rsvp::Reply {
            response: rsvp::Response::Attending,
            party_size: 2,
            name_display: None,
            choice: None,
        },
        Lang::EnGb,
        crate::web::PRIVACY_VERSION,
    )
    .await
    .unwrap();
    let html = body(site.get(&page).await).await;
    assert!(html.contains(&escape(&t.add_to_calendar)), "{html}");
    assert!(html.contains(&format!(r#"href="{calendar}""#)), "{html}");

    let response = site.get(&calendar).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "text/calendar; charset=utf-8"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let file = body(response).await;
    assert!(file.contains("\r\nSUMMARY:Christmas party\r\n"), "{file}");
    // Nothing of the guest's.
    assert!(!file.contains("Kari") && !file.contains(&link), "{file}");

    // A link that doesn't work gets nothing.
    let response = site.get("/r/wrong-code/event.ics").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
