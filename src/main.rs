//! Innby: an RSVP site that keeps as little about its guests as it can. One binary, run
//! as several workloads (`innby serve`, `admin`, `mailer`, `janitor`, `migrate`), linked
//! only by Postgres. AGENTS.md has the map of the whole repository.
//!
//! - `web`: the public site. `admin`: the host's UI. `http`: what both listeners share.
//! - `mailer`: sends the outbox's mail. `janitor`: the retention rules.
//! - `db`: the queries, per table. `config`: the settings. `i18n`: languages and texts.
//! - `link`: signed links. `paths`: every route and link. `theme`: the colour scheme.
//! - `random`: tokens. `testing` and `e2e`: test helpers and end-to-end journeys.
//! - `alloc`: the memory allocator.

mod admin;
mod alloc;
mod calendar;
mod config;
mod db;
#[cfg(test)]
mod e2e;
mod http;
mod i18n;
mod icons;
mod janitor;
mod link;
mod mailer;
mod paths;
mod random;
#[cfg(test)]
mod testing;
mod theme;
mod web;

use std::io::Read;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::{AdminConfig, Config, MailConfig};

/// RSVP service for a host's events.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the public website.
    Serve,
    /// Serve the host's admin UI.
    Admin,
    /// Send queued mail.
    Mailer,
    /// Delete data that is past its retention, then exit.
    Janitor,
    /// Apply the database migrations.
    Migrate,
    /// Create an invitation and print the guest's link.
    Invite {
        /// The event's id, from its address in the admin UI. Needed when there's more
        /// than one.
        #[arg(long)]
        event: Option<uuid::Uuid>,
        /// The guest or household, e.g. "Kari & Ola".
        name: String,
        /// How many people the invitation covers.
        #[arg(default_value_t = 1)]
        max_party_size: i32,
    },
    /// List events, or export or import one as a TOML file.
    Event {
        #[command(subcommand)]
        command: EventCommand,
    },
    /// Hash a password read from stdin, for INNBY_ADMIN_PASSWORD_HASH or INNBY_ADMIN_USERS.
    HashPassword,
    /// Print the key a game's server sends its scores with (see docs/games.md).
    GameKey {
        /// The game's id in INNBY_GAMES, e.g. "snake".
        game: String,
    },
}

#[derive(Subcommand)]
enum EventCommand {
    /// List the events, the soonest first, with their ids.
    List,
    /// Print an event and its group links as a TOML file.
    Export {
        /// The event's id, from `innby event list` or its address in the admin UI.
        id: uuid::Uuid,
    },
    /// Make a new event, and its group links, from a TOML file (`-` reads stdin).
    Import { file: std::path::PathBuf },
}

fn main() -> anyhow::Result<()> {
    // Before the runtime starts its threads, as mimalloc's options want.
    alloc::purge_at_once();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    i18n::load();

    match Cli::parse().command {
        Command::Serve => web::serve(Config::from_env()?, db::pool()?).await,
        Command::Admin => {
            let config = Config::from_env()?;
            let admin = AdminConfig::from_env(&config)?;
            admin::serve(config, admin, db::pool()?).await
        }
        Command::Mailer => {
            mailer::run(Config::from_env()?, MailConfig::from_env()?, db::pool()?).await
        }
        Command::Janitor => janitor::run(&db::connect().await?, chrono::Utc::now())
            .await
            .map(drop),
        Command::Migrate => db::migrate(&db::connect().await?).await,
        Command::Invite {
            event,
            name,
            max_party_size,
        } => {
            let config = Config::from_env()?;
            let db = db::connect().await?;
            println!(
                "{}",
                admin::cli::invite(&config, &db, event, &name, max_party_size).await?
            );
            Ok(())
        }
        Command::Event { command } => {
            let db = db::connect().await?;
            match command {
                EventCommand::List => print!("{}", admin::cli::event_list(&db).await?),
                EventCommand::Export { id } => {
                    print!("{}", admin::cli::export_event(&db, id).await?)
                }
                EventCommand::Import { file } => {
                    let text = if file.as_os_str() == "-" {
                        let mut text = String::new();
                        std::io::stdin().read_to_string(&mut text)?;
                        text
                    } else {
                        std::fs::read_to_string(&file)
                            .with_context(|| format!("reading {}", file.display()))?
                    };
                    println!("{}", admin::cli::import_event(&db, &text).await?);
                }
            }
            Ok(())
        }
        Command::GameKey { game } => {
            let config = Config::from_env()?;
            anyhow::ensure!(
                config.games.find(&game).is_some_and(|game| game.server),
                "INNBY_GAMES has no game {game:?} with server: before its name"
            );
            println!("{}", config.link_key.game_key(&game));
            Ok(())
        }
        Command::HashPassword => {
            let mut password = String::new();
            std::io::stdin()
                .read_to_string(&mut password)
                .context("reading the password from stdin")?;
            let password = password.trim_end_matches(['\r', '\n']);
            anyhow::ensure!(!password.is_empty(), "the password is empty");
            println!("{}", admin::auth::hash_password(password)?);
            Ok(())
        }
    }
}
