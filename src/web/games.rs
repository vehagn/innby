//! The minigames: what the guest's page shows of them, and the scores API they call.
//!
//! A game is JavaScript on an origin of its own (`INNBY_GAMES_URL`, see docs/games.md),
//! so it can't read Innby's pages, and the guest's link with them. The guest's page links
//! to it with a score token in the URL's fragment, which browsers never send to a
//! server. The token saves and reads only that guest's scores in that game, and stops
//! working with their link. A game opened any other way plays without saving.
//!
//! - `GET /api/games/{game}/scores`: the event's high score table and the guest's best.
//! - `POST /api/games/{game}/scores` with `{"score": 1234}`: keeps it if it's their
//!   best, and answers with the table.
//!
//! Both take the token as `Authorization: Bearer {token}`, and only the games' origin
//! may call them (CORS). The token is what authorises a call, so the site's CSRF check,
//! which turns away other origins, leaves these paths to it.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tower_http::cors::CorsLayer;

use super::AppState;
use crate::{
    config::{Game, ScoreKind},
    db::{
        event::Event,
        games::{self, Entry, MAX_SCORE},
        rsvp::{self, NameDisplay, Rsvp},
    },
    http::{ClientIp, server_error},
    i18n::{Lang, Messages},
    link::IdLink,
    paths,
};

/// The lines of a high score table on the guest's page; a game asks for more.
const ON_PAGE: i64 = 5;
const IN_GAME: i64 = 10;

/// One line of a high score table.
#[derive(Debug, Serialize)]
pub struct Line {
    /// As on the guest list, or "A guest" for those who aren't listed.
    pub name: String,
    pub score: i64,
    /// The score as the page shows it: points, or seconds.
    pub shown: String,
    /// Whether it's the guest's own.
    pub you: bool,
}

/// A game as the guest's page shows it: where to play it, and its best scores.
pub struct GameView {
    pub name: String,
    pub url: String,
    pub lines: Vec<Line>,
}

/// The event's games, for the guest's page: those it offers that the config still has.
pub(super) async fn for_page(
    state: &AppState,
    lang: Lang,
    event: &Event,
    rsvp: &Rsvp,
) -> sqlx::Result<Vec<GameView>> {
    let games = &state.config.games;
    if games.list.is_empty() {
        return Ok(Vec::new());
    }
    let mut views = Vec::new();
    for id in games::offered(&state.db, event.id).await? {
        let Some(game) = games.find(&id) else {
            continue;
        };
        let entries = games::table(
            &state.db,
            event.id,
            &game.id,
            game.kind.higher_wins(),
            ON_PAGE,
        )
        .await?;
        let token = state
            .config
            .link_key
            .score_token(rsvp.id, rsvp.link_nonce, &game.id);
        views.push(GameView {
            name: game.name.clone(),
            url: games.play_url(game, &state.config.base_url, &token, lang.code()),
            lines: lines(lang.messages(), game.kind, rsvp.id, entries),
        });
    }
    Ok(views)
}

fn lines(t: &Messages, kind: ScoreKind, you: uuid::Uuid, entries: Vec<Entry>) -> Vec<Line> {
    entries
        .into_iter()
        .map(|entry| Line {
            name: NameDisplay::listed(entry.response, entry.name_display)
                .apply(&entry.name)
                .unwrap_or_else(|| t.a_guest.clone()),
            shown: kind.show(entry.score, t),
            score: entry.score,
            you: entry.rsvp_id == you,
        })
        .collect()
}

/// The scores API, for the games' origin alone, when there are games.
pub(super) fn router(state: &AppState) -> Option<Router<AppState>> {
    let origin = state.config.games.origin()?;
    let cors = CorsLayer::new()
        .allow_origin(HeaderValue::from_str(origin).ok()?)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(Duration::from_secs(60 * 60));
    Some(
        Router::new()
            .route(paths::GAME_SCORES, get(table).post(record))
            .layer(cors),
    )
}

