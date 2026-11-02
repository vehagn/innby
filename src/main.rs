mod admin;
mod config;
mod db;
mod i18n;
mod link;
mod web;

use std::io::Read;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::{AdminConfig, Config};

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
    /// Serve the host's admin UI.
    Admin,
    /// Apply the database migrations.
    Migrate,
    /// Create an invitation and print the guest's link.
    Invite {
        /// The guest or household, e.g. "Kari & Ola".
        name: String,
        /// How many people the invitation covers.
        #[arg(default_value_t = 1)]
        max_party_size: i32,
    },
    /// Hash a password read from stdin, for FEST_ADMIN_PASSWORD_HASH.
    HashPassword,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Serve => {
            let db = db::pool(&config::database_url()?)?;
            web::serve(Config::from_env()?, db).await
        }
        Command::Admin => {
            let db = db::pool(&config::database_url()?)?;
            admin::serve(Config::from_env()?, AdminConfig::from_env()?, db).await
        }
        Command::Migrate => db::migrate(&db::connect(&config::database_url()?).await?).await,
        Command::Invite {
            name,
            max_party_size,
        } => {
            let config = Config::from_env()?;
            let db = db::connect(&config::database_url()?).await?;
            println!(
                "{}",
                admin::invite(&config, &db, &name, max_party_size).await?
            );
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
