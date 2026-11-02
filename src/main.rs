mod admin;
mod config;
mod db;
mod i18n;
mod janitor;
mod link;
mod mailer;
mod paths;
mod web;

use std::io::Read;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::{AdminConfig, Config, MailConfig};

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
    /// Send queued mail.
    Mailer,
    /// Delete data that is past its retention, then exit.
    Janitor,
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
        Command::Janitor => {
            let event_start = config::event_start()?.to_utc();
            janitor::run(&db::connect().await?, event_start, chrono::Utc::now())
                .await
                .map(drop)
        }
        Command::Migrate => db::migrate(&db::connect().await?).await,
        Command::Invite {
            name,
            max_party_size,
        } => {
            let config = Config::from_env()?;
            let db = db::connect().await?;
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
