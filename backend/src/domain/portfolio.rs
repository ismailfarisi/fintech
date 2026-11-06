//! Portfolio valuation from holdings and prices.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Serialize;

use super::money::{CASH, round_cash};

#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    pub asset: String,
    pub quantity: Decimal,
    pub reserved: Decimal,
    pub cost_basis: Decimal,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PositionView {
    pub symbol: String,
    pub quantity: Decimal,
    pub available: Decimal,
    pub avg_cost: Decimal,
    /// `None` while no price is known; then valued at cost.
    pub price: Option<Decimal>,
    pub market_value: Decimal,
    pub cost_basis: Decimal,
    pub unrealized_pnl: Decimal,
    pub unrealized_pnl_pct: Decimal,
    pub weight_pct: Decimal,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PortfolioSummary {
    pub cash: Decimal,
    pub buying_power: Decimal,
    pub positions_value: Decimal,
    pub equity: Decimal,
    pub net_contributions: Decimal,
    pub total_pnl: Decimal,
    pub total_pnl_pct: Decimal,
    pub realized_pnl: Decimal,
    pub unrealized_pnl: Decimal,
    pub positions: Vec<PositionView>,
}

fn pct(part: Decimal, whole: Decimal) -> Decimal {
    if whole.is_zero() { Decimal::ZERO } else { (part / whole * Decimal::ONE_HUNDRED).round_dp(4) }
}

pub fn summarize(
    holdings: &[Holding],
    prices: &BTreeMap<String, Decimal>,
    net_contributions: Decimal,
    realized_pnl: Decimal,
) -> PortfolioSummary {
    let mut cash = Decimal::ZERO;
    let mut reserved_cash = Decimal::ZERO;
    let mut positions = Vec::new();
    for h in holdings {
        if h.asset == CASH {
            cash = h.quantity;
            reserved_cash = h.reserved;
            continue;
        }
        if h.quantity.is_zero() {
            continue;
        }
        let price = prices.get(&h.asset).copied();
        let market_value = round_cash(price.map_or(h.cost_basis, |p| h.quantity * p));
        let cost_basis = round_cash(h.cost_basis);
        let unrealized = market_value - cost_basis;
        positions.push(PositionView {
            symbol: h.asset.clone(),
            quantity: h.quantity.normalize(),
            available: (h.quantity - h.reserved).normalize(),
            avg_cost: (h.cost_basis / h.quantity).round_dp(8).normalize(),
            price,
            market_value,
            cost_basis,
            unrealized_pnl: unrealized,
            unrealized_pnl_pct: pct(unrealized, cost_basis),
            weight_pct: Decimal::ZERO,
        });
    }
    let positions_value: Decimal = positions.iter().map(|p| p.market_value).sum();
    let unrealized_pnl: Decimal = positions.iter().map(|p| p.unrealized_pnl).sum();
    let equity = round_cash(cash + positions_value);
    for p in &mut positions {
        p.weight_pct = pct(p.market_value, equity);
    }
    positions.sort_by(|a, b| b.market_value.cmp(&a.market_value).then_with(|| a.symbol.cmp(&b.symbol)));
    let total_pnl = equity - net_contributions;
    PortfolioSummary {
        cash: round_cash(cash),
        buying_power: round_cash(cash - reserved_cash),
        positions_value: round_cash(positions_value),
        equity,
        net_contributions: round_cash(net_contributions),
        total_pnl: round_cash(total_pnl),
        total_pnl_pct: pct(total_pnl, net_contributions),
        realized_pnl: round_cash(realized_pnl),
        unrealized_pnl: round_cash(unrealized_pnl),
        positions,
    }
}

/// Equity only (for snapshots), avoiding the full summary.
pub fn equity(holdings: &[Holding], prices: &BTreeMap<String, Decimal>) -> Decimal {
    round_cash(
        holdings
            .iter()
            .map(|h| {
                if h.asset == CASH { h.quantity } else { prices.get(&h.asset).map_or(h.cost_basis, |p| h.quantity * p) }
            })
            .sum(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn h(asset: &str, q: Decimal, r: Decimal, c: Decimal) -> Holding {
        Holding { asset: asset.into(), quantity: q, reserved: r, cost_basis: c }
    }

    #[test]
    fn values_positions_and_pnl() {
        let holdings = vec![
            h("USD", dec!(8000), dec!(500), dec!(0)),
            h("NOVA", dec!(10), dec!(0), dec!(1000)),
            h("QBIT", dec!(20), dec!(5), dec!(1000)),
            h("GONE", dec!(0), dec!(0), dec!(0)),
        ];
        let prices = BTreeMap::from([("NOVA".to_string(), dec!(110)), ("QBIT".to_string(), dec!(45))]);
        let s = summarize(&holdings, &prices, dec!(10000), dec!(12.5));
        assert_eq!(s.cash, dec!(8000));
        assert_eq!(s.buying_power, dec!(7500));
        assert_eq!(s.positions_value, dec!(2000));
        assert_eq!(s.equity, dec!(10000));
        assert_eq!(s.total_pnl, dec!(0));
        assert_eq!(s.unrealized_pnl, dec!(0));
        assert_eq!(s.positions.len(), 2);
        assert_eq!(s.positions[0].symbol, "NOVA");
        assert_eq!(s.positions[0].unrealized_pnl, dec!(100));
        assert_eq!(s.positions[0].weight_pct, dec!(11));
        assert_eq!(s.positions[1].available, dec!(15));
        assert_eq!(equity(&holdings, &prices), dec!(10000));
    }

    #[test]
    fn unpriced_positions_are_valued_at_cost() {
        let holdings = vec![h("USD", dec!(0), dec!(0), dec!(0)), h("XYZ", dec!(2), dec!(0), dec!(50))];
        let s = summarize(&holdings, &BTreeMap::new(), dec!(50), dec!(0));
        assert_eq!(s.positions[0].price, None);
        assert_eq!(s.equity, dec!(50));
        assert_eq!(s.total_pnl_pct, dec!(0));
    }
}
