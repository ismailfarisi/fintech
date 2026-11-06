//! HTTP layer: routing, middleware, DTOs. Thin: handlers authorize, then call services.

pub mod accounts;
pub mod auth;
pub mod error;
pub mod extract;
pub mod idempotency;
pub mod market;
pub mod ws;

use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderName, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::app::SharedState;
use crate::security::middleware;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const MAX_BODY_BYTES: usize = 16 * 1024;

pub fn router(state: SharedState) -> Router {
    let v1 = Router::new()
        .nest("/auth", auth::router())
        .nest("/accounts", accounts::router())
        .merge(market::router())
        .route("/ws", get(ws::handler))
        .fallback(|| async { error::ApiError::not_found("endpoint") })
        .layer(from_fn_with_state(state.clone(), middleware::rate_limit))
        .layer(from_fn_with_state(state.clone(), middleware::csrf_origin_check));

    let mut app =
        Router::new().nest("/api/v1", v1).route("/healthz", get(|| async { "ok" })).route("/readyz", get(ready));

    let static_dir = &state.config.static_dir;
    if static_dir.join("index.html").exists() {
        let spa = ServeDir::new(static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
        app = app.fallback_service(spa);
    }

    app.layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)))
        .layer(from_fn_with_state(state.clone(), middleware::security_headers))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
            let id = req.headers().get(REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("-");
            // Path only: query strings may carry ids we don't want in logs.
            tracing::info_span!("http", method = %req.method(), path = %req.uri().path(), request_id = %id)
        }))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .with_state(state)
}

/// Readiness: the database answers.
async fn ready(State(state): State<SharedState>) -> (StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (StatusCode::OK, "ready"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "database unavailable"),
    }
}
