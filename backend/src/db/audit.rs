//! Append-only security & money audit trail.

use serde_json::Value;
use sqlx::PgExecutor;

use crate::domain::ids::{AccountId, UserId};

#[derive(Debug, Clone, Default)]
pub struct Actor {
    pub user_id: Option<UserId>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub async fn record(
    db: impl PgExecutor<'_>,
    actor: &Actor,
    account_id: Option<AccountId>,
    action: &str,
    details: Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO audit_log (user_id, account_id, action, ip, user_agent, details) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(actor.user_id)
    .bind(account_id)
    .bind(action)
    .bind(&actor.ip)
    .bind(&actor.user_agent)
    .bind(details)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct AuditEntry {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub details: Value,
}

pub async fn recent_for_user(db: impl PgExecutor<'_>, user_id: UserId, limit: i64) -> sqlx::Result<Vec<AuditEntry>> {
    sqlx::query_as(
        "SELECT at, action, ip, user_agent, details FROM audit_log WHERE user_id = $1 ORDER BY at DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
}
