//! The minigames an event offers, and each player's best score in each: the host turns
//! games on per event, and a guest's or a host's game sends its scores through
//! `web::games`. A guest plays by their RSVP; a host by a row of `game_host`, made when
//! they choose to play (`join`), which their score tokens name. Only the best is kept,
//! and it goes with the guest or the host's row, or when the event stops offering the
//! game.

use jiff::Timestamp;
use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::rsvp::{NameDisplay, Response};

/// The most a score may be: points, or milliseconds (about 31 years).
pub const MAX_SCORE: i64 = 1_000_000_000_000;

/// Who's playing: a guest, by their RSVP, or a host, by their row of `game_host`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    Guest(Uuid),
    Host(Uuid),
}

/// How a line of a high score table names its player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Name {
    /// A guest, as the guest list shows them.
    Guest {
        name: String,
        response: Response,
        name_display: Option<NameDisplay>,
    },
    /// A host, by the name they log in with and their role for the event when they set
    /// the score.
    Host { name: String, role: String },
}

/// One line of a high score table: whose, and how they're named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub player: Player,
    pub name: Name,
    pub score: i64,
}

/// A host who plays an event's games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: Uuid,
    pub event_id: Uuid,
}

/// A guest's best score in one game, for their export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Best {
    pub game: String,
    pub score: i64,
    pub scored_at: Timestamp,
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
        "delete from game_host_score where game <> all($2)
         and host_id in (select id from game_host where event_id = $1)",
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

/// Keeps the score if it's the host's best in the game, which their event must offer,
/// signed with their role now. Returns whether the event offers it, as `record` does.
pub async fn record_host(
    db: impl PgExecutor<'_>,
    host_id: Uuid,
    role: &str,
    game: &str,
    score: i64,
    higher_wins: bool,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"
        with offered as (
            select h.id from game_host h
            join event_game g on g.event_id = h.event_id and g.game = $2
            where h.id = $1
        ), kept as (
            insert into game_host_score (host_id, game, host_role, score)
            select id, $2, $3, $4 from offered
            on conflict (host_id, game) do update
            set score = excluded.score, host_role = excluded.host_role, scored_at = now()
            where case when $5 then excluded.score > game_host_score.score
                       else excluded.score < game_host_score.score end
            returning 1
        )
        select exists (select from offered) as "offered!"
        "#,
        host_id,
        game,
        role,
        score,
        higher_wins,
    )
    .fetch_one(db)
    .await
}

/// A line of the table as the query gives it: a guest's or a host's.
struct Row {
    id: Uuid,
    host: bool,
    name: String,
    response: Option<Response>,
    name_display: Option<NameDisplay>,
    role: Option<String>,
    score: i64,
}

impl From<Row> for Entry {
    fn from(row: Row) -> Self {
        let (player, name) = match (row.host, row.response, row.role) {
            (false, Some(response), _) => (
                Player::Guest(row.id),
                Name::Guest {
                    name: row.name,
                    response,
                    name_display: row.name_display,
                },
            ),
            (_, _, role) => (
                Player::Host(row.id),
                Name::Host {
                    name: row.name,
                    role: role.unwrap_or_default(),
                },
            ),
        };
        Self {
            player,
            name,
            score: row.score,
        }
    }
}

/// The best scores in the event's game, guests' and hosts' together, the best first,
/// and the earlier of two equal.
pub async fn table(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    game: &str,
    higher_wins: bool,
    limit: i64,
) -> sqlx::Result<Vec<Entry>> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        select id as "id!", host as "host!", name as "name!",
            response as "response?: Response", name_display as "name_display?: NameDisplay",
            role, score as "score!"
        from (
            select r.id, false as host, r.name, r.response, r.name_display,
                null::text as role, s.score, s.scored_at
            from game_score s join rsvp r on r.id = s.rsvp_id
            where r.event_id = $1 and s.game = $2
            union all
            select h.id, true, h.host_name, null, null, s.host_role, s.score, s.scored_at
            from game_host_score s join game_host h on h.id = s.host_id
            where h.event_id = $1 and s.game = $2
        ) as line
        order by case when $3 then -score else score end, scored_at
        limit $4
        "#,
        event_id,
        game,
        higher_wins,
        limit,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Entry::from).collect())
}

/// The player's best score in the game, if they have one.
pub async fn best(
    db: impl PgExecutor<'_>,
    player: Player,
    game: &str,
) -> sqlx::Result<Option<i64>> {
    match player {
        Player::Guest(id) => {
            sqlx::query_scalar!(
                "select score from game_score where rsvp_id = $1 and game = $2",
                id,
                game
            )
            .fetch_optional(db)
            .await
        }
        Player::Host(id) => {
            sqlx::query_scalar!(
                "select score from game_host_score where host_id = $1 and game = $2",
                id,
                game
            )
            .fetch_optional(db)
            .await
        }
    }
}

/// Makes the host a player of the event's games, if they aren't already, and returns
/// their row.
pub async fn join(db: impl PgExecutor<'_>, event_id: Uuid, host_name: &str) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        "insert into game_host (event_id, host_name) values ($1, $2)
         on conflict (event_id, host_name) do update set host_name = excluded.host_name
         returning id",
        event_id,
        host_name,
    )
    .fetch_one(db)
    .await
}

/// The host's row, if they play the event's games.
pub async fn playing(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    host_name: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar!(
        "select id from game_host where event_id = $1 and host_name = $2",
        event_id,
        host_name,
    )
    .fetch_optional(db)
    .await
}

/// The playing host a score token names, if they still play.
pub async fn host(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Host>> {
    sqlx::query_as!(Host, "select id, event_id from game_host where id = $1", id)
        .fetch_optional(db)
        .await
}

/// Stops the host playing the event's games, which deletes their scores and stops their
/// score tokens. Returns whether they were playing.
pub async fn leave(db: impl PgExecutor<'_>, event_id: Uuid, host_name: &str) -> sqlx::Result<bool> {
    let deleted = sqlx::query!(
        "delete from game_host where event_id = $1 and host_name = $2",
        event_id,
        host_name,
    )
    .execute(db)
    .await?;
    Ok(deleted.rows_affected() > 0)
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
mod tests;
