use sqlx::PgPool;

use super::*;
use crate::db::{event, rsvp};

/// A host who posts as the admin they are.
const HOST: Writer = Writer::Host {
    name: "admin",
    role: "admin",
};

/// An event with an open board, and two guests.
async fn setup(db: &PgPool) -> (Uuid, Uuid, Uuid) {
    let event = event::tests::event(db).await.id;
    assert!(set_state(db, event, BoardState::Open).await.unwrap());
    let kari = rsvp::create_invite(db, event, "Kari Nordmann", 1)
        .await
        .unwrap();
    let ola = rsvp::create_invite(db, event, "Ola", 1).await.unwrap();
    (event, kari.id, ola.id)
}

async fn post(db: &PgPool, event: Uuid, parent: Option<Uuid>, by: Uuid, body: &str) -> Uuid {
    create(db, event, parent, Writer::Guest(by), body)
        .await
        .unwrap()
        .expect("a post")
}

fn find(posts: &[Post], id: Uuid) -> &Post {
    posts.iter().find(|post| post.id == id).expect("the post")
}

#[sqlx::test]
async fn votes_add_up_and_the_same_vote_takes_it_back(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    vote(&db, event, hello, Voter::Guest(ola), 1).await.unwrap();
    let posts = page(&db, event, Some(Voter::Guest(ola)), 0)
        .await
        .unwrap()
        .posts;
    assert_eq!(
        (find(&posts, hello).score, find(&posts, hello).vote),
        (1, 1)
    );

    // Down instead, then down again, which takes it back.
    vote(&db, event, hello, Voter::Guest(ola), -1)
        .await
        .unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, -1);
    vote(&db, event, hello, Voter::Guest(ola), -1)
        .await
        .unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, 0);

    // Not on one's own.
    vote(&db, event, hello, Voter::Guest(kari), 1)
        .await
        .unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, 0);
}

#[sqlx::test]
async fn a_reply_is_to_a_post_of_the_same_event(db: PgPool) {
    let (event, kari, _) = setup(&db).await;
    let other = event::create(&db, &event::tests::details())
        .await
        .unwrap()
        .id;
    let elsewhere = create(&db, other, None, HOST, "Elsewhere")
        .await
        .unwrap()
        .unwrap();
    let reply = create(&db, event, Some(elsewhere), Writer::Guest(kari), "Hi")
        .await
        .unwrap();
    assert_eq!(reply, Err(Refused::Gone));
}

#[sqlx::test]
async fn a_deleted_post_stays_for_its_replies_only(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    let reply = post(&db, event, Some(hello), ola, "Hi Kari").await;

    // Not someone else's.
    assert!(!delete(&db, event, hello, Some(ola)).await.unwrap());
    assert!(delete(&db, event, hello, Some(kari)).await.unwrap());
    let posts = page(&db, event, None, 0).await.unwrap().posts;
    assert_eq!(find(&posts, hello).body, None);
    assert_eq!(find(&posts, hello).author, Author::Gone);

    // Once the reply goes, so does the empty post it answered.
    assert!(delete(&db, event, reply, None).await.unwrap());
    assert!(super::posts(&db, event, None).await.unwrap().is_empty());
}

#[sqlx::test]
async fn a_deleted_guests_posts_lose_their_text(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    let reply = post(&db, event, Some(hello), ola, "Hi Kari").await;
    vote(&db, event, reply, Voter::Guest(kari), 1)
        .await
        .unwrap();
    rsvp::delete(&db, kari).await.unwrap();

    let posts = page(&db, event, None, 0).await.unwrap().posts;
    assert_eq!(find(&posts, hello).body, None);
    assert_eq!(find(&posts, hello).author, Author::Gone);
    assert_eq!(find(&posts, reply).body.as_deref(), Some("Hi Kari"));
    // And their votes.
    assert_eq!(find(&posts, reply).score, 0);
    assert_eq!(written_by(&db, ola).await.unwrap().len(), 1);
}

#[test]
fn a_body_is_trimmed_and_checked() {
    assert_eq!(body("  Hi\r\nthere \n").as_deref(), Some("Hi\nthere"));
    assert_eq!(body(" \n "), None);
    assert_eq!(body("bell\u{7}"), None);
    assert_eq!(body("moc.elpmaxe\u{202e}"), None);
    assert_eq!(body(&"x".repeat(MAX_BODY + 1)), None);
    assert!(body(&"ø".repeat(MAX_BODY)).is_some());
}

fn at(id: u128, parent: Option<u128>, minute: i64, score: i64, body: bool) -> Post {
    Post {
        id: Uuid::from_u128(id),
        parent_id: parent.map(Uuid::from_u128),
        thread_id: Uuid::from_u128(id),
        author: Author::Gone,
        deleted: !body,
        body: body.then(|| format!("post {id}")),
        created_at: Timestamp::from_second(minute * 60).unwrap(),
        score,
        vote: 0,
    }
}

