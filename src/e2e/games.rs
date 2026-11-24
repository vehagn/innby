//! Journeys through the minigames: a host offers them and plays them from the admin UI,
//! beside the guests, who see the host's score on their own Games tab.

use axum::http::StatusCode;
use sqlx::PgPool;

use super::{World, admin, en};

#[sqlx::test]
async fn a_host_plays_the_games_beside_the_guests(db: PgPool) {
    let world = World::with_games(db);
    let (t, a) = (en(), admin());
    let kari_link = world.invite("Kari Nordmann", 1).await;

    // The host offers the quiz, then chooses to play.
    let mut host = world.host().await;
    host.click(&a.games).await;
    host.check("Quick quiz");
    host.press(&a.save_games).await.expect(&a.saved);
    host.page()
        .expect("admin (Admin)")
        .expect_not("Play Quick quiz");
    host.press(&a.games_join).await.expect("Play Quick quiz");

    // Kari plays first, then the host beats her time.
    let mut kari = world.browser();
    kari.open(&kari_link).await;
    kari.click(&t.games_heading).await;
    world
        .play(&kari.link("Play Quick quiz"), 9_870)
        .await
        .unwrap();
    let table = world
        .play(&host.link("Play Quick quiz"), 8_760)
        .await
        .unwrap();
    assert_eq!(table["scores"][0]["name"], "admin (Admin)");
    assert_eq!(table["scores"][0]["host"], true);
    assert_eq!(table["scores"][0]["you"], true);
    assert_eq!(table["scores"][1]["name"], "Kari N.");

    // Kari sees the host's score ranked above hers, and so does the host.
    kari.click(&t.games_heading)
        .await
        .expect("admin (Admin) 8.76 s")
        .expect("Kari N. (you) 9.87 s");
    host.click(&a.games)
        .await
        .expect("admin (Admin) 8.76 s")
        .expect("Kari Nordmann 9.87 s");

    // Stopping takes the host off the table, and the old link no longer counts.
    let old_link = host.link("Play Quick quiz");
    host.press(&a.games_leave)
        .await
        .expect(&a.games_join)
        .expect_not("Play Quick quiz");
    kari.click(&t.games_heading)
        .await
        .expect_not("admin (Admin)")
        .expect("Kari N. (you)");
    assert_eq!(
        world.play(&old_link, 1).await.unwrap_err(),
        StatusCode::UNAUTHORIZED
    );
}