/// What the API answers with.
#[derive(Serialize)]
pub struct Table {
    game: String,
    kind: ScoreKind,
    /// The best first.
    scores: Vec<Line>,
    /// The guest's best, whether or not it's in the table.
    yours: Option<i64>,
}

/// The guest a score token is for, and the game it's for, once both check out.
struct Player {
    rsvp: Rsvp,
    game: Game,
}

/// Why a call was turned away. The same answer for a token that's malformed, forged,
/// revoked or for a deleted guest, so a caller can't tell them apart.
enum Refusal {
    NoGame,
    Unauthorised,
    BadScore,
    TooMany,
    Database(sqlx::Error),
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Self::NoGame => (StatusCode::NOT_FOUND, "no such game").into_response(),
            Self::Unauthorised => {
                (StatusCode::UNAUTHORIZED, "a valid score token is needed").into_response()
            }
            Self::BadScore => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "a score is a whole number from 0",
            )
                .into_response(),
            Self::TooMany => (StatusCode::TOO_MANY_REQUESTS, "too many scores").into_response(),
            Self::Database(err) => server_error(err),
        }
    }
}

impl From<sqlx::Error> for Refusal {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

async fn player(state: &AppState, game: &str, headers: &HeaderMap) -> Result<Player, Refusal> {
    let game = state
        .config
        .games
        .find(game)
        .ok_or(Refusal::NoGame)?
        .clone();
    let token: IdLink = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|token| token.trim().parse().ok())
        .ok_or(Refusal::Unauthorised)?;
    let rsvp = rsvp::find(&state.db, token.id)
        .await?
        .filter(|rsvp| {
            state
                .config
                .link_key
                .verify_score(&token, rsvp.link_nonce, &game.id)
        })
        .ok_or(Refusal::Unauthorised)?;
    Ok(Player { rsvp, game })
}

async fn answer(state: &AppState, lang: Lang, player: Player) -> Result<Json<Table>, Refusal> {
    let Player { rsvp, game } = player;
    let higher_wins = game.kind.higher_wins();
    let (entries, bests) = tokio::try_join!(
        games::table(&state.db, rsvp.event_id, &game.id, higher_wins, IN_GAME),
        games::of_guest(&state.db, rsvp.id),
    )?;
    let yours = bests.into_iter().find(|best| best.game == game.id);
    Ok(Json(Table {
        scores: lines(lang.messages(), game.kind, rsvp.id, entries),
        yours: yours.map(|best| best.score),
        game: game.id,
        kind: game.kind,
    }))
}

/// The event's high score table for the game, if the event offers it.
async fn table(
    State(state): State<AppState>,
    lang: Lang,
    Path(game): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Table>, Refusal> {
    let player = player(&state, &game, &headers).await?;
    let offered = games::offered(&state.db, player.rsvp.event_id).await?;
    if !offered.contains(&player.game.id) {
        return Err(Refusal::NoGame);
    }
    answer(&state, lang, player).await
}

#[derive(Deserialize)]
struct ScoreForm {
    score: i64,
}

/// Keeps a score if it's the guest's best, and answers with the table.
async fn record(
    State(state): State<AppState>,
    lang: Lang,
    client: ClientIp,
    Path(game): Path<String>,
    headers: HeaderMap,
    Json(form): Json<ScoreForm>,
) -> Result<Json<Table>, Refusal> {
    if !state.score_limiter.check(client) {
        return Err(Refusal::TooMany);
    }
    if !(0..=MAX_SCORE).contains(&form.score) {
        return Err(Refusal::BadScore);
    }
    let player = player(&state, &game, &headers).await?;
    let higher_wins = player.game.kind.higher_wins();
    if !games::record(
        &state.db,
        player.rsvp.id,
        &player.game.id,
        form.score,
        higher_wins,
    )
    .await?
    {
        return Err(Refusal::NoGame);
    }
    tracing::info!(rsvp = %player.rsvp.id, game = player.game.id, "score sent");
    answer(&state, lang, player).await
}

#[cfg(test)]
mod tests;
