//! /api/v1/market and /api/v1/watchlist.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use serde::Serialize;

use super::error::{ApiError, ApiResult};
use super::extract::{ApiPath, Auth, Json};
use crate::app::SharedState;
use crate::db::watchlist;
use crate::domain::order::normalize_symbol;
use crate::market::hub::ProviderStatus;
use crate::market::{PricePoint, Quote};

const MAX_WATCHLIST: i64 = 100;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/market/providers", get(providers))
        .route("/market/quotes", get(quotes))
        .route("/market/quotes/{symbol}", get(quote))
        .route("/watchlist", get(list_watch))
        .route("/watchlist/{symbol}", put(add_watch).delete(remove_watch))
}

/// Provider status without internal error details (those are logged server-side).
#[derive(Serialize)]
struct ProviderView {
    id: String,
    state: crate::market::hub::ProviderState,
    symbols: usize,
    last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    healthy: bool,
}

async fn providers(State(state): State<SharedState>, _auth: Auth) -> Json<Vec<ProviderView>> {
    let list: Vec<ProviderStatus> = state.providers.read().unwrap().values().cloned().collect();
    Json(
        list.into_iter()
            .map(|p| ProviderView {
                healthy: p.state == crate::market::hub::ProviderState::Live,
                id: p.id,
                state: p.state,
                symbols: p.symbols.len(),
                last_event_at: p.last_event_at,
            })
            .collect(),
    )
}

async fn quotes(State(state): State<SharedState>, _auth: Auth) -> Json<Vec<Quote>> {
    Json(state.market.read().unwrap().quotes())
}

#[derive(Serialize)]
struct QuoteDetail {
    quote: Quote,
    history: Vec<PricePoint>,
}

async fn quote(
    State(state): State<SharedState>,
    _auth: Auth,
    ApiPath(symbol): ApiPath<String>,
) -> ApiResult<Json<QuoteDetail>> {
    let symbol = normalize_symbol(&symbol);
    let m = state.market.read().unwrap();
    let quote = m.quote(&symbol).ok_or_else(|| ApiError::not_found("symbol"))?;
    let history = m.history(&symbol).unwrap_or_default();
    Ok(Json(QuoteDetail { quote, history }))
}

async fn list_watch(State(state): State<SharedState>, Auth(ctx): Auth) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(watchlist::list(&state.db, ctx.user.id).await?))
}

async fn add_watch(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(symbol): ApiPath<String>,
) -> ApiResult<Json<Vec<String>>> {
    let symbol = normalize_symbol(&symbol);
    if !state.market.read().unwrap().contains(&symbol) {
        return Err(ApiError::not_found("symbol"));
    }
    if watchlist::count(&state.db, ctx.user.id).await? >= MAX_WATCHLIST {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "limit_reached", "watchlist is full"));
    }
    watchlist::add(&state.db, ctx.user.id, &symbol).await?;
    Ok(Json(watchlist::list(&state.db, ctx.user.id).await?))
}

async fn remove_watch(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(symbol): ApiPath<String>,
) -> ApiResult<Json<Vec<String>>> {
    watchlist::remove(&state.db, ctx.user.id, &normalize_symbol(&symbol)).await?;
    Ok(Json(watchlist::list(&state.db, ctx.user.id).await?))
}
