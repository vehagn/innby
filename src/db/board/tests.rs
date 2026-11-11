use sqlx::PgPool;

use super::*;
use crate::db::{event, rsvp};

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
    vote(&db, event, hello, ola, 1).await.unwrap();
    let posts = posts(&db, event, Some(ola)).await.unwrap();
    assert_eq!(
        (find(&posts, hello).score, find(&posts, hello).vote),
        (1, 1)
    );

    // Down instead, then down again, which takes it back.
    vote(&db, event, hello, ola, -1).await.unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, -1);
    vote(&db, event, hello, ola, -1).await.unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, 0);

    // Not on one's own.
    vote(&db, event, hello, kari, 1).await.unwrap();
    assert_eq!(super::posts(&db, event, None).await.unwrap()[0].score, 0);
}

#[sqlx::test]
async fn a_reply_is_to_a_post_of_the_same_event(db: PgPool) {
    let (event, kari, _) = setup(&db).await;
    let other = event::create(&db, &event::tests::details())
        .await
        .unwrap()
        .id;
    let elsewhere = create(&db, other, None, Writer::Host("admin"), "Elsewhere")
        .await
        .unwrap()
        .unwrap();
    let reply = create(&db, event, Some(elsewhere), Writer::Guest(kari), "Hi")
        .await
        .unwrap();
    assert_eq!(reply, None);
}

#[sqlx::test]
async fn a_deleted_post_stays_for_its_replies_only(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    let reply = post(&db, event, Some(hello), ola, "Hi Kari").await;

    // Not someone else's.
    assert!(!delete(&db, event, hello, Some(ola)).await.unwrap());
    assert!(delete(&db, event, hello, Some(kari)).await.unwrap());
    let posts = posts(&db, event, None).await.unwrap();
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
    vote(&db, event, reply, kari, 1).await.unwrap();
    rsvp::delete(&db, kari).await.unwrap();

    let posts = posts(&db, event, None).await.unwrap();
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
    assert_eq!(body(&"x".repeat(MAX_BODY + 1)), None);
    assert!(body(&"ø".repeat(MAX_BODY)).is_some());
}

fn at(id: u128, parent: Option<u128>, minute: i64, score: i64, body: bool) -> Post {
    Post {
        id: Uuid::from_u128(id),
        parent_id: parent.map(Uuid::from_u128),
        author: Author::Gone,
        body: body.then(|| format!("post {id}")),
        created_at: DateTime::from_timestamp(minute * 60, 0).unwrap(),
        score,
        vote: 0,
    }
}

/// The steps as text: a post by its number, `(` and `)` around its replies, `-` for one
/// folded away.
fn shape(steps: &[Step<u128>]) -> String {
    steps
        .iter()
        .map(|step| match step {
            Step::Open { post, collapsed } => {
                format!("{}{post}(", if *collapsed { "-" } else { "" })
            }
            Step::Close { .. } => ")".to_owned(),
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
    let steps = threads(posts, |post| post.id.as_u128());
    assert_eq!(shape(&steps), "3()2()1(4()5()-6(7()))9(10())");
}

#[test]
fn deep_replies_line_up_instead_of_nesting() {
    let posts: Vec<Post> = (1..=MAX_DEPTH as u128 + 2)
        .map(|id| at(id, (id > 1).then(|| id - 1), id as i64, 0, true))
        .collect();
    let steps = threads(posts, |post| post.id.as_u128());
    assert_eq!(shape(&steps), "1(2(3(4(5(6()7()8())))))");
}

#[sqlx::test]
async fn the_board_goes_with_its_event(db: PgPool) {
    let (event, kari, ola) = setup(&db).await;
    let hello = post(&db, event, None, kari, "Hello").await;
    create(&db, event, Some(hello), Writer::Host("admin"), "Hi")
        .await
        .unwrap();
    vote(&db, event, hello, ola, 1).await.unwrap();
    assert!(event::delete(&db, event).await.unwrap());
    let left = sqlx::query_scalar!(
        r#"select (select count(*) from board_post) + (select count(*) from board_vote) as "n!""#
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left, 0);
}
