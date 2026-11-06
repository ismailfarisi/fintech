//! Order placement, cancellation and execution.
//!
//! Concurrency model: every operation on an account first locks the account
//! row (`SELECT ... FOR UPDATE`), which serializes all money movements per
//! account. The holdings CHECK constraints are a second line of defence.

use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;

use super::{ServiceError, ServiceResult};
use crate::db::accounts;
use crate::db::audit::{self, Actor};
use crate::db::ledger::{self, Book, EntryKind, Posting};
use crate::db::orders::{self, Order, Trade};
use crate::domain::DomainError;
use crate::domain::ids::{AccountId, OrderId};
use crate::domain::money::{CASH, price_from_f64, round_qty};
use crate::domain::order::{
    self, FeeSchedule, MarketInfo, OrderKind, OrderRequest, OrderStatus, PositionState, Side, compute_fill,
    is_marketable,
};
use crate::domain::portfolio::Holding;
use crate::events::{AccountChange, Event};
use crate::market::MarketStore;

#[derive(Debug, Clone, Serialize)]
pub struct Placed {
    pub order: Order,
    pub trade: Option<Trade>,
}

/// Look up a symbol's tradable price and lot size as exact decimals.
pub fn market_info(store: &MarketStore, symbol: &str) -> Result<MarketInfo, DomainError> {
    let symbol = order::normalize_symbol(symbol);
    if !store.contains(&symbol) {
        return Err(DomainError::UnknownSymbol(symbol));
    }
    let t = store.tradable(&symbol).ok_or_else(|| DomainError::NoPrice(symbol.clone()))?;
    let price = price_from_f64(t.price).ok_or_else(|| DomainError::NoPrice(symbol.clone()))?;
    let lot_size = price_from_f64(t.qty_step).unwrap_or(Decimal::ONE);
    Ok(MarketInfo { price, lot_size })
}

fn holding<'a>(hs: &'a [Holding], asset: &str) -> &'a Holding {
    hs.iter().find(|h| h.asset == asset).expect("holding row was created by lock_holdings")
}

fn filled_event(account_id: AccountId, o: &Order, price: Decimal) -> Event {
    Event::Account {
        account_id,
        change: AccountChange::OrderFilled {
            order_id: o.id.to_string(),
            symbol: o.symbol.clone(),
            side: o.side.to_string(),
            quantity: o.quantity.normalize().to_string(),
            price: price.normalize().to_string(),
        },
    }
}

/// Place an order on an account the caller has already authorized.
pub async fn place(
    conn: &mut PgConnection,
    account_id: AccountId,
    req: OrderRequest,
    market: MarketInfo,
    fees: FeeSchedule,
    actor: &Actor,
) -> ServiceResult<(Placed, Vec<Event>)> {
    let o = order::validate(req, market)?;
    accounts::lock_open(&mut *conn, account_id).await?.ok_or(ServiceError::Conflict("account is closed".into()))?;
    let hs = ledger::lock_holdings(&mut *conn, account_id, &[CASH, o.symbol.as_str()]).await?;
    let usd = holding(&hs, CASH);
    let pos = holding(&hs, &o.symbol);

    let marketable = is_marketable(o.side, o.limit_price, market.price);
    let position = PositionState { quantity: pos.quantity, cost_basis: pos.cost_basis };
    let reserve = match (o.side, marketable) {
        (Side::Buy, true) => {
            let needed = -compute_fill(Side::Buy, o.quantity, market.price, position, fees).cash_delta;
            let available = usd.quantity - usd.reserved;
            if needed > available {
                return Err(DomainError::InsufficientFunds { needed, available }.into());
            }
            Decimal::ZERO
        }
        (Side::Buy, false) => {
            let (_, needed) = order::reservation(&o, fees);
            let available = usd.quantity - usd.reserved;
            if needed > available {
                return Err(DomainError::InsufficientFunds { needed, available }.into());
            }
            needed
        }
        (Side::Sell, _) => {
            let available = pos.quantity - pos.reserved;
            if o.quantity > available {
                return Err(
                    DomainError::InsufficientQuantity { needed: o.quantity, available: available.normalize() }.into()
                );
            }
            if marketable { Decimal::ZERO } else { o.quantity }
        }
    };

    let mut order = orders::insert(
        &mut *conn,
        OrderId::new(),
        account_id,
        &o.symbol,
        o.side,
        o.kind,
        o.quantity,
        o.limit_price,
        reserve,
    )
    .await?;
    audit::record(
        &mut *conn,
        actor,
        Some(account_id),
        "order.placed",
        json!({ "order_id": order.id, "symbol": o.symbol, "side": o.side, "kind": o.kind,
                "quantity": o.quantity, "limit_price": o.limit_price, "market_price": market.price }),
    )
    .await?;

    let mut events =
        vec![Event::Account { account_id, change: AccountChange::OrderPlaced { order_id: order.id.to_string() } }];
    let trade = if marketable {
        let (filled, trade) = execute(conn, &order, market.price, position, fees).await?;
        events.push(filled_event(account_id, &filled, market.price));
        order = filled;
        Some(trade)
    } else {
        let asset = if o.side == Side::Buy { CASH } else { o.symbol.as_str() };
        ledger::adjust(&mut *conn, account_id, asset, Decimal::ZERO, reserve, Decimal::ZERO).await?;
        None
    };
    Ok((Placed { order, trade }, events))
}

