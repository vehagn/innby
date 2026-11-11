use axum::http::StatusCode;
use sqlx::PgPool;

use crate::{
    db::board::{self, BoardState, Writer},
    web::testing::{Site, body, en, escape, location},
};

async fn set(site: &Site, state: BoardState) {
    let event = site.event().await;
    board::set_state(&site.db, event.id, state).await.unwrap();
}

#[sqlx::test]
async fn the_board_is_for_guests_only_when_its_hosts_say(db: PgPool) {
    let site = Site::new(db);
    let (_, link) = site.invite("Kari Nordmann", 1).await;
    let board = format!("/r/{link}/board?lang=en");
    for state in [BoardState::Off, BoardState::Hidden] {
        set(&site, state).await;
        let html = body(site.get(&format!("/r/{link}?lang=en")).await).await;
        assert!(!html.contains(&format!("/r/{link}/board")), "{html}");
        let response = site.get(&board).await;
        assert_eq!(location(&response), format!("/r/{link}?lang=en"));
    }

    // Hosts post; guests read and vote, but don't post.
    set(&site, BoardState::Hosts).await;
    let event = site.event().await.id;
    board::create(&site.db, event, None, Writer::Host("admin"), "Welcome!")
        .await
        .unwrap();
    let html = body(site.get(&board).await).await;
    assert!(html.contains("Welcome!"), "{html}");
    // Signed as a host, never by name.
    assert!(html.contains(&en().board_a_host), "{html}");
    assert!(!html.contains("admin"), "{html}");
    assert!(html.contains(&escape(&en().board_hint_hosts)), "{html}");
    assert!(!html.contains(r#"name="body""#), "{html}");
    let response = site.post(&board, "body=Hello").await;
    assert_eq!(location(&response), format!("/r/{link}?lang=en"));
    assert_eq!(board::count(&site.db, event).await.unwrap(), 1);
}

#[sqlx::test]
async fn guests_post_reply_and_vote_on_an_open_board(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (_, kari) = site.invite("Kari Nordmann", 1).await;
    let (_, ola) = site.invite("Ola Hansen", 1).await;

    let response = site
        .post(
            &format!("/r/{kari}/board?lang=en"),
            "body=Who+brings+cake%3F",
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let to = location(&response).to_owned();
    let (page, anchor) = to.split_once('#').unwrap();
    assert_eq!(page, format!("/r/{kari}/board?lang=en&notice=posted"));
    let post = anchor.strip_prefix("post-").unwrap();

    // Ola sees it signed as the guest list shows Kari, who hasn't answered yet.
    let html = body(site.get(&format!("/r/{ola}/board?lang=en")).await).await;
    assert!(html.contains("Who brings cake?"), "{html}");
    assert!(html.contains("Kari N."), "{html}");
    assert!(!html.contains("Nordmann"), "{html}");
    let vote = format!("/r/{ola}/board/{post}/vote?lang=en");
    assert!(html.contains(&escape(&vote)), "{html}");

    // A reply, and votes: down, then down again takes it back.
    let response = site
        .post(
            &format!("/r/{ola}/board?lang=en"),
            &format!("body=I+do&parent={post}"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    site.post(&vote, "vote=down").await;
    let html = body(site.get(&format!("/r/{kari}/board?lang=en")).await).await;
    assert!(html.contains(r#"<details class="folded">"#), "{html}");
    assert!(html.contains("I do"), "{html}");
    // Kari can't vote on her own post, but can delete it.
    assert!(
        !html.contains(&format!("/r/{kari}/board/{post}/vote")),
        "{html}"
    );
    assert!(
        html.contains(&format!("/r/{kari}/board/{post}/delete")),
        "{html}"
    );
    site.post(&vote, "vote=down").await;
    let html = body(site.get(&format!("/r/{kari}/board?lang=en")).await).await;
    assert!(!html.contains(r#"class="folded""#), "{html}");
}

#[sqlx::test]
async fn a_guest_deletes_only_their_own_posts(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (kari, kari_link) = site.invite("Kari", 1).await;
    let (_, ola_link) = site.invite("Ola", 1).await;
    let event = site.event().await.id;
    let post = board::create(&site.db, event, None, Writer::Guest(kari.id), "Mine")
        .await
        .unwrap()
        .unwrap();

    let response = site
        .post(&format!("/r/{ola_link}/board/{post}/delete?lang=en"), "")
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = site
        .post(&format!("/r/{kari_link}/board/{post}/delete?lang=en"), "")
        .await;
    assert_eq!(
        location(&response),
        format!("/r/{kari_link}/board?lang=en&notice=post_deleted")
    );
    assert_eq!(board::count(&site.db, event).await.unwrap(), 0);
}

#[sqlx::test]
async fn an_empty_post_is_kept_to_fix(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (_, link) = site.invite("Kari", 1).await;
    let long = "x".repeat(board::MAX_BODY + 1);
    let response = site
        .post(&format!("/r/{link}/board?lang=en"), &format!("body={long}"))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let error = crate::i18n::filled(&en().board_error_body, "n", board::MAX_BODY);
    assert!(html.contains(&escape(&error)), "{html}");
    assert!(html.contains(&long), "the draft is kept");
}

#[sqlx::test]
async fn posts_are_markdown_and_never_html(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (_, link) = site.invite("Kari", 1).await;
    let board = format!("/r/{link}/board?lang=en");
    let text = "**Cake** at <b>8</b>\n- [map](https://example.com)";
    site.post(
        &board,
        &format!("body={}", text.replace(' ', "+").replace('\n', "%0A")),
    )
    .await;
    let html = body(site.get(&board).await).await;
    assert!(
        html.contains("<strong>Cake</strong> at &#60;b&#62;8&#60;/b&#62;"),
        "{html}"
    );
    assert!(
        html.contains(
            r#"<li><a href="https://example.com" rel="nofollow ugc noopener">map</a></li>"#
        ),
        "{html}"
    );
}

#[sqlx::test]
async fn a_guest_posts_so_many_an_hour(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (_, kari) = site.invite("Kari", 1).await;
    let (_, ola) = site.invite("Ola", 1).await;
    let board = |link: &str| format!("/r/{link}/board?lang=en");
    for n in 0..crate::web::BOARD_POSTS_AN_HOUR {
        let response = site.post(&board(&kari), &format!("body=Post+{n}")).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "post {n}");
    }
    let response = site.post(&board(&kari), "body=One+more").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let html = body(response).await;
    assert!(html.contains(&escape(&en().board_error_busy)), "{html}");
    assert!(html.contains("One more"), "the draft is kept");
    // Others are counted apart.
    let response = site.post(&board(&ola), "body=Hi").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}
