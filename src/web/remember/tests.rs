use axum::{
    body::Body,
    http::{Request, header},
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

/// An invitation to a second event, the summer party, and its link.
async fn summer_invite(site: &Site, name: &str) -> (event::Event, String) {
    let summer = event::create(
        &site.db,
        &event::Details {
            title: Texts {
                nb: "Sommerfest".into(),
                ..Texts::default()
            },
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
    let party = site.event().await;

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
        format!("/r/{link}?lang=en&notice=remembered")
    );
    let party_cookie = format!("__Host-innby-{}", party.id.simple());
    let [set] = set_cookies(&response)[..] else {
        panic!("one cookie")
    };
    assert!(
        set.starts_with(&format!("{party_cookie}={link}; Path=/; Max-Age=")),
        "{set}"
    );
    assert!(set.ends_with("; HttpOnly; SameSite=Lax; Secure"), "{set}");

    // The same device remembers an invitation to another event too.
    let (summer, summer_link) = summer_invite(&site, "Kari N").await;
    let summer_cookie = format!("__Host-innby-{}", summer.id.simple());
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
    assert!(html.contains(&format!(r#"name="event" value="{}""#, summer.id)));
    // Without cookies, the front page is the same for everyone.
    assert!(!body(site.get("/?lang=en").await).await.contains(&welcome));

    // The guest's page knows, and offers to forget.
    let html = body(site.send(get(&format!("/r/{link}?lang=en"), &cookie)).await).await;
    assert!(html.contains(&escape(&en().remembered_text)));
    let response = site.post(&format!("/r/{link}/forget?lang=en"), "").await;
    assert_eq!(
        location(&response),
        format!("/r/{link}?lang=en&notice=forgotten")
    );
    assert!(set_cookies(&response)[0].starts_with(&format!("{party_cookie}=; Path=/; Max-Age=0;")));

    // The front page forgets one invitation at a time.
    let response = site
        .post("/forget?lang=en", &format!("event={}", summer.id))
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
async fn deleting_the_reply_forgets_the_device(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari", 1).await;
    let party = site.event().await;
    let request = form_post(&format!("/r/{link}/delete"), "confirm=yes", "same-origin");
    let cookie = format!("__Host-innby-{}={link}", party.id.simple());
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
