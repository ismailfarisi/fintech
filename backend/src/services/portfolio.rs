//! Read models: portfolio valuation.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use sqlx::PgExecutor;

use super::ServiceResult;
use crate::app::AppState;
use crate::db::{ledger, orders};
use crate::domain::ids::AccountId;
use crate::domain::money::price_from_f64;
use crate::domain::portfolio::{self, Holding, PortfolioSummary};

/// Current market prices as exact decimals.
pub fn prices(state: &AppState) -> BTreeMap<String, Decimal> {
    state.market.read().unwrap().prices().into_iter().filter_map(|(s, p)| price_from_f64(p).map(|d| (s, d))).collect()
}

/// Everything needed to value an account, loaded once; re-valued cheaply on each tick.
#[derive(Debug, Clone)]
pub struct AccountBook {
    pub holdings: Vec<Holding>,
    pub net_contributions: Decimal,
    pub realized_pnl: Decimal,
}

impl AccountBook {
    pub async fn load(db: impl PgExecutor<'_> + Copy, account_id: AccountId) -> ServiceResult<Self> {
        Ok(Self {
            holdings: ledger::holdings(db, account_id).await?,
            net_contributions: ledger::net_contributions(db, account_id).await?,
            realized_pnl: orders::realized_pnl(db, account_id).await?,
        })
    }

    pub fn value(&self, prices: &BTreeMap<String, Decimal>) -> PortfolioSummary {
        portfolio::summarize(&self.holdings, prices, self.net_contributions, self.realized_pnl)
    }
}
