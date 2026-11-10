use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use sqlx::PgPool;

use super::*;
use crate::{
    db::{
        event::{self, tests::details},
        rsvp,
    },
    i18n::{Texts, filled},
    link::Purpose,
    testing::set_cookies,
    web::testing::{Site, body, en, escape, form_post, location},
};

fn with_cookie(mut request: Request<Body>, cookie: &str) -> Request<Body> {
    let cookie = HeaderValue::from_str(cookie).unwrap();
    request.headers_mut().insert(header::COOKIE, cookie);
    request
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    with_cookie(Request::get(uri).body(Body::empty()).unwrap(), cookie)
}

/// The name of the cookie that remembers this link: its invitation's.
fn cookie_name(link: &str) -> String {
    let link: IdLink = link.parse().unwrap();
    format!("__Host-innby-{}", link.id.simple())
}

/// An invitation to a second event, the summer party, and its link.
async fn summer_invite(site: &Site, name: &str) -> (event::Event, String) {
    let summer = event::create(
        &site.db,
        &event::Details {
            title: Texts::from([(Lang::NB, "Sommerfest")]),
            ..details()
        },
    )
    .await
    .unwrap();
    let invite = rsvp::create_invite(&site.db, summer.id, name, 1)
        .await
        .unwrap();
    let link = site
        .config
        .link_key
        .link(Purpose::Rsvp, invite.id, invite.link_nonce);
    (summer, link)
}

