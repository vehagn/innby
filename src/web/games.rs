//! The minigames: the Games tab of a guest's pages, `/r/{link}/games`, with each game's
//! best scores and a way to play it, and the scores API the games call.
//!
//! A game is JavaScript on an origin of its own (`INNBY_GAMES_URL`, see docs/games.md),
//! so it can't read Innby's pages, and the guest's link with them. The guest's page links
//! to it with a score token in the URL's fragment, which browsers never send to a
//! server. The token saves and reads only that guest's scores in that game, and stops
//! working with their link. A game opened any other way plays without saving.
//!
//! The public API, which the game's page calls from the guest's browser:
//!
//! - `GET /api/games/{game}/scores`: the event's high score table and the guest's best.
//! - `POST /api/games/{game}/scores` with `{"score": 1234}`: keeps it if it's their
//!   best, and answers with the table. Not for a game whose server sends its scores.
//!
//! Both take the token as `Authorization: Bearer {token}`, and only the games' origin
//! may call them (CORS). The token is what authorises a call, so the site's CSRF check,
//! which turns away other origins, leaves these paths to it.
//!
//! A score from the guest's browser is whatever the browser says. A game with a server
//! of its own (`server:` in `INNBY_GAMES`) can work the score out there instead, say by
//! replaying the guest's moves, and send it on the internal API, on a port of its own
//! that only the games' servers can reach (`INNBY_INTERNAL_LISTEN`):
//!
//! - `GET /internal/games/{game}/scores` and `POST` with `{"score": 1234}`, as above,
//!   with the game's key as `Authorization: Bearer {key}` and the guest's score token
//!   as `Innby-Token: {token}`. The key (`innby game-key {game}`) proves which game is
//!   speaking, and the token which guest it's for.

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

