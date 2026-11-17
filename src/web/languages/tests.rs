use axum::{
    body::Body,
    http::{Request, header},
};
use sqlx::PgPool;

use crate::{
    db::{event, languages, rsvp},
    i18n::{Lang, LangSet, Offer},
    web::testing::{Site, body},
};

/// The page at `uri`, for a browser that asks for `accept`.
async fn page(site: &Site, uri: &str, accept: &str) -> String {
    let request = Request::get(uri).header(header::ACCEPT_LANGUAGE, accept);
    body(site.send(request.body(Body::empty()).unwrap()).await).await
}

fn set(langs: &[Lang]) -> LangSet {
    langs.iter().copied().collect()
}

/// Whether the page's language menu offers this language.
fn offers(html: &str, code: &str) -> bool {
    switch_link(html, code).is_some()
}

fn switch_link<'a>(html: &'a str, code: &str) -> Option<&'a str> {
    html.split(r#"<a href=""#).find_map(|link| {
        let (href, attributes) = link.split_once('"')?;
        (href == format!("?lang={code}") && attributes.starts_with(" hreflang=")).then_some(href)
    })
}

#[sqlx::test]
async fn the_site_offers_only_the_languages_its_admin_picked(db: PgPool) {
    let site = Site::new(db);
    // Every language there's a file for, until an admin picks.
    let html = page(&site, "/", "zh-CN").await;
    assert!(html.contains(r#"<html lang="zh-Hans" "#), "{html}");
    assert!(offers(&html, "en-gb") && offers(&html, "nn"), "{html}");

    // Bokmål and plain English, and Bokmål for anyone else.
    let picked = Offer::new(set(&[Lang::NB, Lang::EN]), Lang::NB);
    languages::save_site(&site.db, picked).await.unwrap();
    let html = page(&site, "/", "zh-CN").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    assert!(offers(&html, "en"), "{html}");
    for gone in ["nn", "en-gb", "en-us", "zh-hans", "zh-hant"] {
        assert!(!offers(&html, gone), "{gone}: {html}");
    }
    // A language that isn't offered isn't to be had by asking for it, either way.
    let html = page(&site, "/privacy?lang=zh-hans", "zh-CN").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    // A British browser gets the plain English that is offered.
    let html = page(&site, "/", "en-GB,en;q=0.9").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    assert!(offers(&html, "nb") && !offers(&html, "en"), "{html}");
}

#[sqlx::test]
async fn an_event_narrows_the_languages_of_its_own_pages(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari", 1).await;
    let guest = format!("/r/{link}");
    // The event, written in Bokmål, is offered in English too, and nothing else.
    let party = site.event().await;
    let mut details = party.details.clone();
    details.offered = set(&[Lang::EN]);
    event::update(&site.db, party.id, &details).await.unwrap();

    // Its own language is the default, and always offered.
    let html = page(&site, &guest, "zh-CN,zh;q=0.9").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    assert!(offers(&html, "en"), "{html}");
    for gone in ["nn", "en-gb", "zh-hans"] {
        assert!(!offers(&html, gone), "{gone}: {html}");
    }
    let switch = switch_link(&html, "en").unwrap();
    let html = page(&site, &format!("{guest}{switch}"), "nb").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    let html = page(&site, &format!("{guest}/guests?lang=nn"), "nn").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    let switch = switch_link(&html, "en").unwrap();
    let html = page(&site, &format!("{guest}/guests{switch}"), "nb").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    let html = page(&site, &format!("{guest}/you"), "en-GB").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    let switch = switch_link(&html, "nb").unwrap();
    let html = page(&site, &format!("{guest}/you{switch}"), "en").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    // The rest of the site still has every language.
    let html = page(&site, "/", "zh-CN").await;
    assert!(html.contains(r#"<html lang="zh-Hans" "#), "{html}");

    // An answer is kept in the language of the page it was given on, not the one
    // asked for, so the guest's mail is in a language the event has.
    site.post(
        &format!("{guest}?lang=zh-hans"),
        "party_size=1&response=attending",
    )
    .await;
    let answered = rsvp::find(&site.db, invite.id).await.unwrap().unwrap();
    assert_eq!(answered.lang, Lang::NB);

    // The site's languages bound the event's: with English gone from the site,
    // the event is left with its own.
    let only = Offer::new(set(&[Lang::NB, Lang::NN]), Lang::NN);
    languages::save_site(&site.db, only).await.unwrap();
    let html = page(&site, &guest, "en").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    assert!(!offers(&html, "en") && !offers(&html, "nn"), "{html}");
}

#[sqlx::test]
async fn a_group_sign_up_page_is_in_its_events_languages(db: PgPool) {
    let site = Site::new(db);
    let party = site.event().await;
    let mut details = party.details.clone();
    details.offered = set(&[Lang::NB, Lang::EN]);
    event::update(&site.db, party.id, &details).await.unwrap();
    let club = crate::db::group::create(&site.db, party.id, "Climbing club", 2)
        .await
        .unwrap();
    let join = format!("/join/{}", club.link(&site.config.link_key));

    let html = page(&site, &join, "zh-TW, en;q=0.5").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    assert!(offers(&html, "nb") && !offers(&html, "zh-hant"), "{html}");
    let switch = switch_link(&html, "nb").unwrap();
    let html = page(&site, &format!("{join}{switch}"), "en").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    let html = page(&site, &format!("{join}/guests?lang=zh-hant"), "en").await;
    assert!(html.contains(r#"<html lang="en" "#), "{html}");
    let switch = switch_link(&html, "nb").unwrap();
    let html = page(&site, &format!("{join}/guests{switch}"), "en").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
    let html = page(&site, &join, "zh-TW").await;
    assert!(html.contains(r#"<html lang="nb" "#), "{html}");
}

#[sqlx::test]
async fn concurrent_pages_keep_their_own_language_offers(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari", 1).await;
    let guest = format!("/r/{link}");
    let party = site.event().await;
    let mut details = party.details.clone();
    details.offered = set(&[Lang::EN]);
    event::update(&site.db, party.id, &details).await.unwrap();
    details.offered = set(&[Lang::NN]);
    let other = event::create(&site.db, &details).await.unwrap();
    let club = crate::db::group::create(&site.db, other.id, "Climbing club", 2)
        .await
        .unwrap();
    let join = format!("/join/{}", club.link(&site.config.link_key));

    let (guest, group, front) = tokio::join!(
        page(&site, &guest, "en"),
        page(&site, &join, "en"),
        page(&site, "/", "zh-TW"),
    );
    assert!(guest.contains(r#"<html lang="en" "#), "{guest}");
    assert!(offers(&guest, "nb") && !offers(&guest, "nn"), "{guest}");
    assert!(group.contains(r#"<html lang="nb" "#), "{group}");
    assert!(offers(&group, "nn") && !offers(&group, "en"), "{group}");
    assert!(front.contains(r#"<html lang="zh-Hant" "#), "{front}");
    assert!(offers(&front, "en") && offers(&front, "nn"), "{front}");
}

#[sqlx::test]
async fn invalid_links_use_the_sites_language_offer(db: PgPool) {
    let site = Site::new(db);
    for uri in ["/r/nope", "/join/nope"] {
        let html = page(&site, uri, "zh-TW").await;
        assert!(html.contains(r#"<html lang="zh-Hant" "#), "{html}");
    }
    let picked = Offer::new(set(&[Lang::NN, Lang::EN]), Lang::NN);
    languages::save_site(&site.db, picked).await.unwrap();
    for uri in ["/r/nope", "/join/nope"] {
        let html = page(&site, uri, "zh-TW").await;
        assert!(html.contains(r#"<html lang="nn" "#), "{html}");
        assert!(offers(&html, "en") && !offers(&html, "zh-hant"), "{html}");
        let switch = switch_link(&html, "en").unwrap();
        let html = page(&site, &format!("{uri}{switch}"), "nn").await;
        assert!(html.contains(r#"<html lang="en" "#), "{html}");
    }
}
