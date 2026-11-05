//! The minigames an event offers, and each guest's best score in each: the host turns
//! games on per event, and a guest's game sends its scores through `web::games`. Only
//! the best is kept, and it goes with the guest, or when the event stops offering the
//! game.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::rsvp::{NameDisplay, Response};

/// The most a score may be: points, or milliseconds (about 31 years).
pub const MAX_SCORE: i64 = 1_000_000_000_000;

/// One line of a high score table: whose, and how they're listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub rsvp_id: Uuid,
    pub name: String,
    pub response: Response,
    pub name_display: Option<NameDisplay>,
    pub score: i64,
}

/// A guest's best score in one game, for their export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Best {
    pub game: String,
    pub score: i64,
    pub scored_at: DateTime<Utc>,
}

/// The games the event offers, by id.
pub async fn offered(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        "select game from event_game where event_id = $1 order by game",
        event_id
    )
    .fetch_all(db)
    .await
}

/// Offers these games at the event, and no others. A game it stops offering takes its
/// scores with it.
pub async fn offer(db: &PgPool, event_id: Uuid, games: &[String]) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query!(
        "delete from game_score where game <> all($2)
         and rsvp_id in (select id from rsvp where event_id = $1)",
        event_id,
        games,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "delete from event_game where event_id = $1 and game <> all($2)",
        event_id,
        games,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "insert into event_game (event_id, game) select $1, unnest($2::text[])
         on conflict do nothing",
        event_id,
        games,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Keeps the score if it's the guest's best in the game, which their event must offer.
/// Returns whether the event offers it: one statement, so a game the host turns off
/// meanwhile can't keep a score.
pub async fn record(
    db: impl PgExecutor<'_>,
    rsvp_id: Uuid,
    game: &str,
    score: i64,
    higher_wins: bool,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"
        with offered as (
            select r.id from rsvp r
            join event_game g on g.event_id = r.event_id and g.game = $2
            where r.id = $1
        ), kept as (
            insert into game_score (rsvp_id, game, score) select id, $2, $3 from offered
            on conflict (rsvp_id, game) do update set score = excluded.score, scored_at = now()
            where case when $4 then excluded.score > game_score.score
                       else excluded.score < game_score.score end
            returning 1
        )
        select exists (select from offered) as "offered!"
        "#,
        rsvp_id,
        game,
        score,
        higher_wins,
    )
    .fetch_one(db)
    .await
}

/// The best scores in the event's game, the best first, and the earlier of two equal.
pub async fn table(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    game: &str,
    higher_wins: bool,
    limit: i64,
) -> sqlx::Result<Vec<Entry>> {
    sqlx::query_as!(
        Entry,
        r#"
        select r.id as rsvp_id, r.name, r.response as "response: Response",
            r.name_display as "name_display: NameDisplay", s.score
        from game_score s join rsvp r on r.id = s.rsvp_id
        where r.event_id = $1 and s.game = $2
        order by case when $3 then -s.score else s.score end, s.scored_at
        limit $4
        "#,
        event_id,
        game,
        higher_wins,
        limit,
    )
    .fetch_all(db)
    .await
}

/// The guest's best score in each game, for their export and their own line.
pub async fn of_guest(db: impl PgExecutor<'_>, rsvp_id: Uuid) -> sqlx::Result<Vec<Best>> {
    sqlx::query_as!(
        Best,
        "select game, score, scored_at from game_score where rsvp_id = $1 order by game",
        rsvp_id
    )
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{event, rsvp};

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
            .map(|entry| (entry.name.as_str(), entry.score))
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
}