use super::{
    AppState,
    rsvp::Guest,
    views::{GuestTab, TabLink, guest_tabs},
};
use crate::{
    config::{Game, ScoreKind},
    db::{
        event::Event,
        games::{self, Entry, MAX_SCORE},
        rsvp::{self, NameDisplay, Rsvp},
    },
    http::{ClientIp, render, server_error},
    i18n::{Lang, Messages},
    link::IdLink,
    paths::{self, GuestLinks},
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

#[derive(askama::Template)]
#[template(path = "games.html")]
struct GamesPage {
    lang: Lang,
    t: &'static Messages,
    title: String,
    tabs: Vec<TabLink>,
    games: Vec<GameView>,
}

/// The Games tab: each game the event offers, with its best scores. An event with none
/// has no such tab, so this leads back to the guest's reply.
pub async fn list(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let games = match for_page(&state, lang, &guest.event, &guest.rsvp).await {
        Ok(games) => games,
        Err(err) => return server_error(err),
    };
    let links = GuestLinks::new(&guest.link, lang);
    if games.is_empty() {
        return axum::response::Redirect::to(&links.page).into_response();
    }
    let t = lang.messages();
    render(GamesPage {
        lang,
        t,
        title: guest.event.details.title(lang).to_owned(),
        tabs: guest_tabs(t, &links, true, GuestTab::Games),
        games,
    })
}

/// Whether the event offers any game the config still has, for the guest's tabs.
pub(super) async fn offers_any(state: &AppState, event: &Event) -> sqlx::Result<bool> {
    let games = &state.config.games;
    if games.list.is_empty() {
        return Ok(false);
    }
    let offered = games::offered(&state.db, event.id).await?;
    Ok(offered.iter().any(|id| games.find(id).is_some()))
}

/// The event's games, for the Games tab: those it offers that the config still has.
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

/// The internal API, for the games' servers, when a game has one.
pub(super) fn internal_router(state: &AppState) -> Option<Router<AppState>> {
    state
        .config
        .games
        .list
        .iter()
        .any(|game| game.server)
        .then(|| {
            Router::new().route(
                paths::INTERNAL_GAME_SCORES,
                get(internal_table).post(internal_record),
            )
        })
}

/// The header a game's server sends the guest's score token in.
const TOKEN_HEADER: &str = "innby-token";

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
    /// A game's server without the game's key.
    NotTheGame,
    /// A score from a browser for a game whose server sends them.
    FromServer,
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
            Self::NotTheGame => {
                (StatusCode::UNAUTHORIZED, "the game's key is needed").into_response()
            }
            Self::FromServer => {
                (StatusCode::FORBIDDEN, "this game's server sends its scores").into_response()
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

fn game(state: &AppState, id: &str) -> Result<Game, Refusal> {
    state.config.games.find(id).cloned().ok_or(Refusal::NoGame)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    header(headers, header::AUTHORIZATION.as_str())?.strip_prefix("Bearer ")
}

/// The guest whose score token this is, for this game.
async fn player(state: &AppState, game: Game, token: Option<&str>) -> Result<Player, Refusal> {
    let token: IdLink = token
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

/// A game's server, with the game's key, and the guest it's sending for.
async fn server_player(state: &AppState, id: &str, headers: &HeaderMap) -> Result<Player, Refusal> {
    let game = game(state, id)
        .ok()
        .filter(|game| game.server)
        .ok_or(Refusal::NoGame)?;
    let key = bearer(headers).ok_or(Refusal::NotTheGame)?;
    if !state.config.link_key.verify_game_key(&game.id, key) {
        return Err(Refusal::NotTheGame);
    }
    player(state, game, header(headers, TOKEN_HEADER)).await
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

/// The table, if the guest's event offers the game.
async fn show(state: &AppState, lang: Lang, player: Player) -> Result<Json<Table>, Refusal> {
    let offered = games::offered(&state.db, player.rsvp.event_id).await?;
    if !offered.contains(&player.game.id) {
        return Err(Refusal::NoGame);
    }
    answer(state, lang, player).await
}

/// Keeps the score if it's the guest's best, and answers with the table.
async fn keep(
    state: &AppState,
    lang: Lang,
    player: Player,
    score: i64,
) -> Result<Json<Table>, Refusal> {
    if !(0..=MAX_SCORE).contains(&score) {
        return Err(Refusal::BadScore);
    }
    let higher_wins = player.game.kind.higher_wins();
    if !games::record(
        &state.db,
        player.rsvp.id,
        &player.game.id,
        score,
        higher_wins,
    )
    .await?
    {
        return Err(Refusal::NoGame);
    }
    tracing::info!(rsvp = %player.rsvp.id, game = player.game.id, "score sent");
    answer(state, lang, player).await
}

#[derive(Deserialize)]
struct ScoreForm {
    score: i64,
}

/// The event's high score table for the game, for the guest's browser.
async fn table(
    State(state): State<AppState>,
    lang: Lang,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Table>, Refusal> {
    let player = player(&state, game(&state, &id)?, bearer(&headers)).await?;
    show(&state, lang, player).await
}

/// A score from the guest's browser.
async fn record(
    State(state): State<AppState>,
    lang: Lang,
    client: ClientIp,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(form): Json<ScoreForm>,
) -> Result<Json<Table>, Refusal> {
    if !state.score_limiter.check(client) {
        return Err(Refusal::TooMany);
    }
    let game = game(&state, &id)?;
    if game.server {
        return Err(Refusal::FromServer);
    }
    let player = player(&state, game, bearer(&headers)).await?;
    keep(&state, lang, player, form.score).await
}

/// The table, for a game's server.
async fn internal_table(
    State(state): State<AppState>,
    lang: Lang,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Table>, Refusal> {
    let player = server_player(&state, &id, &headers).await?;
    show(&state, lang, player).await
}

/// A score the game's server worked out.
async fn internal_record(
    State(state): State<AppState>,
    lang: Lang,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(form): Json<ScoreForm>,
) -> Result<Json<Table>, Refusal> {
    let player = server_player(&state, &id, &headers).await?;
    keep(&state, lang, player, form.score).await
}

#[cfg(test)]
mod tests;
