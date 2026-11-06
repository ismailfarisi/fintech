//! Decimal helpers and limits.

use rust_decimal::prelude::*;
use rust_decimal::{Decimal, RoundingStrategy};

/// The account currency.
pub const CASH: &str = "USD";
/// Cash is kept in cents.
pub const CASH_SCALE: u32 = 2;
/// Quantities and prices are kept to 8 decimals (satoshi precision).
pub const QTY_SCALE: u32 = 8;

/// Hard upper bounds: reject absurd inputs before they reach arithmetic.
pub fn max_quantity() -> Decimal {
    Decimal::from(1_000_000_000_000i64)
}
pub fn max_price() -> Decimal {
    Decimal::from(100_000_000_000i64)
}
pub fn max_cash_amount() -> Decimal {
    Decimal::from(1_000_000_000i64)
}

/// Round cash to cents, half away from zero.
pub fn round_cash(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(CASH_SCALE, RoundingStrategy::MidpointAwayFromZero).normalize_zero()
}

/// Round cash *up* to the next cent (used for reservations so they always cover the cost).
pub fn round_cash_up(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(CASH_SCALE, RoundingStrategy::AwayFromZero).normalize_zero()
}

pub fn round_qty(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(QTY_SCALE, RoundingStrategy::MidpointAwayFromZero).normalize_zero()
}

/// Convert a market-data observation into an exact price (8 dp).
/// Returns `None` for non-finite or non-positive input.
pub fn price_from_f64(p: f64) -> Option<Decimal> {
    if !(p.is_finite() && p > 0.0) {
        return None;
    }
    Decimal::from_f64(p).map(round_qty).filter(|d| d.is_sign_positive() && !d.is_zero())
}

trait NormalizeZero {
    fn normalize_zero(self) -> Self;
}

impl NormalizeZero for Decimal {
    /// Avoid "-0.00" in output.
    fn normalize_zero(self) -> Self {
        if self.is_zero() { Decimal::new(0, self.scale()) } else { self }
    }
}

/// Is `value` an exact multiple of `step`?
pub fn is_multiple_of(value: Decimal, step: Decimal) -> bool {
    step > Decimal::ZERO && (value % step).is_zero()
}

/// Basis points as a multiplier, e.g. 25 bps -> 0.0025.
pub fn bps(bps: u32) -> Decimal {
    Decimal::from(bps) / Decimal::from(10_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn rounding() {
        assert_eq!(round_cash(dec!(1.005)), dec!(1.01));
        assert_eq!(round_cash(dec!(-1.005)), dec!(-1.01));
        assert_eq!(round_cash_up(dec!(1.001)), dec!(1.01));
        assert_eq!(round_cash(dec!(-0.001)).to_string(), "0.00");
        assert_eq!(round_qty(dec!(0.123456789)), dec!(0.12345679));
    }

    #[test]
    fn f64_boundary_is_exact_for_typical_prices() {
        assert_eq!(price_from_f64(75786.03), Some(dec!(75786.03)));
        assert_eq!(price_from_f64(0.0726), Some(dec!(0.0726)));
        assert_eq!(price_from_f64(0.1 + 0.2), Some(dec!(0.3)));
        assert_eq!(price_from_f64(f64::NAN), None);
        assert_eq!(price_from_f64(-1.0), None);
        assert_eq!(price_from_f64(0.0), None);
    }

    #[test]
    fn multiples() {
        assert!(is_multiple_of(dec!(0.0125), dec!(0.00000001)));
        assert!(is_multiple_of(dec!(12.3), dec!(0.1)));
        assert!(!is_multiple_of(dec!(12.34), dec!(0.1)));
        assert!(!is_multiple_of(dec!(1), dec!(0)));
        assert_eq!(bps(25), dec!(0.0025));
    }
}