/// Fill `order` in full at `price`. Caller holds the account + holdings locks.
async fn execute(
    conn: &mut PgConnection,
    order: &Order,
    price: Decimal,
    position: PositionState,
    fees: FeeSchedule,
) -> ServiceResult<(Order, Trade)> {
    let fill = compute_fill(order.side, order.quantity, price, position, fees);
    let account_id = order.account_id;

    // Release whatever the resting order was holding.
    if order.reserved > Decimal::ZERO {
        let asset = if order.side == Side::Buy { CASH } else { order.symbol.as_str() };
        ledger::adjust(&mut *conn, account_id, asset, Decimal::ZERO, -order.reserved, Decimal::ZERO).await?;
    }

    // Cash: account pays/receives; market is the counterparty; fees go to the fee book.
    let market_cash = match order.side {
        Side::Buy => fill.gross,
        Side::Sell => -fill.gross,
    };
    let postings = [
        Posting::new(Book::Account, CASH, fill.cash_delta),
        Posting::new(Book::Market, CASH, market_cash),
        Posting::new(Book::Fees, CASH, fill.fee),
        Posting::new(Book::Account, order.symbol.clone(), fill.quantity_delta),
        Posting::new(Book::Market, order.symbol.clone(), -fill.quantity_delta),
    ];
    let description =
        format!("{} {} {} @ {}", order.side, round_qty(order.quantity).normalize(), order.symbol, price.normalize());
    let entry =
        ledger::post(&mut *conn, account_id, EntryKind::Trade, Some(order.id.0), &description, &postings).await?;
    ledger::adjust(&mut *conn, account_id, &order.symbol, Decimal::ZERO, Decimal::ZERO, fill.cost_basis_delta).await?;

    let filled = orders::mark_filled(&mut *conn, order.id, price).await?;
    let trade = orders::insert_trade(&mut *conn, order, entry, price, fill.gross, fill.fee, fill.realized_pnl).await?;
    Ok((filled, trade))
}

/// Cancel an open order on an account the caller has already authorized.
pub async fn cancel(
    conn: &mut PgConnection,
    account_id: AccountId,
    order_id: OrderId,
    actor: &Actor,
) -> ServiceResult<(Order, Vec<Event>)> {
    accounts::lock_open(&mut *conn, account_id).await?.ok_or(ServiceError::Conflict("account is closed".into()))?;
    cancel_locked(conn, account_id, order_id, actor).await
}

/// Cancel with the account row already locked.
pub(super) async fn cancel_locked(
    conn: &mut PgConnection,
    account_id: AccountId,
    order_id: OrderId,
    actor: &Actor,
) -> ServiceResult<(Order, Vec<Event>)> {
    let order = orders::lock(&mut *conn, order_id)
        .await?
        .filter(|o| o.account_id == account_id)
        .ok_or(ServiceError::NotFound("order"))?;
    if order.status != OrderStatus::Open {
        return Err(ServiceError::Conflict(format!("order is already {}", order.status)));
    }
    if order.reserved > Decimal::ZERO {
        let asset = if order.side == Side::Buy { CASH } else { order.symbol.as_str() };
        ledger::lock_holdings(&mut *conn, account_id, &[asset]).await?;
        ledger::adjust(&mut *conn, account_id, asset, Decimal::ZERO, -order.reserved, Decimal::ZERO).await?;
    }
    let cancelled = orders::mark_cancelled(&mut *conn, order_id).await?;
    audit::record(&mut *conn, actor, Some(account_id), "order.cancelled", json!({ "order_id": order_id })).await?;
    Ok((
        cancelled,
        vec![Event::Account { account_id, change: AccountChange::OrderCancelled { order_id: order_id.to_string() } }],
    ))
}

/// Try to fill a resting limit order at `price` (called by the matcher).
/// Returns `None` if it no longer applies (filled/cancelled meanwhile, price moved).
pub async fn try_fill_resting(
    conn: &mut PgConnection,
    account_id: AccountId,
    order_id: OrderId,
    price: Decimal,
    fees: FeeSchedule,
) -> ServiceResult<Option<(Trade, Vec<Event>)>> {
    if accounts::lock_open(&mut *conn, account_id).await?.is_none() {
        return Ok(None);
    }
    let Some(order) = orders::lock(&mut *conn, order_id).await? else { return Ok(None) };
    if order.status != OrderStatus::Open
        || order.kind != OrderKind::Limit
        || !is_marketable(order.side, order.limit_price, price)
    {
        return Ok(None);
    }
    let hs = ledger::lock_holdings(&mut *conn, account_id, &[CASH, order.symbol.as_str()]).await?;
    let pos = holding(&hs, &order.symbol);
    let position = PositionState { quantity: pos.quantity, cost_basis: pos.cost_basis };
    let (filled, trade) = execute(conn, &order, price, position, fees).await?;
    let actor = Actor::default();
    audit::record(
        &mut *conn,
        &actor,
        Some(account_id),
        "order.filled",
        json!({ "order_id": order_id, "price": price, "by": "matcher" }),
    )
    .await?;
    Ok(Some((trade, vec![filled_event(account_id, &filled, price)])))
}
