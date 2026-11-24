use axum::http::{StatusCode, header};
use sqlx::PgPool;

use crate::{
    db::rsvp,
    i18n::Lang,
    web::testing::{Site, body, en, escape, location},
};

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

#[sqlx::test]
async fn a_guest_gives_the_rest_of_their_answer_here(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari & Ola", 2).await;
    let event = site.event().await;
    let mut details = event.details;
    details.question.set(Lang::NB, "Hva tar du med?");
    details.choices.set(Lang::NB, "Salat\nDessert");
    details.open_questions.set(Lang::NB, "Hvem kjører?");
    crate::db::event::update(&site.db, event.id, &details)
        .await
        .unwrap();
    let page = format!("/r/{link}/you?lang=en");
    let html = body(site.get(&page).await).await;
    assert!(html.contains(&escape(&en().details_heading)), "{html}");
    assert!(
        html.contains(r#"<option value="2">2 people</option>"#),
        "{html}"
    );
    assert!(html.contains("Hva tar du med?"), "{html}");
    assert!(html.contains(r#"name="answer_0""#), "{html}");
    assert!(html.contains(&format!(r#"action="/r/{link}/details?lang=en""#)));

    let form = "party_size=2&name_display=partial&choice=1&answer_0=Vi+kj%C3%B8rer";
    let response = site.post(&format!("/r/{link}/details?lang=en"), form).await;
    assert_eq!(
        location(&response),
        format!("/r/{link}/you?lang=en&notice=saved")
    );
    let saved = site.find(&invite).await.unwrap();
    // Whether they're coming is the reply page's; this leaves it be.
    assert_eq!(saved.response, rsvp::Response::Pending);
    assert!(saved.responded_at.is_none());
    assert_eq!(saved.party_size, 2);
    assert_eq!(saved.name_display, Some(rsvp::NameDisplay::Partial));
    assert_eq!(saved.choice, Some(1));
    assert_eq!(saved.answers, ["Vi kjører"]);
    let html = body(site.get(&format!("{page}&notice=saved")).await).await;
    assert!(html.contains(&escape(&en().saved)), "{html}");
    assert!(html.contains("Vi kjører</textarea>"), "{html}");

    // Saying they're coming keeps it all.
    site.post(&format!("/r/{link}"), "response=attending").await;
    let coming = site.find(&invite).await.unwrap();
    assert_eq!((coming.party_size, coming.choice), (2, Some(1)));
}

#[sqlx::test]
async fn the_full_name_left_as_preselected_is_no_choice(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari Nordmann", 1).await;
    let details = format!("/r/{link}/details?lang=en");
    // Saved before answering, the name stays first name and initial until she's coming.
    site.post(&details, "party_size=1&name_display=full").await;
    let saved = site.find(&invite).await.unwrap();
    assert_eq!(saved.name_display, None);
    assert_eq!(saved.listed(), rsvp::NameDisplay::Partial);
    // Chosen otherwise, it's hers; and the full name again once she has chosen.
    site.post(&details, "party_size=1&name_display=hidden")
        .await;
    site.post(&details, "party_size=1&name_display=full").await;
    let saved = site.find(&invite).await.unwrap();
    assert_eq!(saved.name_display, Some(rsvp::NameDisplay::Full));
}

#[sqlx::test]
async fn refused_details_are_shown_again_to_fix(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 2).await;
    let event = site.event().await;
    let mut details = event.details;
    details.question.set(Lang::NB, "Hva tar du med?");
    details.choices.set(Lang::NB, "Salat\nDessert");
    details.open_questions.set(Lang::NB, "Hvem kjører?");
    crate::db::event::update(&site.db, event.id, &details)
        .await
        .unwrap();
    let post = format!("/r/{link}/details?lang=en");
    let long = "x".repeat(crate::web::views::MAX_ANSWER + 1);
    for (form, message) in [
        (
            "party_size=3&answer_0=Kept".to_owned(),
            &en().invalid_party_size,
        ),
        (
            "party_size=0&answer_0=Kept".to_owned(),
            &en().invalid_party_size,
        ),
        (
            "party_size=1&choice=2&answer_0=Kept".to_owned(),
            &en().invalid_choice,
        ),
        (
            "party_size=1&choice=salad&answer_0=Kept".to_owned(),
            &en().invalid_choice,
        ),
        (
            format!("party_size=1&answer_0=Kept{long}"),
            &en().invalid_answer,
        ),
    ] {
        let response = site.post(&post, &form).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{form}"
        );
        let html = body(response).await;
        assert!(html.contains(&escape(message)), "{form}: {html}");
        // On its own tab still, with what was typed.
        assert!(html.contains(r#"aria-current="page""#), "{html}");
        assert!(html.contains("Kept"), "{form}: {html}");
    }
    let unchanged = site.find(&invite).await.unwrap();
    assert_eq!((unchanged.party_size, unchanged.choice), (1, None));
    assert!(unchanged.answers.is_empty());

    // After the deadline, nothing changes.
    site.close_answers().await;
    let html = body(site.get(&format!("/r/{link}/you?lang=en")).await).await;
    assert!(html.contains(&escape(&en().answers_closed)), "{html}");
    assert!(!html.contains(r#"name="party_size""#), "{html}");
    let response = site.post(&post, "party_size=2").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(site.find(&invite).await.unwrap().party_size, 1);
}
