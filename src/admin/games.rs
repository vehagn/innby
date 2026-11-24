//! An event's Games tab: which of the minigames in `INNBY_GAMES` the event offers, and
//! each one's high score table, with every name in full. Turning a game off deletes its
//! scores (`db::games::offer`).
//!
//! Every host of the event may play its games from here. They choose to (`create`), which
//! makes them a row of `game_host`, and then each game's link carries a score token of
//! theirs, as a guest's page does with the guest's (`web::games`). Their scores are
//! ranked with the guests', signed with their name and role, as "Hagen (Editor)", until
//! they stop playing (`delete`), which deletes them.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, load_event, redirect, saved},
    role::{Host, Role},
};
use crate::{
    db::{
        event::Event,
        games::{self, Name},
    },
    http::render,
    paths::{self, EventLinks},
};

/// A game in the config, and whether the event offers it.
struct Choice {
    id: String,
    name: String,
    offered: bool,
}

/// A game the event offers, and its best scores.
struct Scores {
    name: String,
    lines: Vec<(String, String)>,
}

/// A game to play, and where: the games' origin, with the host's score token.
struct Play {
    name: String,
    url: String,
}

#[derive(Template)]
#[template(path = "admin/games.html")]
struct GamesPage {
    event: Event,
    links: EventLinks,
    choices: Vec<Choice>,
    tables: Vec<Scores>,
    /// How guests see the host's scores signed, as "Hagen (Editor)".
    signed: String,
    /// The games to play, once the host plays them.
    play: Option<Vec<Play>>,
    saved: bool,
    error: Option<String>,
}

/// The most lines a table shows here.
const LINES: i64 = 10;

/// How long a host's score token lasts. Their role comes from their login, which the
/// site can't ask, so a token outlives a role taken away by at most this; it's long
/// enough for an evening's play, and the tab makes a new one each time it's opened.
const PLAY_FOR: SignedDuration = SignedDuration::from_hours(12);

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let host = Host::current();
    let Some(role) = host.role else {
        return Ok(StatusCode::FORBIDDEN.into_response());
    };
    let (offered, playing) = tokio::try_join!(
        games::offered(&state.db, event.id),
        games::playing(&state.db, event.id, &host.user),
    )?;
    let lang = lang::current();
    let t = lang.messages();
    let config = &state.config;
    let offered_games: Vec<_> = config
        .games
        .list
        .iter()
        .filter(|game| offered.contains(&game.id))
        .collect();
    let mut tables = Vec::new();
    for game in &offered_games {
        let entries = games::table(
            &state.db,
            event.id,
            &game.id,
            game.kind.higher_wins(),
            LINES,
        )
        .await?;
        tables.push(Scores {
            name: game.name.clone(),
            lines: entries
                .into_iter()
                .map(|entry| {
                    let name = match entry.name {
                        Name::Guest { name, .. } => name,
                        Name::Host { name, role } => Role::signature(t, &name, &role, true),
                    };
                    (name, game.kind.show(entry.score, t))
                })
                .collect(),
        });
    }
    let expires = Timestamp::now() + PLAY_FOR;
    let play = playing.map(|id| {
        offered_games
            .iter()
            .map(|game| {
                let token = config
                    .link_key
                    .host_score_token(id, role.name(), &game.id, expires);
                Play {
                    name: game.name.clone(),
                    url: config
                        .games
                        .play_url(game, &config.base_url, &token, lang.code()),
                }
            })
            .collect()
    });
    let choices = config
        .games
        .list
        .iter()
        .map(|game| Choice {
            offered: offered.contains(&game.id),
            id: game.id.clone(),
            name: game.name.clone(),
        })
        .collect();
    Ok(render(GamesPage {
        links: EventLinks::new(event.id),
        signed: Role::signature(t, &host.user, role.name(), false),
        event,
        choices,
        tables,
        play,
        saved: query.saved.is_some(),
        error: None,
    }))
}

/// Offers the games ticked, and no others: a `game` field for each.
pub(super) async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<Vec<(String, String)>>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    // Only games the config has, whatever the form says.
    let ticked: Vec<String> = state
        .config
        .games
        .list
        .iter()
        .filter(|game| {
            form.iter()
                .any(|(key, value)| key == "game" && *value == game.id)
        })
        .map(|game| game.id.clone())
        .collect();
    games::offer(&state.db, event.id, &ticked).await?;
    tracing::info!(event = %event.id, games = ticked.len(), "games offered");
    Ok(saved(paths::EVENT_GAMES, event.id))
}

/// Makes the host a player of the event's games, so the tab gives them links to play.
pub(super) async fn create(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    games::join(&state.db, event.id, &Host::current().user).await?;
    tracing::info!(event = %event.id, "a host plays the games");
    Ok(redirect(&paths::path(paths::EVENT_GAMES, event.id)))
}

/// Stops the host playing the event's games, which deletes their scores and stops the
/// links they had.
pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    games::leave(&state.db, event.id, &Host::current().user).await?;
    tracing::info!(event = %event.id, "a host stopped playing the games");
    Ok(redirect(&paths::path(paths::EVENT_GAMES, event.id)))
}
