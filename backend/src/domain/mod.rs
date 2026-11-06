//! Pure business rules: no I/O, no database, no HTTP.
//!
//! All money and quantity math uses [`rust_decimal::Decimal`] (exact base-10).
//! Market data arrives as `f64` observations and is converted at the boundary
//! with [`money::price_from_f64`].

pub mod ids;
pub mod money;
pub mod order;
pub mod portfolio;

use rust_decimal::Decimal;
use thiserror::Error;

/// A business-rule violation. Safe to show to the user.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("quantity must be greater than zero")]
    InvalidQuantity,
    #[error("quantity must be a multiple of {0}")]
    QuantityIncrement(Decimal),
    #[error("quantity is too large")]
    QuantityTooLarge,
    #[error("limit orders need a positive limit price")]
    InvalidLimitPrice,
    #[error("market orders must not have a limit price")]
    UnexpectedLimitPrice,
    #[error("price is too large")]
    PriceTooLarge,
    #[error("unknown or untradable symbol '{0}'")]
    UnknownSymbol(String),
    #[error("no current price for '{0}'; try again shortly")]
    NoPrice(String),
    #[error("insufficient buying power: need ${needed}, have ${available}")]
    InsufficientFunds { needed: Decimal, available: Decimal },
    #[error("insufficient quantity: need {needed}, have {available} available to sell")]
    InsufficientQuantity { needed: Decimal, available: Decimal },
    #[error("amount must be a positive number of dollars and cents")]
    InvalidAmount,
    #[error("amount is too large")]
    AmountTooLarge,
}