#[sqlx::test]
async fn the_front_page_leads_back_to_each_remembered_invitation(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 2).await;

    // Nothing is set until the guest asks.
    let response = site.get(&format!("/r/{link}?lang=en")).await;
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert!(
        body(response)
            .await
            .contains(&escape(&en().remember_device))
    );

    let response = site.post(&format!("/r/{link}/remember?lang=en"), "").await;
    assert_eq!(
        location(&response),
        format!("/r/{link}/you?lang=en&notice=remembered")
    );
    let party_cookie = cookie_name(&link);
    let [set] = set_cookies(&response)[..] else {
        panic!("one cookie")
    };
    assert!(
        set.starts_with(&format!("{party_cookie}={link}; Path=/; Max-Age=")),
        "{set}"
    );
    assert!(set.ends_with("; HttpOnly; SameSite=Lax; Secure"), "{set}");

    // The same device remembers an invitation to another event too.
    let (_, summer_link) = summer_invite(&site, "Kari N").await;
    let summer_cookie = cookie_name(&summer_link);
    let summer_invitation = summer_link.parse::<IdLink>().unwrap().id;
    let cookie = format!("other=1; {party_cookie}={link}; {summer_cookie}={summer_link}");

    let response = site.send(get("/?lang=en", &cookie)).await;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(set_cookies(&response).is_empty());
    let html = body(response).await;
    // The soonest first: the summer party is in December too, but it was made later.
    for (title, link) in [("Christmas party", &link), ("Sommerfest", &summer_link)] {
        assert!(html.contains(&format!(">{title}</h3>")), "{html}");
        assert!(html.contains(&format!(r#"href="/r/{link}?lang=en""#)));
    }
    let welcome = escape(&filled(&en().welcome_back, "name", "Kari"));
    assert!(html.contains(&welcome), "{html}");
    // One invitation to each event is nothing to remark on.
    assert!(!html.contains(&escape(&en().remembered_twice)), "{html}");
    assert!(html.contains(&format!(r#"name="invitation" value="{summer_invitation}""#)));
    // Without cookies, the front page is the same for everyone.
    assert!(!body(site.get("/?lang=en").await).await.contains(&welcome));

    // The guest's page knows: the box to be remembered is ticked.
    let html = body(site.send(get(&format!("/r/{link}?lang=en"), &cookie)).await).await;
    assert!(
        html.contains(r#"name="remember" value="yes" checked>"#),
        "{html}"
    );
    let response = site.post(&format!("/r/{link}/forget?lang=en"), "").await;
    assert_eq!(
        location(&response),
        format!("/r/{link}/you?lang=en&notice=forgotten")
    );
    assert!(set_cookies(&response)[0].starts_with(&format!("{party_cookie}=; Path=/; Max-Age=0;")));

    // The front page forgets one invitation at a time.
    let response = site
        .post(
            "/forget?lang=en",
            &format!("invitation={summer_invitation}"),
        )
        .await;
    assert_eq!(location(&response), "/?lang=en");
    assert!(
        set_cookies(&response)[0].starts_with(&format!("{summer_cookie}=; Path=/; Max-Age=0;"))
    );

    // A link that stopped working is forgotten.
    rsvp::revoke_link(&site.db, invite.id).await.unwrap();
    let response = site.send(get("/?lang=en", &cookie)).await;
    assert_eq!(
        set_cookies(&response),
        [format!(
            "{party_cookie}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax; Secure"
        )]
    );
    let html = body(response).await;
    assert!(!html.contains("Christmas party") && html.contains("Sommerfest"));
}

#[sqlx::test]
async fn the_answer_remembers_the_device_only_when_its_box_is_ticked(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari", 1).await;
    let cookie = format!("{}={link}", cookie_name(&link));
    let page = format!("/r/{link}?lang=en");
    let checkbox = r#"<input type="checkbox" name="remember" value="yes""#;
    let answer = |form: &str, cookie: &str| {
        let request = form_post(&format!("/r/{link}?lang=en"), form, "same-origin");
        site.send(with_cookie(request, cookie))
    };

    // Unticked until the guest ticks it: a box ticked for them wouldn't be asking.
    let html = body(site.get(&page).await).await;
    assert!(html.contains(&format!("{checkbox}>")), "{html}");
    assert!(!html.contains("/remember?"), "{html}");
    let response = answer("response=attending&party_size=1", "").await;
    assert_eq!(location(&response), format!("{page}&notice=saved"));
    assert!(set_cookies(&response).is_empty());

    // Ticked, the answer remembers the device, and the box shows it.
    let response = answer("response=attending&party_size=1&remember=yes", "").await;
    let [set] = set_cookies(&response)[..] else {
        panic!("one cookie")
    };
    assert!(
        set.starts_with(&format!("{cookie}; Path=/; Max-Age=")),
        "{set}"
    );
    let html = body(site.send(get(&page, &cookie)).await).await;
    assert!(html.contains(&format!("{checkbox} checked>")), "{html}");

    // Unticked again, it forgets it.
    let response = answer("response=declined&party_size=1", &cookie).await;
    assert!(set_cookies(&response)[0].contains("Max-Age=0"));

    // Once answers close there's no answer to tick it on, but the guest's details
    // have a button for it.
    site.close_answers().await;
    let html = body(site.get(&page).await).await;
    assert!(!html.contains(checkbox), "{html}");
    let html = body(site.get(&format!("/r/{link}/you?lang=en")).await).await;
    assert!(html.contains("/remember?"), "{html}");
}

#[sqlx::test]
async fn deleting_the_reply_forgets_the_device(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari", 1).await;
    let request = form_post(&format!("/r/{link}/delete"), "confirm=yes", "same-origin");
    let cookie = format!("{}={link}", cookie_name(&link));
    let response = site.send(with_cookie(request, &cookie)).await;
    assert!(set_cookies(&response)[0].contains("Max-Age=0"));
}

#[test]
fn reads_only_its_own_cookies() {
    let headers = |cookie: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        headers
    };
    let id = Uuid::nil().simple().to_string();
    let ours = format!("__Host-innby-{id}");
    assert_eq!(
        links(&headers(&format!("__Host-innby=a.b; {ours}=c.d")), true),
        [(ours.as_str(), "c.d")]
    );
    assert_eq!(
        links(&headers("x=1;innby-e=a.b"), false),
        [("innby-e", "a.b")]
    );
    assert!(links(&headers("innby-e=a.b"), true).is_empty());
    assert!(links(&headers("innbyx=a.b"), false).is_empty());
    assert!(links(&headers("innby-e="), false).is_empty());
    assert!(links(&HeaderMap::new(), false).is_empty());
}

#[sqlx::test]
async fn the_front_page_reads_only_so_many_remembered_links(db: PgPool) {
    let site = Site::new(db);
    // Well-formed links that lead nowhere: each one read is a lookup, and is dropped.
    let forged = "AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAA";
    let prefix = prefix(site.config.is_https());
    let cookie: Vec<String> = (0..40).map(|n| format!("{prefix}{n}={forged}")).collect();
    let response = site.send(get("/?lang=en", &cookie.join("; "))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(set_cookies(&response).len(), MOST_REMEMBERED);
}

#[sqlx::test]
async fn the_front_page_lists_events_to_come_before_those_that_are_over(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari", 1).await;
    // A party two days ago, over by now, and one that started an hour ago, still on.
    let past = |title: &str, hours: i64| {
        let starts_at = chrono::Utc::now().with_timezone(&chrono_tz::Europe::Oslo)
            - chrono::Duration::hours(hours);
        event::Details {
            title: Texts::from([(Lang::NB, title)]),
            starts_at,
            ..details()
        }
    };
    let mut cookie = format!("{}={link}", cookie_name(&link));
    for (title, hours) in [("Over", 48), ("Under way", 1)] {
        let event = event::create(&site.db, &past(title, hours)).await.unwrap();
        let invite = rsvp::create_invite(&site.db, event.id, "Kari", 1)
            .await
            .unwrap();
        let link = site
            .config
            .link_key
            .link(Purpose::Rsvp, invite.id, invite.link_nonce);
        cookie.push_str(&format!("; {}={link}", cookie_name(&link)));
    }
    let html = body(site.send(get("/?lang=en", &cookie)).await).await;
    let at = |text: &str| {
        html.find(text)
            .unwrap_or_else(|| panic!("{text} in {html}"))
    };
    // Under way and to come, the soonest first; then the past, under a heading of its own.
    assert!(
        at(">Under way</h3>") < at(">Christmas party</h3>"),
        "{html}"
    );
    assert!(
        at(">Christmas party</h3>") < at(&en().past_invitations),
        "{html}"
    );
    assert!(at(&en().past_invitations) < at(">Over</h3>"), "{html}");
    // Forgetting one says it's that one alone.
    assert!(html.contains(&escape(&en().forget_here)));
}
