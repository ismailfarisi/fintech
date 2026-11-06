//! REST + WebSocket API.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::engine::{
    Account, CashFlow, EngineError, EquityPoint, Order, OrderRequest, OrderStatus,
    PortfolioSummary, Trade, DEFAULT_STARTING_CASH,
};
use crate::market::{PricePoint, Quote};
use crate::state::{Event, SharedState};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/api/quotes", get(list_quotes))
        .route("/api/quotes/{symbol}", get(get_quote))
        .route("/api/portfolio", get(get_portfolio))
        .route("/api/portfolio/history", get(get_equity_history))
        .route("/api/orders", get(list_orders).post(place_order))
        .route("/api/orders/{id}", delete(cancel_order))
        .route("/api/trades", get(list_trades))
        .route("/api/cash", get(list_cash_flows))
        .route("/api/cash/deposit", post(deposit))
        .route("/api/cash/withdraw", post(withdraw))
        .route("/api/watchlist", get(get_watchlist))
        .route("/api/watchlist/{symbol}", put(add_watch).delete(remove_watch))
        .route("/api/account/reset", post(reset_account))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

// ---- errors --------------------------------------------------------------

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        let status = match e {
            EngineError::OrderNotFound | EngineError::UnknownSymbol(_) => StatusCode::NOT_FOUND,
            EngineError::OrderNotOpen => StatusCode::CONFLICT,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError(status, e.to_string())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn prices(state: &SharedState) -> std::collections::BTreeMap<String, f64> {
    state.market.read().unwrap().prices()
}

// ---- market --------------------------------------------------------------

async fn list_quotes(State(state): State<SharedState>) -> Json<Vec<Quote>> {
    Json(state.market.read().unwrap().quotes())
}

#[derive(Serialize)]
struct QuoteDetail {
    quote: Quote,
    history: Vec<PricePoint>,
}

async fn get_quote(State(state): State<SharedState>, Path(symbol): Path<String>) -> ApiResult<QuoteDetail> {
    let symbol = symbol.to_uppercase();
    let market = state.market.read().unwrap();
    let not_found = || ApiError(StatusCode::NOT_FOUND, format!("unknown symbol '{symbol}'"));
    let quote = market.quotes().into_iter().find(|q| q.symbol == symbol).ok_or_else(not_found)?;
    let history = market.history(&symbol).ok_or_else(not_found)?;
    Ok(Json(QuoteDetail { quote, history }))
}

// ---- portfolio -----------------------------------------------------------

async fn get_portfolio(State(state): State<SharedState>) -> Json<PortfolioSummary> {
    let prices = prices(&state);
    Json(state.account.lock().unwrap().summary(&prices))
}

async fn get_equity_history(State(state): State<SharedState>) -> Json<Vec<EquityPoint>> {
    Json(state.account.lock().unwrap().equity_history.iter().copied().collect())
}

// ---- orders --------------------------------------------------------------

#[derive(Deserialize)]
struct OrderFilter {
    status: Option<OrderStatus>,
}

async fn list_orders(State(state): State<SharedState>, Query(f): Query<OrderFilter>) -> Json<Vec<Order>> {
    let account = state.account.lock().unwrap();
    let mut orders: Vec<Order> = account
        .orders
        .iter()
        .filter(|o| f.status.is_none_or(|s| o.status == s))
        .cloned()
        .collect();
    orders.reverse(); // newest first
    Json(orders)
}

#[derive(Serialize)]
struct PlaceOrderResponse {
    order: Order,
    trade: Option<Trade>,
}

async fn place_order(
    State(state): State<SharedState>,
    Json(req): Json<OrderRequest>,
) -> Result<(StatusCode, Json<PlaceOrderResponse>), ApiError> {
    let price = state.market.read().unwrap().price(&req.symbol.trim().to_uppercase());
    let (order, trade) = state.account.lock().unwrap().place_order(req, price, Utc::now())?;
    state.save();
    if let Some(t) = &trade {
        state.notify(Event::Fill { trade: t.clone() });
    }
    state.notify(Event::AccountChanged);
    Ok((StatusCode::CREATED, Json(PlaceOrderResponse { order, trade })))
}

async fn cancel_order(State(state): State<SharedState>, Path(id): Path<Uuid>) -> ApiResult<Order> {
    let order = state.account.lock().unwrap().cancel_order(id)?;
    state.save();
    state.notify(Event::AccountChanged);
    Ok(Json(order))
}

async fn list_trades(State(state): State<SharedState>) -> Json<Vec<Trade>> {
    let mut trades = state.account.lock().unwrap().trades.clone();
    trades.reverse();
    Json(trades)
}

// ---- cash ----------------------------------------------------------------

#[derive(Deserialize)]
struct Amount {
    amount: f64,
}

async fn list_cash_flows(State(state): State<SharedState>) -> Json<Vec<CashFlow>> {
    let mut flows = state.account.lock().unwrap().cash_flows.clone();
    flows.reverse();
    Json(flows)
}

async fn deposit(State(state): State<SharedState>, Json(a): Json<Amount>) -> ApiResult<PortfolioSummary> {
    state.account.lock().unwrap().deposit(a.amount, Utc::now())?;
    after_cash_change(&state)
}

async fn withdraw(State(state): State<SharedState>, Json(a): Json<Amount>) -> ApiResult<PortfolioSummary> {
    state.account.lock().unwrap().withdraw(a.amount, Utc::now())?;
    after_cash_change(&state)
}

fn after_cash_change(state: &SharedState) -> ApiResult<PortfolioSummary> {
    state.save();
    state.notify(Event::AccountChanged);
    let prices = prices(state);
    Ok(Json(state.account.lock().unwrap().summary(&prices)))
}

// ---- watchlist -----------------------------------------------------------

async fn get_watchlist(State(state): State<SharedState>) -> Json<Vec<String>> {
    Json(state.account.lock().unwrap().watchlist.clone())
}

async fn add_watch(State(state): State<SharedState>, Path(symbol): Path<String>) -> ApiResult<Vec<String>> {
    let symbol = symbol.to_uppercase();
    if !state.market.read().unwrap().contains(&symbol) {
        return Err(EngineError::UnknownSymbol(symbol).into());
    }
    let list = {
        let mut a = state.account.lock().unwrap();
        a.add_to_watchlist(&symbol);
        a.watchlist.clone()
    };
    state.save();
    Ok(Json(list))
}

async fn remove_watch(State(state): State<SharedState>, Path(symbol): Path<String>) -> ApiResult<Vec<String>> {
    let list = {
        let mut a = state.account.lock().unwrap();
        a.remove_from_watchlist(&symbol);
        a.watchlist.clone()
    };
    state.save();
    Ok(Json(list))
}

// ---- account -------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ResetRequest {
    starting_cash: Option<f64>,
}

