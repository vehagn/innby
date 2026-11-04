//! Postgres: the pool, the migrations in `migrations/`, and the queries, one module per
//! table. Queries are `sqlx::query!` macros, checked against the schema at compile time
//! with the offline data in `.sqlx/` (`just db prepare` after changing one).
//!
//! - `event`: events, and deleting each one with everything in it.
//! - `rsvp`: guests' answers. `group`: group sign-up links. `outbox`: mail jobs.
//! - `qr`: the QR code style. `access`: who may see or run one event.
//! - `admin_login`: refresh tokens and revoked JWTs.
//! - `retention`: deleting what's no longer needed.

pub mod access;
pub mod admin_login;
pub mod event;
pub mod group;
pub mod outbox;
pub mod qr;
pub mod retention;
pub mod rsvp;

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::config::database_url;

/// The migrations in `migrations/`, compiled into the binary.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(3))
}

/// A pool for `DATABASE_URL` that connects on first use, so the servers start (and
/// report not ready) while the database is unavailable.
pub fn pool() -> Result<PgPool> {
    pool_options()
        .connect_lazy(&database_url()?)
        .context("parsing DATABASE_URL")
}

/// A pool with an established connection, for commands that need the database right away.
pub async fn connect() -> Result<PgPool> {
    pool_options()
        .connect(&database_url()?)
        .await
        .context("connecting to the database")
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    MIGRATOR.run(pool).await.context("running migrations")?;
    tracing::info!("database migrated");
    Ok(())
}

pub async fn ping(pool: &PgPool) -> sqlx::Result<()> {
    sqlx::query("select 1").execute(pool).await.map(drop)
}
