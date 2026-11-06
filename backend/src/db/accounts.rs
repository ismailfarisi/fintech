use chrono::{DateTime, Utc};
use sqlx::PgExecutor;

use crate::domain::ids::{AccountId, UserId};

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Account {
    pub id: AccountId,
    #[serde(skip)]
    pub user_id: UserId,
    pub name: String,
    pub kind: String,
    pub base_currency: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

pub async fn insert(db: impl PgExecutor<'_>, id: AccountId, user_id: UserId, name: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO accounts (id, user_id, name, kind) VALUES ($1, $2, $3, 'paper')")
        .bind(id)
        .bind(user_id)
        .bind(name)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn list_for_user(db: impl PgExecutor<'_>, user_id: UserId) -> sqlx::Result<Vec<Account>> {
    sqlx::query_as(
        "SELECT id, user_id, name, kind, base_currency, created_at, closed_at FROM accounts
         WHERE user_id = $1 ORDER BY closed_at IS NOT NULL, created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

/// The authorization check for everything account-scoped: returns the account
/// only if `user_id` owns it.
pub async fn get_owned(db: impl PgExecutor<'_>, user_id: UserId, id: AccountId) -> sqlx::Result<Option<Account>> {
    sqlx::query_as(
        "SELECT id, user_id, name, kind, base_currency, created_at, closed_at FROM accounts
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Lock an open account row for the rest of the transaction.
pub async fn lock_open(db: impl PgExecutor<'_>, id: AccountId) -> sqlx::Result<Option<Account>> {
    sqlx::query_as(
        "SELECT id, user_id, name, kind, base_currency, created_at, closed_at FROM accounts
         WHERE id = $1 AND closed_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn close(db: impl PgExecutor<'_>, id: AccountId) -> sqlx::Result<()> {
    sqlx::query("UPDATE accounts SET closed_at = now() WHERE id = $1 AND closed_at IS NULL")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn list_open_ids(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<AccountId>> {
    sqlx::query_scalar("SELECT id FROM accounts WHERE closed_at IS NULL").fetch_all(db).await
}
