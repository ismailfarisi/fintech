//! /api/v1/accounts/{id}/...: portfolio, orders, trades, cash.
//! Every handler starts with `services::accounts::get_owned` (authorization).

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::error::{ApiError, ApiResult};
use super::extract::{ApiPath, ApiQuery, Auth, Json, RequestMeta};
use super::idempotency::{self, Outcome};
use crate::app::SharedState;
use crate::db::{accounts as accounts_db, ledger, orders, snapshots};
use crate::domain::ids::{AccountId, OrderId};
use crate::domain::order::{FeeSchedule, OrderKind, OrderRequest, OrderStatus, Side};
use crate::domain::portfolio::PortfolioSummary;
use crate::services::accounts::get_owned;
use crate::services::portfolio::{AccountBook, prices};
use crate::services::{ServiceError, accounts, cash, trading};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/", get(list))
        .route("/{account_id}", get(get_one))
        .route("/{account_id}/reset", post(reset))
        .route("/{account_id}/portfolio", get(portfolio))
        .route("/{account_id}/equity", get(equity))
        .route("/{account_id}/orders", get(list_orders).post(place_order))
        .route("/{account_id}/orders/{order_id}", get(get_order).delete(cancel_order))
        .route("/{account_id}/trades", get(list_trades))
        .route("/{account_id}/cash", get(cash_movements))
        .route("/{account_id}/cash/deposits", post(deposit))
        .route("/{account_id}/cash/withdrawals", post(withdraw))
        .route("/{account_id}/reconcile", get(reconcile))
}

const MAX_PAGE: i64 = 200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
    status: Option<OrderStatus>,
}

impl Page {
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, MAX_PAGE)
    }
}

async fn list(State(state): State<SharedState>, Auth(ctx): Auth) -> ApiResult<Json<Vec<accounts_db::Account>>> {
    Ok(Json(accounts_db::list_for_user(&state.db, ctx.user.id).await?))
}

async fn get_one(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
) -> ApiResult<Json<accounts_db::Account>> {
    Ok(Json(get_owned(&state.db, ctx.user.id, id).await?))
}

async fn reset(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
) -> ApiResult<Json<serde_json::Value>> {
    let (new_id, events) = accounts::reset(&state, ctx.user.id, id, &meta.actor(Some(&ctx))).await?;
    for e in events {
        state.events.publish(e);
    }
    Ok(Json(json!({ "closed_account_id": id, "account_id": new_id })))
}

async fn portfolio(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
) -> ApiResult<Json<PortfolioSummary>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    let book = AccountBook::load(&state.db, id).await?;
    Ok(Json(book.value(&prices(&state))))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EquityQuery {
    /// Hours of history (default 24, max 24*90).
    hours: Option<i64>,
}

async fn equity(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    ApiQuery(q): ApiQuery<EquityQuery>,
) -> ApiResult<Json<Vec<snapshots::EquityPoint>>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    let hours = q.hours.unwrap_or(24).clamp(1, 24 * 90);
    Ok(Json(snapshots::list(&state.db, id, Utc::now() - Duration::hours(hours)).await?))
}

async fn list_orders(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<Vec<orders::Order>>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    Ok(Json(orders::list(&state.db, id, page.status, page.before, page.limit()).await?))
}

async fn get_order(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath((id, order_id)): ApiPath<(AccountId, OrderId)>,
) -> ApiResult<Json<orders::Order>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    orders::get(&state.db, id, order_id).await?.map(Json).ok_or_else(|| ApiError::not_found("order"))
}

/// Money and quantities are JSON *strings* ("0.015"), never floats.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlaceOrderBody {
    symbol: String,
    side: Side,
    kind: OrderKind,
    quantity: Decimal,
    limit_price: Option<Decimal>,
}

