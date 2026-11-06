//! Paper-trading engine: cash, positions, order lifecycle, P&L.
//!
//! Pure logic with no I/O — prices and timestamps are passed in, which keeps
//! it deterministic and easy to test.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::market::round_cents;

pub const DEFAULT_STARTING_CASH: f64 = 100_000.0;
const EQUITY_HISTORY_LEN: usize = 2_000;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKind {
    Market,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    Open,
    Filled,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: Uuid,
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub qty: u64,
    pub limit_price: Option<f64>,
    pub status: OrderStatus,
    pub created_at: DateTime<Utc>,
    pub filled_at: Option<DateTime<Utc>>,
    pub fill_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: Uuid,
    pub order_id: Uuid,
    pub symbol: String,
    pub side: Side,
    pub qty: u64,
    pub price: f64,
    pub total: f64,
    /// Realised P&L for sells; `None` for buys.
    pub realized_pnl: Option<f64>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub qty: u64,
    pub avg_cost: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CashFlowKind {
    Deposit,
    Withdrawal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CashFlow {
    pub id: Uuid,
    pub kind: CashFlowKind,
    pub amount: f64,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EquityPoint {
    pub t: DateTime<Utc>,
    pub equity: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub qty: u64,
    pub limit_price: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    InvalidQuantity,
    InvalidLimitPrice,
    UnknownSymbol(String),
    InsufficientFunds { needed: f64, available: f64 },
    InsufficientShares { needed: u64, available: u64 },
    InvalidAmount,
    OrderNotFound,
    OrderNotOpen,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuantity => write!(f, "quantity must be greater than zero"),
            Self::InvalidLimitPrice => write!(f, "limit orders need a positive limit price"),
            Self::UnknownSymbol(s) => write!(f, "unknown symbol '{s}'"),
            Self::InsufficientFunds { needed, available } => write!(
                f,
                "insufficient buying power: need ${needed:.2}, have ${available:.2}"
            ),
            Self::InsufficientShares { needed, available } => write!(
                f,
                "insufficient shares: need {needed}, have {available} available to sell"
            ),
            Self::InvalidAmount => write!(f, "amount must be a positive number"),
            Self::OrderNotFound => write!(f, "order not found"),
            Self::OrderNotOpen => write!(f, "order is no longer open"),
        }
    }
}

impl std::error::Error for EngineError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub cash: f64,
    /// Total money put in minus money taken out (the P&L baseline).
    pub net_contributions: f64,
    pub realized_pnl: f64,
    pub positions: BTreeMap<String, Position>,
    pub orders: Vec<Order>,
    pub trades: Vec<Trade>,
    pub cash_flows: Vec<CashFlow>,
    pub watchlist: Vec<String>,
    pub equity_history: VecDeque<EquityPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionView {
    pub symbol: String,
    pub qty: u64,
    pub avg_cost: f64,
    pub price: f64,
    pub market_value: f64,
    pub cost_basis: f64,
    pub unrealized_pnl: f64,
    pub unrealized_pnl_pct: f64,
    /// Share of total equity, in percent.
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortfolioSummary {
    pub cash: f64,
    pub buying_power: f64,
    pub positions_value: f64,
    pub equity: f64,
    pub net_contributions: f64,
    pub total_pnl: f64,
    pub total_pnl_pct: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub positions: Vec<PositionView>,
}

impl Default for Account {
    fn default() -> Self {
        Self::new(DEFAULT_STARTING_CASH, Utc::now())
    }
}

impl Account {
    pub fn new(starting_cash: f64, now: DateTime<Utc>) -> Self {
        let mut a = Account {
            cash: 0.0,
            net_contributions: 0.0,
            realized_pnl: 0.0,
            positions: BTreeMap::new(),
            orders: Vec::new(),
            trades: Vec::new(),
            cash_flows: Vec::new(),
            watchlist: vec!["NOVA".into(), "QBIT".into(), "BTCX".into()],
            equity_history: VecDeque::new(),
        };
        if starting_cash > 0.0 {
            a.deposit(starting_cash, now).expect("positive deposit");
        }
        a
    }

    // ---- cash ------------------------------------------------------------

    pub fn deposit(&mut self, amount: f64, now: DateTime<Utc>) -> Result<(), EngineError> {
        if !(amount.is_finite() && amount > 0.0) {
            return Err(EngineError::InvalidAmount);
        }
        let amount = round_cents(amount);
        self.cash = round_cents(self.cash + amount);
        self.net_contributions = round_cents(self.net_contributions + amount);
        self.cash_flows.push(CashFlow { id: Uuid::new_v4(), kind: CashFlowKind::Deposit, amount, at: now });
        Ok(())
    }

    pub fn withdraw(&mut self, amount: f64, now: DateTime<Utc>) -> Result<(), EngineError> {
        if !(amount.is_finite() && amount > 0.0) {
            return Err(EngineError::InvalidAmount);
        }
        let amount = round_cents(amount);
        let available = self.buying_power();
        if amount > available + EPS {
            return Err(EngineError::InsufficientFunds { needed: amount, available });
        }
        self.cash = round_cents(self.cash - amount);
        self.net_contributions = round_cents(self.net_contributions - amount);
        self.cash_flows.push(CashFlow { id: Uuid::new_v4(), kind: CashFlowKind::Withdrawal, amount, at: now });
        Ok(())
    }

    /// Cash committed to resting buy-limit orders.
    pub fn reserved_cash(&self) -> f64 {
        self.open_orders()
            .filter(|o| o.side == Side::Buy)
            .map(|o| o.qty as f64 * o.limit_price.unwrap_or(0.0))
            .sum()
    }

    /// Cash available for new buys / withdrawals.
    pub fn buying_power(&self) -> f64 {
        round_cents((self.cash - self.reserved_cash()).max(0.0))
    }

    /// Shares of `symbol` held but not committed to resting sell orders.
    pub fn sellable_qty(&self, symbol: &str) -> u64 {
        let held = self.positions.get(symbol).map_or(0, |p| p.qty);
        let reserved: u64 = self
            .open_orders()
            .filter(|o| o.side == Side::Sell && o.symbol == symbol)
            .map(|o| o.qty)
            .sum();
        held.saturating_sub(reserved)
    }

    pub fn open_orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.iter().filter(|o| o.status == OrderStatus::Open)
    }

    // ---- orders ----------------------------------------------------------

    /// Place an order. `market_price` is the current price of the symbol, or
    /// `None` if the symbol isn't tradable. Returns the order and any trade
    /// produced by an immediate fill.
    pub fn place_order(
        &mut self,
        req: OrderRequest,
        market_price: Option<f64>,
        now: DateTime<Utc>,
    ) -> Result<(Order, Option<Trade>), EngineError> {
        let symbol = req.symbol.trim().to_uppercase();
        let price = market_price.ok_or_else(|| EngineError::UnknownSymbol(symbol.clone()))?;
        if req.qty == 0 {
            return Err(EngineError::InvalidQuantity);
        }
        let limit_price = match req.kind {
            OrderKind::Market => None,
            OrderKind::Limit => match req.limit_price {
                Some(p) if p.is_finite() && p > 0.0 => Some(round_cents(p)),
                _ => return Err(EngineError::InvalidLimitPrice),
            },
        };

        // Does it execute right away? Market orders always do; limit orders
        // do when the market is already at or through the limit.
        let marketable = match (req.kind, req.side, limit_price) {
            (OrderKind::Market, _, _) => true,
            (_, Side::Buy, Some(l)) => price <= l + EPS,
            (_, Side::Sell, Some(l)) => price + EPS >= l,
            _ => unreachable!(),
        };

        match req.side {
            Side::Buy => {
                // A resting limit reserves qty * limit; an immediate fill costs qty * price.
                let per_share = if marketable { price } else { limit_price.unwrap() };
                let needed = round_cents(req.qty as f64 * per_share);
                let available = self.buying_power();
                if needed > available + EPS {
                    return Err(EngineError::InsufficientFunds { needed, available });
                }
            }
            Side::Sell => {
                let available = self.sellable_qty(&symbol);
                if req.qty > available {
                    return Err(EngineError::InsufficientShares { needed: req.qty, available });
                }
            }
        }

        let mut order = Order {
            id: Uuid::new_v4(),
            symbol,
            side: req.side,
            kind: req.kind,
            qty: req.qty,
            limit_price,
            status: OrderStatus::Open,
            created_at: now,
            filled_at: None,
            fill_price: None,
        };
        let trade = marketable.then(|| self.fill(&mut order, price, now));
        self.orders.push(order.clone());
        Ok((order, trade))
    }

    pub fn cancel_order(&mut self, id: Uuid) -> Result<Order, EngineError> {
        let order = self.orders.iter_mut().find(|o| o.id == id).ok_or(EngineError::OrderNotFound)?;
        if order.status != OrderStatus::Open {
            return Err(EngineError::OrderNotOpen);
        }
        order.status = OrderStatus::Cancelled;
        Ok(order.clone())
    }

    /// Match resting limit orders against new prices. Returns the resulting trades.
    pub fn on_prices(&mut self, prices: &BTreeMap<String, f64>, now: DateTime<Utc>) -> Vec<Trade> {
        let mut trades = Vec::new();
        for i in 0..self.orders.len() {
            let o = &self.orders[i];
            if o.status != OrderStatus::Open {
                continue;
            }
            let (Some(&price), Some(limit)) = (prices.get(&o.symbol), o.limit_price) else {
                continue;
            };
            let crosses = match o.side {
                Side::Buy => price <= limit + EPS,
                Side::Sell => price + EPS >= limit,
            };
            if !crosses {
                continue;
            }
            // Sells can become unfillable if shares were sold elsewhere — sellable_qty
            // reservations prevent that, but guard anyway.
            if o.side == Side::Sell && self.positions.get(&o.symbol).map_or(0, |p| p.qty) < o.qty {
                self.orders[i].status = OrderStatus::Cancelled;
                continue;
            }
            let mut order = self.orders[i].clone();
            trades.push(self.fill(&mut order, price, now));
            self.orders[i] = order;
        }
        trades
    }

    /// Execute `order` in full at `price`, updating cash and positions.
    fn fill(&mut self, order: &mut Order, price: f64, now: DateTime<Utc>) -> Trade {
        let total = round_cents(order.qty as f64 * price);
        let mut realized = None;
        match order.side {
            Side::Buy => {
                self.cash = round_cents(self.cash - total);
                let pos = self
                    .positions
                    .entry(order.symbol.clone())
                    .or_insert(Position { qty: 0, avg_cost: 0.0 });
                let new_qty = pos.qty + order.qty;
                pos.avg_cost = (pos.qty as f64 * pos.avg_cost + total) / new_qty as f64;
                pos.qty = new_qty;
            }
            Side::Sell => {
                self.cash = round_cents(self.cash + total);
                let pos = self.positions.get_mut(&order.symbol).expect("validated position");
                let pnl = round_cents((price - pos.avg_cost) * order.qty as f64);
                self.realized_pnl = round_cents(self.realized_pnl + pnl);
                realized = Some(pnl);
                pos.qty -= order.qty;
                if pos.qty == 0 {
                    self.positions.remove(&order.symbol);
                }
            }
        }
        order.status = OrderStatus::Filled;
        order.filled_at = Some(now);
        order.fill_price = Some(price);
        let trade = Trade {
            id: Uuid::new_v4(),
            order_id: order.id,
            symbol: order.symbol.clone(),
            side: order.side,
            qty: order.qty,
            price,
            total,
            realized_pnl: realized,
            at: now,
        };
        self.trades.push(trade.clone());
        trade
    }

    // ---- reporting -------------------------------------------------------

    pub fn summary(&self, prices: &BTreeMap<String, f64>) -> PortfolioSummary {
        let mut positions: Vec<PositionView> = self
            .positions
            .iter()
            .map(|(sym, p)| {
                let price = prices.get(sym).copied().unwrap_or(p.avg_cost);
                let market_value = round_cents(p.qty as f64 * price);
                let cost_basis = round_cents(p.qty as f64 * p.avg_cost);
                let unrealized = round_cents(market_value - cost_basis);
                PositionView {
                    symbol: sym.clone(),
                    qty: p.qty,
                    avg_cost: p.avg_cost,
                    price,
                    market_value,
                    cost_basis,
                    unrealized_pnl: unrealized,
                    unrealized_pnl_pct: if cost_basis > 0.0 { unrealized / cost_basis * 100.0 } else { 0.0 },
                    weight: 0.0,
                }
            })
            .collect();
        let positions_value = round_cents(positions.iter().map(|p| p.market_value).sum());
        let unrealized_pnl = round_cents(positions.iter().map(|p| p.unrealized_pnl).sum());
        let equity = round_cents(self.cash + positions_value);
        for p in &mut positions {
            p.weight = if equity > 0.0 { p.market_value / equity * 100.0 } else { 0.0 };
        }
        positions.sort_by(|a, b| b.market_value.total_cmp(&a.market_value));
        let total_pnl = round_cents(equity - self.net_contributions);
        PortfolioSummary {
            cash: self.cash,
            buying_power: self.buying_power(),
            positions_value,
            equity,
            net_contributions: self.net_contributions,
            total_pnl,
            total_pnl_pct: if self.net_contributions > 0.0 { total_pnl / self.net_contributions * 100.0 } else { 0.0 },
            realized_pnl: self.realized_pnl,
            unrealized_pnl,
            positions,
        }
    }

    pub fn record_equity(&mut self, prices: &BTreeMap<String, f64>, now: DateTime<Utc>) {
        let equity = self.summary(prices).equity;
        if self.equity_history.len() == EQUITY_HISTORY_LEN {
            self.equity_history.pop_front();
        }
        self.equity_history.push_back(EquityPoint { t: now, equity });
    }

    // ---- watchlist -------------------------------------------------------

    pub fn add_to_watchlist(&mut self, symbol: &str) {
        let s = symbol.to_uppercase();
        if !self.watchlist.contains(&s) {
            self.watchlist.push(s);
        }
    }

    pub fn remove_from_watchlist(&mut self, symbol: &str) {
        let s = symbol.to_uppercase();
        self.watchlist.retain(|w| *w != s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn acct(cash: f64) -> Account {
        Account::new(cash, now())
    }

    fn market(sym: &str, side: Side, qty: u64) -> OrderRequest {
        OrderRequest { symbol: sym.into(), side, kind: OrderKind::Market, qty, limit_price: None }
    }

    fn limit(sym: &str, side: Side, qty: u64, px: f64) -> OrderRequest {
        OrderRequest { symbol: sym.into(), side, kind: OrderKind::Limit, qty, limit_price: Some(px) }
    }

    fn prices(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(s, p)| (s.to_string(), *p)).collect()
    }

    #[test]
    fn market_buy_updates_cash_and_position() {
        let mut a = acct(10_000.0);
        let (o, t) = a.place_order(market("NOVA", Side::Buy, 10), Some(100.0), now()).unwrap();
        assert_eq!(o.status, OrderStatus::Filled);
        assert_eq!(t.unwrap().total, 1000.0);
        assert_eq!(a.cash, 9_000.0);
        assert_eq!(a.positions["NOVA"].qty, 10);
        assert_eq!(a.positions["NOVA"].avg_cost, 100.0);
    }

    #[test]
    fn average_cost_is_weighted() {
        let mut a = acct(10_000.0);
        a.place_order(market("NOVA", Side::Buy, 10), Some(100.0), now()).unwrap();
        a.place_order(market("NOVA", Side::Buy, 30), Some(120.0), now()).unwrap();
        assert_eq!(a.positions["NOVA"].qty, 40);
        assert!((a.positions["NOVA"].avg_cost - 115.0).abs() < 1e-9);
    }

    #[test]
    fn sell_realizes_pnl_and_closes_position() {
        let mut a = acct(10_000.0);
        a.place_order(market("NOVA", Side::Buy, 10), Some(100.0), now()).unwrap();
        let (_, t) = a.place_order(market("NOVA", Side::Sell, 10), Some(110.0), now()).unwrap();
        assert_eq!(t.unwrap().realized_pnl, Some(100.0));
        assert_eq!(a.realized_pnl, 100.0);
        assert_eq!(a.cash, 10_100.0);
        assert!(!a.positions.contains_key("NOVA"));
    }

    #[test]
    fn rejects_overspend_and_oversell() {
        let mut a = acct(1_000.0);
        let err = a.place_order(market("NOVA", Side::Buy, 11), Some(100.0), now()).unwrap_err();
        assert!(matches!(err, EngineError::InsufficientFunds { .. }));
        let err = a.place_order(market("NOVA", Side::Sell, 1), Some(100.0), now()).unwrap_err();
        assert_eq!(err, EngineError::InsufficientShares { needed: 1, available: 0 });
        assert!(a.orders.is_empty(), "rejected orders are not recorded");
    }

    #[test]
    fn rejects_bad_inputs() {
        let mut a = acct(1_000.0);
        assert_eq!(
            a.place_order(market("NOVA", Side::Buy, 0), Some(10.0), now()).unwrap_err(),
            EngineError::InvalidQuantity
        );
        assert_eq!(
            a.place_order(limit("NOVA", Side::Buy, 1, -5.0), Some(10.0), now()).unwrap_err(),
            EngineError::InvalidLimitPrice
        );
        assert!(matches!(
            a.place_order(market("NOPE", Side::Buy, 1), None, now()).unwrap_err(),
            EngineError::UnknownSymbol(_)
        ));
    }

    #[test]
    fn resting_buy_limit_reserves_cash_then_fills_when_price_drops() {
        let mut a = acct(1_000.0);
        let (o, t) = a.place_order(limit("NOVA", Side::Buy, 5, 90.0), Some(100.0), now()).unwrap();
        assert!(t.is_none());
        assert_eq!(o.status, OrderStatus::Open);
        assert_eq!(a.buying_power(), 550.0);
        // Can't spend the reserved cash elsewhere.
        assert!(a.place_order(market("QBIT", Side::Buy, 6), Some(100.0), now()).is_err());

        // Price above the limit: nothing happens.
        assert!(a.on_prices(&prices(&[("NOVA", 95.0)]), now()).is_empty());
        // Price falls through the limit: fills at the (better) market price.
        let trades = a.on_prices(&prices(&[("NOVA", 89.0)]), now());
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].price, 89.0);
        assert_eq!(a.cash, 1_000.0 - 445.0);
        assert_eq!(a.buying_power(), 555.0);
        assert_eq!(a.open_orders().count(), 0);
    }

    #[test]
    fn marketable_limit_fills_immediately() {
        let mut a = acct(1_000.0);
        let (o, t) = a.place_order(limit("NOVA", Side::Buy, 1, 120.0), Some(100.0), now()).unwrap();
        assert_eq!(o.status, OrderStatus::Filled);
        assert_eq!(t.unwrap().price, 100.0);
    }

    #[test]
    fn sell_limit_reserves_shares_and_can_be_cancelled() {
        let mut a = acct(1_000.0);
        a.place_order(market("NOVA", Side::Buy, 5), Some(100.0), now()).unwrap();
        let (o, _) = a.place_order(limit("NOVA", Side::Sell, 3, 150.0), Some(100.0), now()).unwrap();
        assert_eq!(a.sellable_qty("NOVA"), 2);
        assert!(a.place_order(market("NOVA", Side::Sell, 3), Some(100.0), now()).is_err());

        a.cancel_order(o.id).unwrap();
        assert_eq!(a.sellable_qty("NOVA"), 5);
        assert_eq!(a.cancel_order(o.id).unwrap_err(), EngineError::OrderNotOpen);
        assert_eq!(a.cancel_order(Uuid::new_v4()).unwrap_err(), EngineError::OrderNotFound);
    }

    #[test]
    fn summary_reports_equity_and_pnl() {
        let mut a = acct(10_000.0);
        a.place_order(market("NOVA", Side::Buy, 10), Some(100.0), now()).unwrap();
        a.place_order(market("QBIT", Side::Buy, 20), Some(50.0), now()).unwrap();
        let s = a.summary(&prices(&[("NOVA", 110.0), ("QBIT", 45.0)]));
        assert_eq!(s.cash, 8_000.0);
        assert_eq!(s.positions_value, 1_100.0 + 900.0);
        assert_eq!(s.equity, 10_000.0);
        assert_eq!(s.unrealized_pnl, 0.0);
        assert_eq!(s.total_pnl, 0.0);
        assert_eq!(s.positions[0].symbol, "NOVA", "sorted by market value");
        assert_eq!(s.positions[0].unrealized_pnl, 100.0);
        assert!((s.positions[0].weight - 11.0).abs() < 1e-9);
    }

    #[test]
    fn deposits_and_withdrawals_move_pnl_baseline() {
        let mut a = acct(1_000.0);
        a.deposit(500.0, now()).unwrap();
        a.withdraw(200.0, now()).unwrap();
        assert_eq!(a.cash, 1_300.0);
        assert_eq!(a.net_contributions, 1_300.0);
        assert_eq!(a.summary(&BTreeMap::new()).total_pnl, 0.0);
        assert!(a.withdraw(5_000.0, now()).is_err());
        assert_eq!(a.deposit(-1.0, now()).unwrap_err(), EngineError::InvalidAmount);
        assert_eq!(a.deposit(f64::NAN, now()).unwrap_err(), EngineError::InvalidAmount);
    }

    #[test]
    fn watchlist_is_deduplicated() {
        let mut a = acct(0.0);
        a.watchlist.clear();
        a.add_to_watchlist("nova");
        a.add_to_watchlist("NOVA");
        assert_eq!(a.watchlist, vec!["NOVA"]);
        a.remove_from_watchlist("Nova");
        assert!(a.watchlist.is_empty());
    }

    #[test]
    fn account_round_trips_through_json() {
        let mut a = acct(1_000.0);
        a.place_order(market("NOVA", Side::Buy, 2), Some(100.0), now()).unwrap();
        let json = serde_json::to_string(&a).unwrap();
        let b: Account = serde_json::from_str(&json).unwrap();
        assert_eq!(b.cash, a.cash);
        assert_eq!(b.positions["NOVA"].qty, 2);
        assert_eq!(b.trades.len(), 1);
    }
}
