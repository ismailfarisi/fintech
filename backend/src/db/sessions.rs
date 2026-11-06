use chrono::{DateTime, Utc};
use sqlx::PgExecutor;

use crate::domain::ids::{SessionId, UserId};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    db: impl PgExecutor<'_>,
    id: SessionId,
    user_id: UserId,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
    ip: &str,
    user_agent: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, expires_at, ip, user_agent) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .bind(ip)
    .bind(user_agent)
    .execute(db)
    .await?;
    Ok(())
}

/// A live session: not revoked, before its absolute expiry, and used within
/// the idle timeout. The owning user must not be disabled.
pub async fn find_active(db: impl PgExecutor<'_>, token_hash: &[u8], idle_secs: i64) -> sqlx::Result<Option<Session>> {
    sqlx::query_as(
        "SELECT s.id, s.user_id, s.created_at, s.last_seen_at, s.expires_at, s.ip, s.user_agent
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = $1
           AND s.revoked_at IS NULL
           AND s.expires_at > now()
           AND s.last_seen_at > now() - make_interval(secs => $2)
           AND u.disabled_at IS NULL",
    )
    .bind(token_hash)
    .bind(idle_secs as f64)
    .fetch_optional(db)
    .await
}

pub async fn touch(db: impl PgExecutor<'_>, id: SessionId) -> sqlx::Result<()> {
    sqlx::query("UPDATE sessions SET last_seen_at = now() WHERE id = $1").bind(id).execute(db).await?;
    Ok(())
}

pub async fn list_active(db: impl PgExecutor<'_>, user_id: UserId, idle_secs: i64) -> sqlx::Result<Vec<Session>> {
    sqlx::query_as(
        "SELECT id, user_id, created_at, last_seen_at, expires_at, ip, user_agent FROM sessions
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()
           AND last_seen_at > now() - make_interval(secs => $2)
         ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .bind(idle_secs as f64)
    .fetch_all(db)
    .await
}

/// Revoke one session owned by `user_id`. Returns whether a session was revoked.
pub async fn revoke(db: impl PgExecutor<'_>, user_id: UserId, id: SessionId) -> sqlx::Result<bool> {
    let r = sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Revoke all of a user's sessions except `keep` (if given).
pub async fn revoke_all(db: impl PgExecutor<'_>, user_id: UserId, keep: Option<SessionId>) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now()
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(user_id)
    .bind(keep)
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// Delete sessions that ended more than 30 days ago.
pub async fn purge_old(db: impl PgExecutor<'_>) -> sqlx::Result<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE coalesce(revoked_at, expires_at) < now() - interval '30 days'")
        .execute(db)
        .await?;
    Ok(r.rows_affected())
}
