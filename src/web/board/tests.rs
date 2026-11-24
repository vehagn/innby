use axum::http::StatusCode;
use sqlx::PgPool;

use crate::{
    db::board::{self, BoardState, Writer},
    i18n::{fill, filled},
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
    let page = format!("/r/{link}?lang=en");
    let board = format!("/r/{link}/board?lang=en");
    for state in [BoardState::Off, BoardState::Hidden] {
        set(&site, state).await;
        let html = body(site.get(&page).await).await;
        assert!(!html.contains(&format!("/r/{link}/board")), "{html}");
        assert!(!html.contains(r#"id="board""#), "{html}");
        // It has no tab of its own, nor a page: it's on the Event tab, below the answer.
        assert!(!html.contains(&escape(&en().board_heading)), "{html}");
    }

    // Hosts post; guests read and vote, but don't post.
    set(&site, BoardState::Hosts).await;
    let event = site.event().await.id;
    // Signed by name and role, but with no more of an address than its name.
    let host = Writer::Host {
        name: "hagen@example.com",
        role: "editor",
    };
    board::create(&site.db, event, None, host, "Welcome!")
        .await
        .unwrap()
        .unwrap();
    let html = body(site.get(&page).await).await;
    assert!(html.contains("Welcome!"), "{html}");
    // Below the answer's buttons, and not a tab.
    let answer = html.find(r#"value="attending""#).unwrap();
    assert!(html.find(r#"id="board""#).unwrap() > answer, "{html}");
    assert!(
        !html.contains(&format!(r#"href="/r/{link}/board"#)),
        "{html}"
    );
    let signed = fill(
        &en().board_signed,
        &[("name", &"hagen"), ("role", &en().admin.role_editor)],
    );
    assert!(html.contains(&escape(&signed)), "{html}");
    assert!(html.contains(&en().board_host), "{html}");
    assert!(!html.contains("example.com"), "{html}");
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
    assert_eq!(page, format!("/r/{kari}?lang=en&notice=posted"));
    // Said on the board, where Kari is, rather than at the top of the page.
    let html = body(site.get(page).await).await;
    let said = html.find(&escape(&en().board_posted)).unwrap();
    assert!(said > html.find(r#"id="board""#).unwrap(), "{html}");
    let post = anchor.strip_prefix("post-").unwrap();

    // Ola sees it signed as the guest list shows Kari, who hasn't answered yet.
    let html = body(site.get(&format!("/r/{ola}?lang=en")).await).await;
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
    let html = body(site.get(&format!("/r/{kari}?lang=en")).await).await;
    assert!(html.contains(r#"<details class="quiet folded">"#), "{html}");
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
    let html = body(site.get(&format!("/r/{kari}?lang=en")).await).await;
    assert!(!html.contains(r#"class="quiet folded""#), "{html}");
}

#[sqlx::test]
async fn a_guest_off_the_guest_list_reads_and_votes_but_does_not_post(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (kari, _) = site.invite("Kari Nordmann", 1).await;
    let (_, ola) = site.invite("Ola Hansen", 1).await;
    let (_, per) = site.invite("Per Berg", 1).await;
    let event = site.event().await.id;
    let post = board::create(&site.db, event, None, Writer::Guest(kari.id), "Cake?")
        .await
        .unwrap()
        .unwrap();
    // Ola hides his name from the guest list; Per isn't coming, so isn't on it.
    site.post(&format!("/r/{ola}/listing?lang=en"), "name_display=hidden")
        .await;
    site.post(&format!("/r/{per}?lang=en"), "response=declined")
        .await;

    for (link, hint) in [
        (&ola, &en().board_hint_unlisted),
        (&per, &en().board_hint_declined),
    ] {
        let board = format!("/r/{link}/board?lang=en");
        let html = body(site.get(&format!("/r/{link}?lang=en")).await).await;
        assert!(html.contains("Cake?"), "{html}");
        assert!(html.contains(&escape(hint)), "{html}");
        assert!(!html.contains(r#"name="body""#), "{html}");
        // Ola is pointed to where he can show his name; Per would have to come.
        let show = format!(
            r#"{} <a href="/r/{link}/guests?lang=en">{}</a>"#,
            escape(hint),
            en().board_show_name
        );
        assert_eq!(html.contains(&show), *link == ola, "{html}");

        // Neither a thread nor a reply is posted.
        for form in ["body=Hello".to_owned(), format!("body=Hello&parent={post}")] {
            let response = site.post(&board, &form).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{form}");
            let html = body(response).await;
            assert!(html.contains(&escape(&en().board_error_unlisted)), "{html}");
        }
        assert_eq!(board::count(&site.db, event).await.unwrap(), 1);

        // Votes are only ever a sum, so anyone invited may vote.
        let vote = format!("/r/{link}/board/{post}/vote?lang=en");
        let response = site.post(&vote, "vote=up").await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    // Once Ola shows his name, he posts.
    site.post(&format!("/r/{ola}/listing?lang=en"), "name_display=full")
        .await;
    let response = site
        .post(&format!("/r/{ola}/board?lang=en"), "body=Yes")
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(board::count(&site.db, event).await.unwrap(), 2);
}

#[sqlx::test]
async fn a_post_keeps_a_name_when_its_guest_leaves_the_guest_list(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (_, kari) = site.invite("Kari Nordmann", 1).await;
    let (_, ola) = site.invite("Ola Hansen", 1).await;
    site.post(&format!("/r/{kari}?lang=en"), "response=attending")
        .await;
    site.post(&format!("/r/{kari}/board?lang=en"), "body=Bringing+cake")
        .await;
    let board = format!("/r/{ola}?lang=en");
    let html = body(site.get(&board).await).await;
    assert!(html.contains("Kari Nordmann"), "{html}");

    // Hidden from the list, or not coming after all: her first name and initial stay.
    for leave in [
        "/listing?lang=en&name_display=hidden",
        "?lang=en&response=declined",
    ] {
        let (path, form) = leave.split_once('&').unwrap();
        site.post(&format!("/r/{kari}{path}"), form).await;
        let html = body(site.get(&board).await).await;
        assert!(html.contains("Bringing cake"), "{html}");
        assert!(html.contains("Kari N."), "{leave}: {html}");
        assert!(!html.contains("Nordmann"), "{leave}: {html}");
        assert!(!html.contains(&en().a_guest), "{leave}: {html}");
    }
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
        format!("/r/{kari_link}?lang=en&notice=post_deleted#board")
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
    // The form for a new thread comes back open, under "nothing here yet", with why
    // beside the text kept.
    let empty = html.find(&escape(&en().board_empty)).expect(&html);
    let write = html
        .find(r#"<details class="quiet write" id="write" open>"#)
        .expect(&html);
    assert!(write > empty, "{html}");
    let error = escape(&filled(&en().board_error_body, "n", board::MAX_BODY));
    let why = html[write..].find(&error).expect(&html);
    let kept = html[write..].find(&long).expect("the draft is kept");
    assert!(why < kept, "{html}");
}

#[sqlx::test]
async fn a_reply_to_a_post_deleted_meanwhile_is_kept_for_a_new_thread(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (kari, link) = site.invite("Kari", 1).await;
    let event = site.event().await.id;
    let mut posts = Vec::new();
    for text in ["Kept", "Gone"] {
        let post = board::create(&site.db, event, None, Writer::Guest(kari.id), text)
            .await
            .unwrap()
            .unwrap();
        posts.push(post);
    }
    assert!(
        board::delete(&site.db, event, posts[1], None)
            .await
            .unwrap()
    );
    let response = site
        .post(
            &format!("/r/{link}/board?lang=en"),
            &format!("body=Too+late&parent={}", posts[1]),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    // Why, at the top of the board, as for any reply; the text, in the form for a new
    // thread, open.
    let why = html.find(&escape(&en().board_error_gone)).expect(&html);
    assert!(
        why < html.find(r#"<article class="post">"#).unwrap(),
        "{html}"
    );
    let write = html
        .find(r#"<details class="quiet write" id="write" open>"#)
        .expect(&html);
    assert!(html[write..].contains("Too late"), "{html}");
}

#[sqlx::test]
async fn a_new_post_is_written_below_the_posts(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (kari, link) = site.invite("Kari", 1).await;
    let event = site.event().await.id;
    // Threads enough for a second page, so the page has links between them.
    let mut newest = None;
    for n in 0..=board::PAGE_POSTS {
        let post = board::create(
            &site.db,
            event,
            None,
            Writer::Guest(kari.id),
            &format!("Post {n}"),
        )
        .await
        .unwrap()
        .unwrap();
        newest = Some(post);
    }
    let newest = newest.unwrap();
    let html = body(site.get(&format!("/r/{link}?lang=en")).await).await;
    // Folded, below the posts and the links between pages, bringing the page back to
    // itself.
    let folded = r#"<details class="quiet write" id="write">"#;
    let write = html.find(folded).expect(&html);
    assert!(write > html.find(r#"class="pager""#).unwrap(), "{html}");
    assert!(
        write > html.rfind(r#"<article class="post">"#).unwrap(),
        "{html}"
    );
    assert!(html[write..].contains(&en().board_new), "{html}");
    let board = format!("/r/{link}/board?lang=en");
    let action = format!(r#"action="{}#write""#, escape(&board));
    assert!(html[write..].contains(&action), "{html}");

    // A reply's error is at the top of the board, above the posts its form is among, and
    // the new post's form stays folded.
    let error = escape(&filled(&en().board_error_body, "n", board::MAX_BODY));
    let response = site.post(&board, &format!("body=+&parent={newest}")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let first = html.find(r#"<article class="post">"#).unwrap();
    assert!(html.find(&error).unwrap() < first, "{html}");
    assert!(html.contains(folded), "{html}");
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
    let html = body(site.get(&format!("/r/{link}?lang=en")).await).await;
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

#[sqlx::test]
async fn a_long_board_has_pages_and_a_vote_stays_on_its_own(db: PgPool) {
    let site = Site::new(db);
    set(&site, BoardState::Open).await;
    let (kari, _) = site.invite("Kari", 1).await;
    let (_, ola) = site.invite("Ola", 1).await;
    let event = site.event().await.id;
    // The first thread is the oldest, so it's on the second page.
    let mut first = None;
    for n in 0..=board::PAGE_POSTS {
        let post = board::create(
            &site.db,
            event,
            None,
            Writer::Guest(kari.id),
            &format!("Post {n}"),
        )
        .await
        .unwrap()
        .unwrap();
        first.get_or_insert(post);
    }
    let first = first.unwrap();
    let html = body(site.get(&format!("/r/{ola}?lang=en")).await).await;
    assert!(
        html.contains(&escape(&format!("/r/{ola}?lang=en&page=2#board"))),
        "{html}"
    );
    assert!(!html.contains("Post 0<"), "{html}");
    let html = body(site.get(&format!("/r/{ola}?lang=en&page=2")).await).await;
    assert!(html.contains("Post 0<"), "{html}");

    // Voted down, it stays on the second page, and the vote leads back there.
    let response = site
        .post(&format!("/r/{ola}/board/{first}/vote?lang=en"), "vote=down")
        .await;
    assert_eq!(
        location(&response),
        format!("/r/{ola}?lang=en&page=2#post-{first}")
    );
}