async fn reset_account(
    State(state): State<SharedState>,
    body: Option<Json<ResetRequest>>,
) -> ApiResult<PortfolioSummary> {
    let cash = body.map(|b| b.0).unwrap_or_default().starting_cash.unwrap_or(DEFAULT_STARTING_CASH);
    if !(cash.is_finite() && cash >= 0.0) {
        return Err(EngineError::InvalidAmount.into());
    }
    *state.account.lock().unwrap() = Account::new(cash, Utc::now());
    after_cash_change(&state)
}

// ---- websocket -----------------------------------------------------------

async fn ws_handler(State(state): State<SharedState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(mut socket: WebSocket, state: SharedState) {
    let mut rx = state.events.subscribe();
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                // Slow client: skip missed ticks, keep going.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(app: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, json)
    }

    #[tokio::test]
    async fn order_flow_over_http() {
        let app = router(AppState::new(None));

        let (s, quotes) = call(&app, "GET", "/api/quotes", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(quotes.as_array().unwrap().len() >= 5);

        let (s, body) = call(&app, "POST", "/api/orders",
            Some(json!({"symbol": "nova", "side": "buy", "kind": "market", "qty": 3}))).await;
        assert_eq!(s, StatusCode::CREATED);
        assert_eq!(body["order"]["status"], "filled");
        assert_eq!(body["trade"]["symbol"], "NOVA");

        let (_, p) = call(&app, "GET", "/api/portfolio", None).await;
        assert_eq!(p["positions"][0]["qty"], 3);

        let (s, body) = call(&app, "POST", "/api/orders",
            Some(json!({"symbol": "NOVA", "side": "sell", "kind": "market", "qty": 99}))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].as_str().unwrap().contains("insufficient shares"));

        // Resting limit far below market, then cancel it.
        let (s, body) = call(&app, "POST", "/api/orders",
            Some(json!({"symbol": "NOVA", "side": "buy", "kind": "limit", "qty": 1, "limit_price": 1.0}))).await;
        assert_eq!(s, StatusCode::CREATED);
        let id = body["order"]["id"].as_str().unwrap().to_string();
        let (_, open) = call(&app, "GET", "/api/orders?status=open", None).await;
        assert_eq!(open.as_array().unwrap().len(), 1);
        let (s, _) = call(&app, "DELETE", &format!("/api/orders/{id}"), None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = call(&app, "DELETE", &format!("/api/orders/{id}"), None).await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn unknown_symbols_are_404() {
        let app = router(AppState::new(None));
        let (s, _) = call(&app, "GET", "/api/quotes/ZZZZ", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = call(&app, "PUT", "/api/watchlist/ZZZZ", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = call(&app, "POST", "/api/orders",
            Some(json!({"symbol": "ZZZZ", "side": "buy", "kind": "market", "qty": 1}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cash_and_reset() {
        let app = router(AppState::new(None));
        let (s, p) = call(&app, "POST", "/api/cash/deposit", Some(json!({"amount": 2500}))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(p["cash"], 102_500.0);
        let (s, _) = call(&app, "POST", "/api/cash/withdraw", Some(json!({"amount": 1e9}))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        let (_, p) = call(&app, "POST", "/api/account/reset", Some(json!({"starting_cash": 5000}))).await;
        assert_eq!(p["equity"], 5000.0);
    }
}
