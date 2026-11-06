//! Order validation, reservations and fill math.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::DomainError;
use super::money::{self, CASH, is_multiple_of, round_cash, round_cash_up, round_qty};

macro_rules! text_enum {
    ($name:ident { $($variant:ident => $text:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
        #[serde(rename_all = "lowercase")]
        #[sqlx(type_name = "text", rename_all = "lowercase")]
        pub enum $name { $($variant),* }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),* }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s { $($text => Ok(Self::$variant),)* other => Err(format!("invalid {}: {other}", stringify!($name))) }
            }
        }
    };
}

text_enum!(Side { Buy => "buy", Sell => "sell" });
text_enum!(OrderKind { Market => "market", Limit => "limit" });
text_enum!(OrderStatus { Open => "open", Filled => "filled", Cancelled => "cancelled" });

/// What the market currently says about a symbol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarketInfo {
    pub price: Decimal,
    pub lot_size: Decimal,
}

/// A client's order request, before validation.
#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub quantity: Decimal,
    pub limit_price: Option<Decimal>,
}

/// An order that passed all stateless checks.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedOrder {
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub quantity: Decimal,
    pub limit_price: Option<Decimal>,
}

/// Fees charged on fills, in basis points of gross value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeeSchedule {
    pub bps: u32,
}

impl FeeSchedule {
    pub fn fee(&self, gross: Decimal) -> Decimal {
        round_cash(gross * money::bps(self.bps))
    }
}

pub fn normalize_symbol(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

pub fn validate(req: OrderRequest, market: MarketInfo) -> Result<ValidatedOrder, DomainError> {
    let quantity = req.quantity;
    if quantity <= Decimal::ZERO {
        return Err(DomainError::InvalidQuantity);
    }
    if quantity > money::max_quantity() {
        return Err(DomainError::QuantityTooLarge);
    }
    if round_qty(quantity) != quantity || !is_multiple_of(quantity, market.lot_size) {
        return Err(DomainError::QuantityIncrement(market.lot_size.normalize()));
    }
    let limit_price = match (req.kind, req.limit_price) {
        (OrderKind::Market, None) => None,
        (OrderKind::Market, Some(_)) => return Err(DomainError::UnexpectedLimitPrice),
        (OrderKind::Limit, Some(p)) if p > Decimal::ZERO => {
            if p > money::max_price() {
                return Err(DomainError::PriceTooLarge);
            }
            Some(round_qty(p))
        }
        (OrderKind::Limit, _) => return Err(DomainError::InvalidLimitPrice),
    };
    Ok(ValidatedOrder { symbol: normalize_symbol(&req.symbol), side: req.side, kind: req.kind, quantity, limit_price })
}

/// Would this order execute immediately at `price`?
pub fn is_marketable(side: Side, limit_price: Option<Decimal>, price: Decimal) -> bool {
    match (side, limit_price) {
        (_, None) => true,
        (Side::Buy, Some(l)) => price <= l,
        (Side::Sell, Some(l)) => price >= l,
    }
}

/// Asset and amount a resting order must hold: cash for buys (at the limit,
/// fees included, rounded up), units for sells.
pub fn reservation(order: &ValidatedOrder, fees: FeeSchedule) -> (String, Decimal) {
    match order.side {
        Side::Buy => {
            let limit = order.limit_price.expect("resting orders are limit orders");
            let gross = order.quantity * limit;
            (CASH.to_string(), round_cash_up(gross + gross * money::bps(fees.bps)))
        }
        Side::Sell => (order.symbol.clone(), order.quantity),
    }
}

/// A position before a fill.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PositionState {
    pub quantity: Decimal,
    pub cost_basis: Decimal,
}

/// Everything a fill changes. Positive deltas increase the account's holdings.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub gross: Decimal,
    pub fee: Decimal,
    pub cash_delta: Decimal,
    pub quantity_delta: Decimal,
    pub cost_basis_delta: Decimal,
    pub realized_pnl: Option<Decimal>,
}

/// Compute a full fill of `quantity` at `price` against `position`.
pub fn compute_fill(side: Side, quantity: Decimal, price: Decimal, position: PositionState, fees: FeeSchedule) -> Fill {
    let gross = round_cash(quantity * price);
    let fee = fees.fee(gross);
    match side {
        Side::Buy => Fill {
            gross,
            fee,
            cash_delta: -(gross + fee),
            quantity_delta: quantity,
            // Fees are part of what the position cost.
            cost_basis_delta: gross + fee,
            realized_pnl: None,
        },
        Side::Sell => {
            // Remove basis pro rata; selling everything removes all of it, so no dust remains.
            let removed = if quantity >= position.quantity || position.quantity.is_zero() {
                position.cost_basis
            } else {
                round_qty(position.cost_basis * quantity / position.quantity)
            };
            let proceeds = gross - fee;
            Fill {
                gross,
                fee,
                cash_delta: proceeds,
                quantity_delta: -quantity,
                cost_basis_delta: -removed,
                realized_pnl: Some(round_cash(proceeds - removed)),
            }
        }
    }
}

