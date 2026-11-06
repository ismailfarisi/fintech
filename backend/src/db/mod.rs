//! Repositories: the only code that talks SQL.
//!
//! * All SQL is static (`&'static str`) with bound parameters; sqlx 0.9 refuses
//!   dynamically built query strings, which rules out SQL injection by construction.
//! * Functions that must run inside a transaction take `&mut PgConnection`;
//!   callers pass `&mut *tx`.
//! * Every account-scoped read takes the owning `UserId` (tenant isolation).

pub mod accounts;
pub mod audit;
pub mod idempotency;
pub mod ledger;
pub mod orders;
pub mod sessions;
pub mod snapshots;
pub mod users;
pub mod watchlist;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(config: &Config) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(config.database_url.expose())
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect to database: {e}"))?;
    Ok(pool)
}

/// Postgres SQLSTATE for CHECK violations (e.g. a balance would go negative).
pub fn is_check_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23514"))
}

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}
