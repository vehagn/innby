use sqlx::PgPool;

use super::*;
use crate::web::testing::{Site, body, en, escape, form_post, location};

#[sqlx::test]
async fn guest_answers_through_their_link(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari & Ola", 2).await;

    let response = site.get(&format!("/r/{link}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    let html = body(response).await;
    assert!(html.contains("Hei, Kari &#38; Ola!"), "{html}");
    assert!(html.contains("Du er invitert!"));
    assert!(html.contains("Ikke svart ennå"));
    assert!(html.contains(r#"<option value="2">2 personer</option>"#));

    let response = site
        .post(
            &format!("/r/{link}?lang=en"),
            "party_size=2&response=attending",
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("/r/{link}?lang=en&notice=saved").as_str()
    );

    let answered = site.find(&invite).await.unwrap();
    assert_eq!(answered.response, rsvp::Response::Attending);
    assert_eq!((answered.party_size, answered.lang), (2, Lang::EnGb));
    assert_eq!(answered.privacy_version.as_deref(), Some(PRIVACY_VERSION));

    let html = body(site.get(&format!("/r/{link}?lang=en&notice=saved")).await).await;
    assert!(html.contains(&escape(&en().saved)));
    assert!(html.contains(&en().answer(rsvp::Response::Attending, 2)));
    assert!(!html.contains("You're invited!"));

    site.post(&format!("/r/{link}"), "party_size=1&response=declined")
        .await;
    let declined = site.find(&invite).await.unwrap();
    assert_eq!(declined.response, rsvp::Response::Declined);
}

#[sqlx::test]
async fn bad_links_all_look_the_same(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;
    let (_, other) = site.invite("Ola", 1).await;
    let forged = format!("{}{}", &link[..23], &other[23..]);
    rsvp::revoke_link(&site.db, invite.id).await.unwrap();

    for bad in [link.as_str(), &forged, "garbage", "a.b"] {
        let response = site.get(&format!("/r/{bad}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
        assert!(body(response).await.contains("Denne lenken virker ikke"));
    }
    let response = site
        .post(&format!("/r/{link}"), "party_size=1&response=attending")
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        site.find(&invite).await.unwrap().response,
        rsvp::Response::Pending
    );

    let fresh = site.config.link_key.link(Purpose::Rsvp, invite.id, 1);
    assert_eq!(
        site.get(&format!("/r/{fresh}")).await.status(),
        StatusCode::OK
    );
}

#[sqlx::test]
async fn party_size_is_limited_to_the_invitation(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 2).await;
    for size in ["3", "0"] {
        let response = site
            .post(
                &format!("/r/{link}"),
                &format!("party_size={size}&response=attending"),
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("Velg et gyldig antall."));
    }
    assert_eq!(
        site.find(&invite).await.unwrap().response,
        rsvp::Response::Pending
    );
}

#[sqlx::test]
async fn cross_site_posts_are_rejected(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;
    let response = site
        .send(form_post(
            &format!("/r/{link}/delete"),
            "confirm=yes",
            "cross-site",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(site.find(&invite).await.is_some());
}

#[sqlx::test]
async fn answers_close_at_the_deadline(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;
    site.close_answers().await;

    let html = body(site.get(&format!("/r/{link}")).await).await;
    assert!(html.contains("Svarfristen har gått ut."));
    assert!(!html.contains(r#"value="attending""#));

    let response = site
        .post(&format!("/r/{link}"), "party_size=1&response=attending")
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        site.find(&invite).await.unwrap().response,
        rsvp::Response::Pending
    );
}

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
async fn guest_list_shows_names_as_each_guest_chose(db: PgPool) {
    let site = Site::new(db);
    let (_, kari) = site.invite("Kari Nordmann", 1).await;
    let (_, ola) = site.invite("Ola Hansen", 1).await;
    let (_, per) = site.invite("Per Berg", 1).await;
    site.invite("Anne Lie", 1).await;
    let guests = |link: &str| format!("/r/{link}/guests?lang=en");

    // Coming defaults to the full name.
    let html = body(site.get(&format!("/r/{kari}?lang=en")).await).await;
    assert!(html.contains(&format!(
        r#"<option value="full" selected>{} (Kari Nordmann)</option>"#,
        en().display_full
    )));
    // The page a link opens has no other guest's name: link previews fetch it.
    assert!(!html.contains("Ola") && !html.contains("Anne"), "{html}");
    assert!(html.contains(&format!(
        r#"<a class="button secondary" href="/r/{kari}/guests?lang=en" rel="nofollow">"#
    )));
    assert!(
        body(site.get(&guests(&kari)).await)
            .await
            .contains("<li>Ola H.</li>")
    );
    for (link, answer) in [
        (&kari, "attending&name_display=full"),
        (&ola, "attending&name_display=hidden"),
        (&per, "declined&name_display=full"),
    ] {
        let form = format!("party_size=1&response={answer}");
        site.post(&format!("/r/{link}"), &form).await;
    }

    let response = site.get(&guests(&kari)).await;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(response).await;
    let t = en();
    assert!(html.contains(&format!(
        r#"<li>Kari Nordmann <span class="muted">{}</span></li>"#,
        t.you
    )));
    assert!(html.contains("<li>Anne L.</li>"));
    // Ola is coming but hidden, and Per declined.
    assert!(html.contains(&format!(r#"<li class="muted">{}</li>"#, t.more_guests(1))));
    assert!(!html.contains("Ola H") && !html.contains("Per B"));
    assert!(html.contains(&format!(
        "{}</p>",
        crate::i18n::bold(&t.listed_as, "name", "Kari Nordmann")
    )));
    assert!(html.contains(&format!(
        r#"href="/r/{kari}?lang=en">{}"#,
        t.back_to_your_page
    )));

    let response = site
        .post(
            &format!("/r/{kari}/listing?lang=en"),
            "name_display=partial",
        )
        .await;
    let back = location(&response).to_owned();
    assert_eq!(back, format!("/r/{kari}/guests?lang=en&notice=saved"));
    assert!(
        body(site.get(&back).await)
            .await
            .contains(&escape(&t.saved))
    );
    let html = body(site.get(&guests(&ola)).await).await;
    assert!(html.contains("<li>Kari N.</li>"));
    assert!(html.contains(&escape(&t.listed_hidden)));
    // Answering again keeps the choice the form shows.
    let html = body(site.get(&format!("/r/{ola}?lang=en")).await).await;
    assert!(html.contains(r#"<option value="hidden" selected>"#));
    let html = body(site.get(&guests(&per)).await).await;
    assert!(html.contains(&escape(&t.listed_declined)));
    assert!(!html.contains(&t.show_instead));

    let response = site.get("/r/garbage/guests").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn guest_list_shows_who_brings_others(db: PgPool) {
    let site = Site::new(db);
    let (_, kari) = site.invite("Kari Nordmann", 3).await;
    let (_, ola) = site.invite("Ola Hansen", 2).await;
    let (_, per) = site.invite("Per Berg", 3).await;
    for (link, party_size) in [(&kari, 3), (&ola, 2), (&per, 1)] {
        let form = format!("party_size={party_size}&response=attending&name_display=full");
        site.post(&format!("/r/{link}"), &form).await;
    }
    site.invite("Anne Lie", 3).await;

    let html = body(site.get(&format!("/r/{kari}/guests?lang=en")).await).await;
    assert!(html.contains(&format!(
        r#"<li>Kari Nordmann <span class="plus">+2</span> <span class="muted">{}</span></li>"#,
        en().you
    )));
    assert!(html.contains(r#"<li>Ola Hansen <span class="plus">+1</span></li>"#));
    // Coming alone, or not answered yet: no one to count.
    assert!(html.contains("<li>Per Berg</li>"));
    assert!(html.contains("<li>Anne L.</li>"));
    // The totals count hidden guests too.
    site.invite("Liv Moe", 1).await;
    site.post(&format!("/r/{ola}/listing?lang=en"), "name_display=hidden")
        .await;
    let html = body(site.get(&format!("/r/{kari}/guests?lang=en")).await).await;
    let t = en();
    assert!(html.contains(&format!(
        "<h3>{} · {}</h3>",
        t.guest_list_coming,
        t.people_count(6)
    )));
    assert!(html.contains(&format!(
        "<h3>{} · {}</h3>",
        t.guest_list_pending,
        t.invitation_count(2)
    )));
    // Ola is hidden now, and so is the one Ola brings.
    assert!(html.contains(&format!(r#"<li class="muted">{}</li>"#, t.more_guests(2))));
}

#[sqlx::test]
async fn chinese_pages_and_names(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("王小明", 1).await;
    let html = body(site.get(&format!("/r/{link}?lang=zh-hans")).await).await;
    assert!(html.contains(r#"<html lang="zh-Hans" "#));
    assert!(html.contains("王小明，你好！"));
    assert!(html.contains("你的回复：<strong>尚未回复</strong>"));
    // Before answering, other guests see only the family name.
    assert!(html.contains(r#"<option value="partial">部分姓名（王**）</option>"#));
    let html = body(site.get(&format!("/r/{link}/guests?lang=zh-hant")).await).await;
    assert!(html.contains("其他賓客看到的你：<strong>王**</strong>"));
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