/// Validate and round a deposit/withdrawal amount.
pub fn validate_cash_amount(amount: Decimal) -> Result<Decimal, DomainError> {
    if amount <= Decimal::ZERO || round_cash(amount) != amount {
        return Err(DomainError::InvalidAmount);
    }
    if amount > money::max_cash_amount() {
        return Err(DomainError::AmountTooLarge);
    }
    Ok(amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn req(side: Side, kind: OrderKind, qty: Decimal, limit: Option<Decimal>) -> OrderRequest {
        OrderRequest { symbol: " btc-usd ".into(), side, kind, quantity: qty, limit_price: limit }
    }
    const BTC: MarketInfo = MarketInfo { price: dec!(75000), lot_size: dec!(0.00000001) };
    const STOCK: MarketInfo = MarketInfo { price: dec!(100), lot_size: dec!(1) };

    #[test]
    fn validation() {
        let ok = validate(req(Side::Buy, OrderKind::Market, dec!(0.0125), None), BTC).unwrap();
        assert_eq!(ok.symbol, "BTC-USD");
        use DomainError::*;
        let e = |r| validate(r, STOCK).unwrap_err();
        assert_eq!(e(req(Side::Buy, OrderKind::Market, dec!(0), None)), InvalidQuantity);
        assert_eq!(e(req(Side::Buy, OrderKind::Market, dec!(-1), None)), InvalidQuantity);
        assert_eq!(e(req(Side::Buy, OrderKind::Market, dec!(1.5), None)), QuantityIncrement(dec!(1)));
        assert_eq!(e(req(Side::Buy, OrderKind::Market, dec!(1e13), None)), QuantityTooLarge);
        assert_eq!(e(req(Side::Buy, OrderKind::Market, dec!(1), Some(dec!(5)))), UnexpectedLimitPrice);
        assert_eq!(e(req(Side::Buy, OrderKind::Limit, dec!(1), None)), InvalidLimitPrice);
        assert_eq!(e(req(Side::Buy, OrderKind::Limit, dec!(1), Some(dec!(-5)))), InvalidLimitPrice);
        assert_eq!(e(req(Side::Buy, OrderKind::Limit, dec!(1), Some(dec!(1e12)))), PriceTooLarge);
        // More than 8 decimals is rejected rather than silently rounded.
        assert!(validate(req(Side::Buy, OrderKind::Market, dec!(0.000000001), None), BTC).is_err());
    }

    #[test]
    fn marketability() {
        assert!(is_marketable(Side::Buy, None, dec!(100)));
        assert!(is_marketable(Side::Buy, Some(dec!(100)), dec!(99)));
        assert!(!is_marketable(Side::Buy, Some(dec!(100)), dec!(101)));
        assert!(is_marketable(Side::Sell, Some(dec!(100)), dec!(100)));
        assert!(!is_marketable(Side::Sell, Some(dec!(100)), dec!(99.99)));
    }

    #[test]
    fn reservation_covers_cost_including_fees() {
        let o = validate(req(Side::Buy, OrderKind::Limit, dec!(3), Some(dec!(33.333))), STOCK).unwrap();
        let (asset, amt) = reservation(&o, FeeSchedule { bps: 10 });
        assert_eq!(asset, "USD");
        // 99.999 + 0.099999 = 100.098999 -> rounded up
        assert_eq!(amt, dec!(100.10));
        let fill = compute_fill(Side::Buy, dec!(3), dec!(33.333), PositionState::default(), FeeSchedule { bps: 10 });
        assert!(-fill.cash_delta <= amt);
    }

    #[test]
    fn buy_then_sell_pnl() {
        let fees = FeeSchedule::default();
        let b1 = compute_fill(Side::Buy, dec!(10), dec!(100), PositionState::default(), fees);
        assert_eq!((b1.cash_delta, b1.cost_basis_delta), (dec!(-1000), dec!(1000)));
        let pos = PositionState { quantity: dec!(40), cost_basis: dec!(4600) }; // avg 115
        let s = compute_fill(Side::Sell, dec!(10), dec!(120), pos, fees);
        assert_eq!(s.cash_delta, dec!(1200));
        assert_eq!(s.cost_basis_delta, dec!(-1150));
        assert_eq!(s.realized_pnl, Some(dec!(50)));
        // Selling the rest removes all basis.
        let s2 = compute_fill(
            Side::Sell,
            dec!(30),
            dec!(100),
            PositionState { quantity: dec!(30), cost_basis: dec!(3450) },
            fees,
        );
        assert_eq!(s2.cost_basis_delta, dec!(-3450));
        assert_eq!(s2.realized_pnl, Some(dec!(-450)));
    }

    #[test]
    fn fees_reduce_proceeds_and_increase_basis() {
        let fees = FeeSchedule { bps: 25 };
        let b = compute_fill(Side::Buy, dec!(1), dec!(1000), PositionState::default(), fees);
        assert_eq!((b.fee, b.cash_delta, b.cost_basis_delta), (dec!(2.50), dec!(-1002.50), dec!(1002.50)));
        let s = compute_fill(
            Side::Sell,
            dec!(1),
            dec!(1000),
            PositionState { quantity: dec!(1), cost_basis: dec!(1002.50) },
            fees,
        );
        assert_eq!(s.realized_pnl, Some(dec!(-5.00)));
    }

    #[test]
    fn cash_amounts() {
        assert_eq!(validate_cash_amount(dec!(10.50)), Ok(dec!(10.50)));
        assert_eq!(validate_cash_amount(dec!(10.505)), Err(DomainError::InvalidAmount));
        assert_eq!(validate_cash_amount(dec!(0)), Err(DomainError::InvalidAmount));
        assert_eq!(validate_cash_amount(dec!(2e9)), Err(DomainError::AmountTooLarge));
    }

    #[test]
    fn enums_round_trip_as_text() {
        for s in ["buy", "sell"] {
            assert_eq!(s.parse::<Side>().unwrap().as_str(), s);
        }
        assert!("BUY".parse::<Side>().is_err());
        assert_eq!("cancelled".parse::<OrderStatus>().unwrap(), OrderStatus::Cancelled);
    }
}
