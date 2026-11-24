use super::*;
use crate::db::{event, rsvp};

/// The name a line goes by, guest's or host's.
fn name(entry: &Entry) -> &str {
    match &entry.name {
        Name::Guest { name, .. } | Name::Host { name, .. } => name,
    }
}

#[sqlx::test]
async fn keeps_each_guests_best_while_the_event_offers_the_game(db: PgPool) {
    let party = event::tests::event(&db).await;
    let kari = rsvp::create_invite(&db, party.id, "Kari", 1).await.unwrap();
    let ola = rsvp::create_invite(&db, party.id, "Ola", 1).await.unwrap();
    // Not offered yet.
    assert!(!record(&db, kari.id, "quiz", 900, false).await.unwrap());
    offer(&db, party.id, &["quiz".into(), "pinball".into()])
        .await
        .unwrap();
    assert_eq!(offered(&db, party.id).await.unwrap(), ["pinball", "quiz"]);

    // Time: the least wins, and a slower round doesn't replace it.
    for (who, ms) in [
        (kari.id, 900),
        (kari.id, 1200),
        (ola.id, 700),
        (kari.id, 800),
    ] {
        assert!(record(&db, who, "quiz", ms, false).await.unwrap());
    }
    let quiz = table(&db, party.id, "quiz", false, 10).await.unwrap();
    let scores: Vec<_> = quiz
        .iter()
        .map(|entry| (name(entry), entry.score))
        .collect();
    assert_eq!(scores, [("Ola", 700), ("Kari", 800)]);
    // Points: the most wins.
    record(&db, kari.id, "pinball", 50, true).await.unwrap();
    record(&db, kari.id, "pinball", 40, true).await.unwrap();
    let best = of_guest(&db, kari.id).await.unwrap();
    let best: Vec<_> = best
        .iter()
        .map(|best| (best.game.as_str(), best.score))
        .collect();
    assert_eq!(best, [("pinball", 50), ("quiz", 800)]);
    assert_eq!(
        table(&db, party.id, "quiz", false, 1).await.unwrap().len(),
        1
    );

    // Turning a game off deletes its scores.
    offer(&db, party.id, &["pinball".into()]).await.unwrap();
    assert!(
        table(&db, party.id, "quiz", false, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!record(&db, ola.id, "quiz", 600, false).await.unwrap());
    // And deleting a guest, theirs.
    rsvp::delete(&db, kari.id).await.unwrap();
    assert!(
        table(&db, party.id, "pinball", true, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test]
async fn a_host_plays_beside_the_guests_until_they_stop(db: PgPool) {
    let party = event::tests::event(&db).await;
    let other = event::create(&db, &event::tests::details()).await.unwrap();
    let kari = rsvp::create_invite(&db, party.id, "Kari", 1).await.unwrap();
    offer(&db, party.id, &["quiz".into()]).await.unwrap();
    let hagen = join(&db, party.id, "hagen").await.unwrap();
    // Joining again is the same row; at another event, another.
    assert_eq!(join(&db, party.id, "hagen").await.unwrap(), hagen);
    assert_eq!(playing(&db, party.id, "hagen").await.unwrap(), Some(hagen));
    assert_eq!(playing(&db, party.id, "kari").await.unwrap(), None);
    assert_ne!(join(&db, other.id, "hagen").await.unwrap(), hagen);
    assert_eq!(
        host(&db, hagen).await.unwrap(),
        Some(Host {
            id: hagen,
            event_id: party.id
        })
    );

    // Only a game the event offers, and only a better time, which takes the role then.
    assert!(
        !record_host(&db, hagen, "editor", "pinball", 1, true)
            .await
            .unwrap()
    );
    record(&db, kari.id, "quiz", 800, false).await.unwrap();
    for (role, ms) in [("viewer", 900), ("editor", 700), ("admin", 750)] {
        assert!(
            record_host(&db, hagen, role, "quiz", ms, false)
                .await
                .unwrap()
        );
    }
    let quiz = table(&db, party.id, "quiz", false, 10).await.unwrap();
    assert_eq!(
        quiz,
        [
            Entry {
                player: Player::Host(hagen),
                name: Name::Host {
                    name: "hagen".into(),
                    role: "editor".into()
                },
                score: 700,
            },
            Entry {
                player: Player::Guest(kari.id),
                name: Name::Guest {
                    name: "Kari".into(),
                    response: Response::Pending,
                    name_display: None,
                },
                score: 800,
            },
        ]
    );
    assert_eq!(
        best(&db, Player::Host(hagen), "quiz").await.unwrap(),
        Some(700)
    );
    assert_eq!(
        best(&db, Player::Guest(kari.id), "quiz").await.unwrap(),
        Some(800)
    );
    assert_eq!(
        best(&db, Player::Host(hagen), "pinball").await.unwrap(),
        None
    );

    // Turning the game off takes the host's score too, but they still play.
    offer(&db, party.id, &[]).await.unwrap();
    offer(&db, party.id, &["quiz".into()]).await.unwrap();
    assert!(
        table(&db, party.id, "quiz", false, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(host(&db, hagen).await.unwrap().is_some());

    // Stopping takes their row and their scores.
    record_host(&db, hagen, "editor", "quiz", 700, false)
        .await
        .unwrap();
    assert!(leave(&db, party.id, "hagen").await.unwrap());
    assert!(!leave(&db, party.id, "hagen").await.unwrap());
    assert_eq!(host(&db, hagen).await.unwrap(), None);
    assert!(
        table(&db, party.id, "quiz", false, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !record_host(&db, hagen, "editor", "quiz", 600, false)
            .await
            .unwrap()
    );

    // And deleting the event, the rest.
    event::delete(&db, other.id).await.unwrap();
    assert_eq!(playing(&db, other.id, "hagen").await.unwrap(), None);
}
