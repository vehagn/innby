//! An event's Games tab: which of the minigames in `INNBY_GAMES` the event offers, and
//! each one's high score table, with every name in full. Turning a game off deletes its
//! scores (`db::games::offer`).

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use uuid::Uuid;

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, load_event, redirect},
};
use crate::{
    db::{event::Event, games},
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

#[derive(Template)]
#[template(path = "admin/games.html")]
struct GamesPage {
    event: Event,
    links: EventLinks,
    choices: Vec<Choice>,
    tables: Vec<Scores>,
    saved: bool,
    error: Option<String>,
}

/// The most lines a table shows here.
const LINES: i64 = 10;

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let offered = games::offered(&state.db, event.id).await?;
    let t = lang::current().messages();
    let mut tables = Vec::new();
    for game in state
        .config
        .games
        .list
        .iter()
        .filter(|game| offered.contains(&game.id))
    {
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
                .map(|entry| (entry.name, game.kind.show(entry.score, t)))
                .collect(),
        });
    }
    let choices = state
        .config
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
        event,
        choices,
        tables,
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
    Ok(redirect(&format!(
        "{}?saved",
        paths::path(paths::EVENT_GAMES, event.id)
    )))
}
