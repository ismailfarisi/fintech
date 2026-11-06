//! `Idempotency-Key` support for money-moving POSTs.
//!
//! The operation and the stored response commit in one transaction, under a
//! per-key advisory lock, so a concurrent duplicate waits for the first request
//! and then replays its response instead of executing twice.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use sqlx::PgConnection;

use super::error::ApiError;
use crate::app::AppState;
use crate::db::idempotency;
use crate::domain::ids::UserId;
use crate::events::Event;
use crate::security::token::sha256;
use crate::services::ServiceResult;

pub const HEADER: &str = "idempotency-key";

pub fn key_from(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(v) = headers.get(HEADER) else { return Ok(None) };
    let k = v.to_str().map_err(|_| ApiError::bad_request("Idempotency-Key must be ASCII"))?.trim();
    let valid = (8..=128).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        return Err(ApiError::bad_request("Idempotency-Key must be 8–128 characters of [A-Za-z0-9_-]"));
    }
    Ok(Some(k.to_string()))
}

/// What an operation produced: HTTP status, JSON body, events to publish after commit.
pub struct Outcome {
    pub status: StatusCode,
    pub body: Value,
    pub events: Vec<Event>,
}

/// Run `op` in a transaction, honouring an optional idempotency key.
/// `fingerprint` identifies the request (route + body) so a key can't be
/// reused for a different request.
pub async fn run(
    state: &AppState,
    user_id: UserId,
    key: Option<String>,
    fingerprint: &[u8],
    op: impl AsyncFnOnce(&mut PgConnection) -> ServiceResult<Outcome>,
) -> Result<Response, ApiError> {
    let mut tx = state.db.begin().await?;
    let request_hash = sha256(fingerprint);
    if let Some(key) = &key {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("idem:{user_id}:{key}"))
            .execute(&mut *tx)
            .await?;
        if let Some(stored) = idempotency::get(&mut *tx, user_id, key).await? {
            if stored.request_hash != request_hash {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "idempotency_key_reused",
                    "this Idempotency-Key was already used for a different request",
                ));
            }
            let status = StatusCode::from_u16(stored.status as u16).unwrap_or(StatusCode::OK);
            let mut res = (status, axum::Json(stored.body)).into_response();
            res.headers_mut().insert("idempotent-replayed", HeaderValue::from_static("true"));
            return Ok(res);
        }
    }
    let outcome = op(&mut tx).await?;
    if let Some(key) = &key {
        idempotency::put(&mut *tx, user_id, key, &request_hash, outcome.status.as_u16() as i16, &outcome.body).await?;
    }
    tx.commit().await?;
    for e in outcome.events {
        state.events.publish(e);
    }
    Ok((outcome.status, axum::Json(outcome.body)).into_response())
}
