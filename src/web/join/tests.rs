use sqlx::PgPool;

use super::*;
use crate::web::testing::{Site, body, en, escape, location};

async fn all(site: &Site) -> Vec<rsvp::Rsvp> {
    rsvp::list(&site.db, crate::db::event::tests::event(&site.db).await.id)
        .await
        .unwrap()
}

/// A group for up to two people per sign-up, and its link.
async fn climbing_club(site: &Site) -> (Group, String) {
    let group = group::create(
        &site.db,
        crate::db::event::tests::event(&site.db).await.id,
        "Climbing club",
        2,
    )
    .await
    .unwrap();
    let link = site
        .config
        .link_key
        .link(Purpose::Group, group.id, group.link_nonce);
    (group, link)
}

#[sqlx::test]
async fn only_a_groups_current_link_works(db: PgPool) {
    let site = Site::new(db);
    let (group, link) = climbing_club(&site).await;
    let rsvp_link = site.config.link_key.link(Purpose::Rsvp, group.id, 0);
    for bad in ["wrong-code", rsvp_link.as_str()] {
        let response = site.get(&format!("/join/{bad}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
        assert!(body(response).await.contains("Denne lenken virker ikke"));
        let response = site
            .post(&format!("/join/{bad}"), "name=Per&party_size=1")
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    group::revoke_link(&site.db, group.id).await.unwrap();
    for page in [format!("/join/{link}"), format!("/join/{link}/guests")] {
        let response = site.get(&page).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{page}");
    }
    assert!(all(&site).await.is_empty());
}

#[sqlx::test]
async fn guest_joins_and_gets_their_link(db: PgPool) {
    let site = Site::new(db);
    let (group, link) = climbing_club(&site).await;
    rsvp::create_invite(
        &site.db,
        crate::db::event::tests::event(&site.db).await.id,
        "Kari Nordmann",
        1,
    )
    .await
    .unwrap();
    let response = site.get(&format!("/join/{link}?lang=en")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "no-store"
    );
    let html = body(response).await;
    let t = en();
    assert!(html.contains(&crate::i18n::bold(&t.join_group, "group", "Climbing club")));
    assert!(html.contains(&escape(&t.join_heading)));
    assert!(html.contains(r#"name="website""#));
    assert!(html.contains(&format!(
        r#"<option value="full" selected>{}</option>"#,
        t.display_full
    )));
    // No names on the page the link opens, which link previews fetch: they're on the
    // guest list, with the invited guest who hasn't answered as "Kari N.".
    assert!(!html.contains("Kari"), "{html}");
    let list = format!("/join/{link}/guests?lang=en");
    assert!(html.contains(&format!(r#"href="{list}" rel="nofollow""#)));
    let html = body(site.get(&list).await).await;
    assert!(html.contains("<li>Kari N.</li>"), "{html}");
    assert!(html.contains(&format!(
        r#"href="/join/{link}?lang=en">{}"#,
        t.back_to_sign_up
    )));
    assert!(!html.contains(&t.listed_as));

    let response = site
        .post(
            &format!("/join/{link}?lang=en"),
            "name=+Per+Hansen+&party_size=2&name_display=partial&website=",
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let target = location(&response).to_owned();
    assert!(target.starts_with("/r/") && target.ends_with("?lang=en&new"));

    let guest = all(&site)
        .await
        .into_iter()
        .find(|guest| !guest.is_invitation())
        .unwrap();
    assert_eq!(guest.name, "Per Hansen");
    assert_eq!(guest.group_id, Some(group.id));
    assert_eq!(guest.name_display, Some(NameDisplay::Partial));
    assert_eq!(guest.response, rsvp::Response::Attending);
    assert_eq!((guest.party_size, guest.max_party_size), (2, 2));
    assert_eq!(guest.lang, Lang::EnGb);

    let html = body(site.get(&target).await).await;
    assert!(html.contains(&escape(&t.save_link_title)));
    let path = target.split('?').next().unwrap();
    assert!(html.contains(&format!(r#"value="https://fest.example.com{path}""#)));
    assert!(html.contains(&t.answer(rsvp::Response::Attending, 2)));
    let html = body(site.get(&format!("{path}/guests?lang=en")).await).await;
    assert!(html.contains(&format!(
        r#"Per H. <span class="plus">+1</span> <span class="muted">{}</span>"#,
        t.you
    )));
}

#[sqlx::test]
async fn rejects_bad_input_and_bots(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = climbing_club(&site).await;
    let join = format!("/join/{link}");
    for form in [
        "name=Per&party_size=3",
        "name=+&party_size=1",
        "name=Per&party_size=1&name_display=everyone",
    ] {
        let response = site.post(&join, form).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{form}"
        );
    }
    let response = site
        .post(&join, "name=Bot&party_size=1&website=http%3A%2F%2Fspam")
        .await;
    assert_eq!(location(&response), "/");
    assert!(all(&site).await.is_empty());
}

#[sqlx::test]
async fn limits_sign_ups_per_client(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = climbing_club(&site).await;
    let join = format!("/join/{link}");
    for n in 0..10 {
        let response = site
            .post(
                &join,
                &format!("name=Guest+{}&party_size=1", char::from(b'A' + n)),
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    let response = site.post(&join, "name=One+more&party_size=1").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(body(response).await.contains("For mange forsøk"));
    assert_eq!(all(&site).await.len(), 10);
}

#[sqlx::test]
async fn closes_at_the_deadline(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = climbing_club(&site).await;
    site.close_answers().await;
    let join = format!("/join/{link}");
    assert!(
        body(site.get(&join).await)
            .await
            .contains("Svarfristen har gått ut.")
    );
    let response = site.post(&join, "name=Per&party_size=1").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(all(&site).await.is_empty());
}