/// A post shown by its number, and the number of the one it answers, when `threads` says.
fn numbered(post: &Post, answers: Option<&Post>) -> (u128, Option<u128>) {
    (post.id.as_u128(), answers.map(|post| post.id.as_u128()))
}

/// The steps as text: a post by its number, `-` before one folded away, `↪` and a number
/// after one that says whom it answers; then its list, in `(` and `)` while it nests and
/// `[` and `]` once it doesn't. Each list closes as its post opened it, folded or not.
fn shape(steps: &[Step<(u128, Option<u128>)>]) -> String {
    let mut open = Vec::new();
    steps
        .iter()
        .map(|step| match step {
            Step::Open {
                post: (post, answers),
                collapsed,
                nests,
            } => {
                open.push((*nests, *collapsed));
                let folded = if *collapsed { "-" } else { "" };
                let answers = answers.map(|id| format!("↪{id}")).unwrap_or_default();
                let list = if *nests { '(' } else { '[' };
                format!("{folded}{post}{answers}{list}")
            }
            Step::Close { collapsed } => {
                let (nests, folded) = open.pop().expect("a post to close");
                assert_eq!(*collapsed, folded, "{steps:?}");
                let list = if nests { ")" } else { "]" };
                list.to_owned()
            }
        })
        .collect()
}

#[test]
fn threads_put_the_best_and_newest_first_and_replies_in_order() {
    let posts = vec![
        at(1, None, 1, 0, true),
        at(2, None, 2, 0, true),
        at(3, None, 3, 2, true),
        at(4, Some(1), 4, 0, true),
        at(5, Some(1), 5, 0, true),
        at(6, Some(1), 6, -1, true),
        at(7, Some(6), 7, 0, true),
        // Deleted, with nothing under it: not shown.
        at(8, None, 8, 0, false),
        // Deleted, with a reply: shown, empty.
        at(9, None, 0, 0, false),
        at(10, Some(9), 9, 0, true),
    ];
    let steps = threads(posts, numbered);
    assert_eq!(shape(&steps), "3()2()1(4()5()-6(7()))9(10())");
}

#[test]
fn deep_replies_line_up_after_the_post_they_answer_and_say_whom() {
    // Eight, each a reply to the one before: the first 0 deep, the last 7.
    let posts: Vec<Post> = (1..=8)
        .map(|id| at(id, (id > 1).then(|| id - 1), id as i64, 0, true))
        .collect();
    // Less than MAX_DEPTH deep, a post nests the next; the one at MAX_DEPTH and those
    // past it line up, each past it saying whom it answers. For 3 that's
    // 1(2(3(4[]5↪4[]6↪5[]7↪6[]8↪7[]))).
    let max = MAX_DEPTH as u128;
    let nesting = (1..=8u128).filter(|id| id - 1 < max).count();
    let wanted: String = (1..=8u128)
        .map(|id| match id - 1 {
            depth if depth < max => format!("{id}("),
            depth if depth == max => format!("{id}[]"),
            _ => format!("{id}↪{}[]", id - 1),
        })
        .chain(std::iter::repeat_n(")".to_owned(), nesting))
        .collect();
    let steps = threads(posts, numbered);
    assert_eq!(shape(&steps), wanted);
}

#[test]
fn replies_lined_up_after_a_post_keep_their_order() {
    assert_eq!(MAX_DEPTH, 3, "shapes drawn for a MAX_DEPTH of 3");
    let posts = vec![
        at(1, None, 1, 0, true),
        at(2, Some(1), 2, 0, true),
        at(3, Some(2), 3, 0, true),
        at(4, Some(3), 4, 0, true),
        // Under 4, at MAX_DEPTH (3): the best first, then the oldest, each with its own
        // replies after it.
        at(5, Some(4), 5, 0, true),
        at(6, Some(4), 6, 1, true),
        at(7, Some(5), 7, 0, true),
        at(8, Some(4), 8, 0, true),
        // And the next at MAX_DEPTH, after them all.
        at(9, Some(3), 9, 0, true),
    ];
    let steps = threads(posts, numbered);
    assert_eq!(shape(&steps), "1(2(3(4[]6↪4[]5↪4[]7↪5[]8↪4[]9[])))");
}

#[test]
fn a_post_folded_away_takes_the_replies_lined_up_after_it() {
    assert_eq!(MAX_DEPTH, 3, "shapes drawn for a MAX_DEPTH of 3");
    let posts = vec![
        at(1, None, 1, 0, true),
        at(2, Some(1), 2, 0, true),
        at(3, Some(2), 3, 0, true),
        // Folded at MAX_DEPTH (3), and again past it.
        at(4, Some(3), 4, -1, true),
        at(5, Some(4), 5, 0, true),
        at(6, Some(5), 6, -2, true),
        at(7, Some(6), 7, 0, true),
        at(8, Some(5), 8, 0, true),
        // Under neither, and first, as it scores better.
        at(9, Some(3), 9, 0, true),
    ];
    let steps = threads(posts, numbered);
    assert_eq!(shape(&steps), "1(2(3(9[]-4[5↪4[]8↪5[]-6↪5[7↪6[]]])))");
}

