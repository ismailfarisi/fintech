//! Idempotency keys: a retried POST with the same key gets the original response.

use serde_json::Value;
use sqlx::PgExecutor;

use crate::domain::ids::UserId;

pub struct Stored {
    pub request_hash: Vec<u8>,
    pub status: i16,
    pub body: Value,
}

pub async fn get(db: impl PgExecutor<'_>, user_id: UserId, key: &str) -> sqlx::Result<Option<Stored>> {
    let row: Option<(Vec<u8>, i16, Value)> = sqlx::query_as(
        "SELECT request_hash, response_status, response_body FROM idempotency_keys WHERE user_id = $1 AND key = $2",
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(request_hash, status, body)| Stored { request_hash, status, body }))
}

/// Store a response. Returns `false` if the key was already taken (a concurrent
/// duplicate); the caller's transaction should then be rolled back.
pub async fn put(
    db: impl PgExecutor<'_>,
    user_id: UserId,
    key: &str,
    request_hash: &[u8],
    status: i16,
    body: &Value,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "INSERT INTO idempotency_keys (user_id, key, request_hash, response_status, response_body)
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(key)
    .bind(request_hash)
    .bind(status)
    .bind(body)
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn purge_older_than_days(db: impl PgExecutor<'_>, days: i32) -> sqlx::Result<u64> {
    let r = sqlx::query("DELETE FROM idempotency_keys WHERE created_at < now() - make_interval(days => $1)")
        .bind(days)
        .execute(db)
        .await?;
    Ok(r.rows_affected())
}
