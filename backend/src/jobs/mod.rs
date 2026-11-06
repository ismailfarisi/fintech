//! Background jobs. Each runs in its own task; failures are logged and the
//! loop continues.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;

use crate::app::SharedState;
use crate::db::{idempotency, ledger, sessions, snapshots};
use crate::domain::ids::AccountId;
use crate::domain::order::FeeSchedule;
use crate::domain::portfolio::{self, Holding};
use crate::events::Event;
use crate::services::portfolio::prices;
use crate::services::trading;

pub fn spawn_all(state: SharedState) {
    tokio::spawn(engine(state.clone(), Duration::from_secs(1)));
    tokio::spawn(equity_snapshots(state.clone(), Duration::from_secs(60)));
    tokio::spawn(cleanup(state, Duration::from_secs(3600)));
}

/// Every tick: push quotes to clients, then fill resting limit orders that the
/// current prices cross.
pub async fn engine(state: SharedState, every: Duration) {
    let mut interval = tokio::time::interval(every);
    loop {
        interval.tick().await;
        let quotes = state.market.read().unwrap().quotes();
        state.events.publish(Event::Quotes(Arc::new(quotes)));
        if let Err(e) = match_orders(&state).await {
            tracing::error!(error = ?e, "order matching failed");
        }
    }
}

/// One matching pass. Each fill is its own transaction.
pub async fn match_orders(state: &SharedState) -> anyhow::Result<usize> {
    let fees = FeeSchedule { bps: state.config.fee_bps };
    let mut filled = 0;
    for (symbol, price) in prices(state) {
        for (order_id, account_id) in crate::db::orders::crossing(&state.db, &symbol, price, 100).await? {
            let mut tx = state.db.begin().await?;
            match trading::try_fill_resting(&mut tx, account_id, order_id, price, fees).await {
                Ok(Some((trade, events))) => {
                    tx.commit().await?;
                    filled += 1;
                    tracing::info!(%order_id, %symbol, %price, qty = %trade.quantity, "limit order filled");
                    for e in events {
                        state.events.publish(e);
                    }
                }
                Ok(None) => {} // raced with a cancel or fill; nothing to do
                Err(e) => tracing::warn!(%order_id, error = %e, "could not fill order"),
            }
        }
    }
    Ok(filled)
}

/// Record every open account's equity for the portfolio chart.
pub async fn equity_snapshots(state: SharedState, every: Duration) {
    let mut interval = tokio::time::interval(every);
    loop {
        interval.tick().await;
        if let Err(e) = snapshot_once(&state).await {
            tracing::error!(error = ?e, "equity snapshot failed");
        }
    }
}

pub async fn snapshot_once(state: &SharedState) -> anyhow::Result<usize> {
    let prices = prices(state);
    let mut by_account: BTreeMap<AccountId, Vec<Holding>> = BTreeMap::new();
    for (account, h) in ledger::all_open_holdings(&state.db).await? {
        by_account.entry(account).or_default().push(h);
    }
    let rows: Vec<(AccountId, Decimal)> =
        by_account.into_iter().map(|(a, hs)| (a, portfolio::equity(&hs, &prices))).collect();
    // Truncate to the minute so repeated runs don't duplicate points.
    let at = Utc::now().with_second_and_nanos_zeroed();
    snapshots::insert_many(&state.db, at, &rows).await?;
    Ok(rows.len())
}

trait MinuteFloor {
    fn with_second_and_nanos_zeroed(self) -> Self;
}

impl MinuteFloor for chrono::DateTime<Utc> {
    fn with_second_and_nanos_zeroed(self) -> Self {
        use chrono::Timelike;
        self.with_second(0).and_then(|t| t.with_nanosecond(0)).unwrap_or(self)
    }
}

/// Remove expired sessions and old idempotency keys.
pub async fn cleanup(state: SharedState, every: Duration) {
    let mut interval = tokio::time::interval(every);
    loop {
        interval.tick().await;
        match (sessions::purge_old(&state.db).await, idempotency::purge_older_than_days(&state.db, 1).await) {
            (Ok(s), Ok(k)) if s + k > 0 => tracing::info!(sessions = s, idempotency_keys = k, "cleanup"),
            (Err(e), _) | (_, Err(e)) => tracing::error!(error = ?e, "cleanup failed"),
            _ => {}
        }
    }
}
