use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgExecutor};

use crate::domain::ids::{AccountId, EntryId, OrderId, TradeId};
use crate::domain::order::{OrderKind, OrderStatus, Side};

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Order {
    pub id: OrderId,
    pub account_id: AccountId,
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub quantity: Decimal,
    pub limit_price: Option<Decimal>,
    pub status: OrderStatus,
    pub reserved: Decimal,
    pub fill_price: Option<Decimal>,
    pub created_at: DateTime<Utc>,
    pub filled_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Trade {
    pub id: TradeId,
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol: String,
    pub side: Side,
    pub quantity: Decimal,
    pub price: Decimal,
    pub gross: Decimal,
    pub fee: Decimal,
    pub realized_pnl: Option<Decimal>,
    pub executed_at: DateTime<Utc>,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    db: impl PgExecutor<'_>,
    id: OrderId,
    account_id: AccountId,
    symbol: &str,
    side: Side,
    kind: OrderKind,
    quantity: Decimal,
    limit_price: Option<Decimal>,
    reserved: Decimal,
) -> sqlx::Result<Order> {
    sqlx::query_as(
        "INSERT INTO orders (id, account_id, symbol, side, kind, quantity, limit_price, status, reserved)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', $8)
         RETURNING id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at",
    )
    .bind(id)
    .bind(account_id)
    .bind(symbol)
    .bind(side)
    .bind(kind)
    .bind(quantity)
    .bind(limit_price)
    .bind(reserved)
    .fetch_one(db)
    .await
}

pub async fn get(db: impl PgExecutor<'_>, account_id: AccountId, id: OrderId) -> sqlx::Result<Option<Order>> {
    sqlx::query_as(
        "SELECT id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at
         FROM orders WHERE id = $1 AND account_id = $2",
    )
    .bind(id)
    .bind(account_id)
    .fetch_optional(db)
    .await
}

/// Lock an order row (caller must already hold the account's holdings locks).
pub async fn lock(conn: &mut PgConnection, id: OrderId) -> sqlx::Result<Option<Order>> {
    sqlx::query_as(
        "SELECT id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at
         FROM orders WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
}

pub async fn mark_filled(db: impl PgExecutor<'_>, id: OrderId, price: Decimal) -> sqlx::Result<Order> {
    sqlx::query_as(
        "UPDATE orders SET status = 'filled', reserved = 0, fill_price = $2, filled_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'open'
         RETURNING id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at",
    )
    .bind(id)
    .bind(price)
    .fetch_one(db)
    .await
}

pub async fn mark_cancelled(db: impl PgExecutor<'_>, id: OrderId) -> sqlx::Result<Order> {
    sqlx::query_as(
        "UPDATE orders SET status = 'cancelled', reserved = 0, cancelled_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'open'
         RETURNING id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at",
    )
    .bind(id)
    .fetch_one(db)
    .await
}

/// Orders newest first, optionally filtered by status, with keyset pagination.
pub async fn list(
    db: impl PgExecutor<'_>,
    account_id: AccountId,
    status: Option<OrderStatus>,
    before: Option<DateTime<Utc>>,
    limit: i64,
) -> sqlx::Result<Vec<Order>> {
    sqlx::query_as(
        "SELECT id, account_id, symbol, side, kind, quantity, limit_price, status, reserved, fill_price, created_at, filled_at, cancelled_at
         FROM orders WHERE account_id = $1 AND ($2::text IS NULL OR status = $2)
           AND ($3::timestamptz IS NULL OR created_at < $3)
         ORDER BY created_at DESC LIMIT $4",
    )
    .bind(account_id)
    .bind(status)
    .bind(before)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Open limit orders that `price` crosses, oldest first (price-time priority is
/// irrelevant for paper fills, but oldest-first is fair).
pub async fn crossing(
    db: impl PgExecutor<'_>,
    symbol: &str,
    price: Decimal,
    limit: i64,
) -> sqlx::Result<Vec<(OrderId, AccountId)>> {
    sqlx::query_as(
        "SELECT id, account_id FROM orders
         WHERE status = 'open' AND symbol = $1
           AND ((side = 'buy' AND limit_price >= $2) OR (side = 'sell' AND limit_price <= $2))
         ORDER BY created_at LIMIT $3",
    )
    .bind(symbol)
    .bind(price)
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn open_for_account(db: impl PgExecutor<'_>, account_id: AccountId) -> sqlx::Result<Vec<OrderId>> {
    sqlx::query_scalar("SELECT id FROM orders WHERE account_id = $1 AND status = 'open'")
        .bind(account_id)
        .fetch_all(db)
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_trade(
    db: impl PgExecutor<'_>,
    order: &Order,
    entry_id: EntryId,
    price: Decimal,
    gross: Decimal,
    fee: Decimal,
    realized_pnl: Option<Decimal>,
) -> sqlx::Result<Trade> {
    sqlx::query_as(
        "INSERT INTO trades (id, order_id, account_id, ledger_entry_id, symbol, side, quantity, price, gross, fee, realized_pnl)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         RETURNING id, order_id, account_id, symbol, side, quantity, price, gross, fee, realized_pnl, executed_at",
    )
    .bind(TradeId::new())
    .bind(order.id)
    .bind(order.account_id)
    .bind(entry_id)
    .bind(&order.symbol)
    .bind(order.side)
    .bind(order.quantity)
    .bind(price)
    .bind(gross)
    .bind(fee)
    .bind(realized_pnl)
    .fetch_one(db)
    .await
}

pub async fn trades(
    db: impl PgExecutor<'_>,
    account_id: AccountId,
    before: Option<DateTime<Utc>>,
    limit: i64,
) -> sqlx::Result<Vec<Trade>> {
    sqlx::query_as(
        "SELECT id, order_id, account_id, symbol, side, quantity, price, gross, fee, realized_pnl, executed_at
         FROM trades WHERE account_id = $1 AND ($2::timestamptz IS NULL OR executed_at < $2)
         ORDER BY executed_at DESC LIMIT $3",
    )
    .bind(account_id)
    .bind(before)
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn realized_pnl(db: impl PgExecutor<'_>, account_id: AccountId) -> sqlx::Result<Decimal> {
    sqlx::query_scalar("SELECT coalesce(sum(realized_pnl), 0) FROM trades WHERE account_id = $1")
        .bind(account_id)
        .fetch_one(db)
        .await
}
