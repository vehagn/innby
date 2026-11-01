mod config;
mod db;
mod i18n;
mod link;
mod web;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// RSVP service for a single event.
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
    /// Apply the database migrations.
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Serve => {
            let db = db::pool(&config::database_url()?)?;
            web::serve(Config::from_env()?, db).await
        }
        Command::Migrate => db::migrate(&db::connect(&config::database_url()?).await?).await,
    }
}