#[sqlx::test]
async fn the_board_goes_with_its_event(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    create(&db, event, Some(hello), HOST, "Hi")
        .await
        .unwrap()
        .unwrap();
    vote(&db, event, hello, Voter::Guest(ola), 1).await.unwrap();
    assert!(event::delete(&db, event).await.unwrap());
    let left = sqlx::query_scalar!(
        r#"select (select count(*) from board_post) + (select count(*) from board_vote) as "n!""#
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test]
async fn a_thread_and_the_board_hold_so_many(db: PgPool) {
    let (event, kari, _) = setup(&db).await;
    let first = post(&db, event, None, kari, "First").await;
    // Replies to replies count for the thread too.
    let mut last = first;
    for _ in 1..MAX_THREAD {
        last = post(&db, event, Some(last), kari, "Reply").await;
    }
    let refused = create(&db, event, Some(first), Writer::Guest(kari), "One more")
        .await
        .unwrap();
    assert_eq!(refused, Err(Refused::ThreadFull));

    for _ in MAX_THREAD..MAX_POSTS {
        post(&db, event, None, kari, "Thread").await;
    }
    let refused = create(&db, event, None, HOST, "Full").await.unwrap();
    assert_eq!(refused, Err(Refused::BoardFull));
}

#[sqlx::test]
async fn a_page_is_whole_threads_with_their_text(db: PgPool) {
    let (event, kari, _) = setup(&db).await;
    // A thread of PAGE_POSTS - 1, then threads of one, the newest first.
    let big = post(&db, event, None, kari, "Big").await;
    for _ in 2..PAGE_POSTS {
        post(&db, event, Some(big), kari, "Reply").await;
    }
    for n in 0..PAGE_POSTS + 2 {
        post(&db, event, None, kari, &format!("Small {n}")).await;
    }
    let first = page(&db, event, None, 0).await.unwrap();
    // The newest small threads come first, until there are PAGE_POSTS; the next page has
    // the last two, and the big thread, whole, though that takes it past PAGE_POSTS.
    assert_eq!(first.posts.len(), PAGE_POSTS);
    assert!(first.posts.iter().all(|post| post.body.is_some()));
    let second = page(&db, event, None, 1).await.unwrap();
    assert_eq!(second.count, 2);
    assert_eq!(second.posts.len(), 2 + PAGE_POSTS - 1);
    assert!(second.posts.iter().any(|post| post.id == big));
    // Past the last, the last.
    let past = page(&db, event, None, 99).await.unwrap();
    assert_eq!(past.number, 1);
}

#[sqlx::test]
async fn hosts_vote_as_guests_do_once_each_and_sign_with_their_role(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    let welcome = create(&db, event, None, HOST, "Welcome")
        .await
        .unwrap()
        .unwrap();
    let posts = super::posts(&db, event, None).await.unwrap();
    assert_eq!(
        find(&posts, welcome).author,
        Author::Host {
            name: "admin".into(),
            role: "admin".into()
        }
    );

    // A host's vote adds to the guests', and the host sees their own.
    vote(&db, event, hello, Voter::Guest(ola), 1).await.unwrap();
    vote(&db, event, hello, Voter::Host("admin"), 1)
        .await
        .unwrap();
    vote(&db, event, hello, Voter::Host("editor@example.com"), 1)
        .await
        .unwrap();
    let posts = page(&db, event, Some(Voter::Host("admin")), 0)
        .await
        .unwrap()
        .posts;
    assert_eq!(
        (find(&posts, hello).score, find(&posts, hello).vote),
        (3, 1)
    );
    // Once each: down instead, then down again, which takes it back.
    vote(&db, event, hello, Voter::Host("admin"), -1)
        .await
        .unwrap();
    let score = |posts: Vec<Post>, id| find(&posts, id).score;
    assert_eq!(
        score(super::posts(&db, event, None).await.unwrap(), hello),
        1
    );
    vote(&db, event, hello, Voter::Host("admin"), -1)
        .await
        .unwrap();
    assert_eq!(
        score(super::posts(&db, event, None).await.unwrap(), hello),
        2
    );

    // Not on their own, but on another host's.
    vote(&db, event, welcome, Voter::Host("admin"), 1)
        .await
        .unwrap();
    assert_eq!(
        score(super::posts(&db, event, None).await.unwrap(), welcome),
        0
    );
    vote(&db, event, welcome, Voter::Host("editor@example.com"), 1)
        .await
        .unwrap();
    vote(&db, event, welcome, Voter::Guest(kari), 1)
        .await
        .unwrap();
    assert_eq!(
        score(super::posts(&db, event, None).await.unwrap(), welcome),
        2
    );

    // A deleted post kept for its replies loses its votes and its host's name.
    post(&db, event, Some(welcome), kari, "Thanks").await;
    assert!(delete(&db, event, welcome, None).await.unwrap());
    let posts = super::posts(&db, event, None).await.unwrap();
    assert_eq!(find(&posts, welcome).author, Author::Gone);
    assert_eq!(find(&posts, welcome).score, 0);
}