async fn place_order(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    headers: HeaderMap,
    Json(body): Json<PlaceOrderBody>,
) -> ApiResult<Response> {
    let key = idempotency::key_from(&headers)?;
    get_owned(&state.db, ctx.user.id, id).await?;
    let fingerprint = serde_json::to_vec(&json!({ "op": "place_order", "account": id, "body": &body })).unwrap();
    let actor = meta.actor(Some(&ctx));
    let fees = FeeSchedule { bps: state.config.fee_bps };
    let market = trading::market_info(&state.market.read().unwrap(), &body.symbol).map_err(ServiceError::from)?;
    let req = OrderRequest {
        symbol: body.symbol,
        side: body.side,
        kind: body.kind,
        quantity: body.quantity,
        limit_price: body.limit_price,
    };
    idempotency::run(&state, ctx.user.id, key, &fingerprint, async move |conn| {
        let (placed, events) = trading::place(conn, id, req, market, fees, &actor).await?;
        Ok(Outcome {
            status: StatusCode::CREATED,
            body: serde_json::to_value(placed).map_err(anyhow::Error::from)?,
            events,
        })
    })
    .await
}

async fn cancel_order(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath((id, order_id)): ApiPath<(AccountId, OrderId)>,
) -> ApiResult<Json<orders::Order>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    let mut tx = state.db.begin().await?;
    let (order, events) = trading::cancel(&mut tx, id, order_id, &meta.actor(Some(&ctx))).await?;
    tx.commit().await?;
    for e in events {
        state.events.publish(e);
    }
    Ok(Json(order))
}

async fn list_trades(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<Vec<orders::Trade>>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    Ok(Json(orders::trades(&state.db, id, page.before, page.limit()).await?))
}

async fn cash_movements(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
) -> ApiResult<Json<Vec<ledger::CashMovement>>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    Ok(Json(ledger::cash_movements(&state.db, id, MAX_PAGE).await?))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AmountBody {
    amount: Decimal,
}

async fn deposit(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    headers: HeaderMap,
    Json(body): Json<AmountBody>,
) -> ApiResult<Response> {
    move_cash(state, meta, ctx, id, headers, body, true).await
}

async fn withdraw(
    State(state): State<SharedState>,
    meta: RequestMeta,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
    headers: HeaderMap,
    Json(body): Json<AmountBody>,
) -> ApiResult<Response> {
    move_cash(state, meta, ctx, id, headers, body, false).await
}

async fn move_cash(
    state: SharedState,
    meta: RequestMeta,
    ctx: crate::services::auth::AuthContext,
    id: AccountId,
    headers: HeaderMap,
    body: AmountBody,
    is_deposit: bool,
) -> ApiResult<Response> {
    let key = idempotency::key_from(&headers)?;
    get_owned(&state.db, ctx.user.id, id).await?;
    let op = if is_deposit { "deposit" } else { "withdraw" };
    let fingerprint = serde_json::to_vec(&json!({ "op": op, "account": id, "body": &body })).unwrap();
    let actor = meta.actor(Some(&ctx));
    idempotency::run(&state, ctx.user.id, key, &fingerprint, async move |conn| {
        let (entry_id, events) = if is_deposit {
            cash::deposit(conn, id, body.amount, &actor).await?
        } else {
            cash::withdraw(conn, id, body.amount, &actor).await?
        };
        Ok(Outcome {
            status: StatusCode::CREATED,
            body: json!({ "entry_id": entry_id, "amount": body.amount }),
            events,
        })
    })
    .await
}

/// Verify that cached holdings match the ledger for this account.
async fn reconcile(
    State(state): State<SharedState>,
    Auth(ctx): Auth,
    ApiPath(id): ApiPath<AccountId>,
) -> ApiResult<Json<serde_json::Value>> {
    get_owned(&state.db, ctx.user.id, id).await?;
    let mismatches = ledger::reconcile(&state.db, id).await?;
    Ok(Json(json!({
        "consistent": mismatches.is_empty(),
        "mismatches": mismatches.into_iter().map(|(asset, holding, ledger)| json!({ "asset": asset, "holding": holding, "ledger": ledger })).collect::<Vec<_>>(),
    })))
}
