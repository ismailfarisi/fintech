//! Shared application state, persistence and the background market loop.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::engine::{Account, PortfolioSummary, Trade};
use crate::market::{Market, Quote};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub market: RwLock<Market>,
    pub account: Mutex<Account>,
    pub events: broadcast::Sender<Event>,
    data_path: Option<PathBuf>,
}

/// What's written to the data file.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    account: Account,
    /// Last market prices, so the simulation resumes where it left off.
    #[serde(default)]
    prices: BTreeMap<String, f64>,
}

/// Pushed to WebSocket clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Tick { quotes: Vec<Quote>, portfolio: PortfolioSummary },
    Fill { trade: Trade },
    AccountChanged,
}

impl AppState {
    /// Build state, loading the account from `data_path` if it exists.
    pub fn new(data_path: Option<PathBuf>) -> SharedState {
        let snapshot = data_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| match serde_json::from_str::<Snapshot>(&s) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    tracing::warn!("ignoring unreadable data file: {e}");
                    None
                }
            });
        let (account, prices) = match snapshot {
            Some(s) => (s.account, s.prices),
            None => (Account::default(), BTreeMap::new()),
        };
        let (events, _) = broadcast::channel(256);
        Arc::new(AppState {
            market: RwLock::new(Market::simulated(&prices)),
            account: Mutex::new(account),
            events,
            data_path,
        })
    }

    /// Persist the account atomically (write temp file, then rename).
    pub fn save(&self) {
        let Some(path) = &self.data_path else { return };
        let prices = self.market.read().unwrap().prices();
        let json = {
            let account = self.account.lock().unwrap().clone();
            serde_json::to_vec_pretty(&Snapshot { account, prices })
        };
        let result = json.map_err(anyhow::Error::from).and_then(|bytes| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        });
        if let Err(e) = result {
            tracing::error!("failed to save account to {}: {e}", path.display());
        }
    }

    pub fn notify(&self, event: Event) {
        // No subscribers is fine.
        let _ = self.events.send(event);
    }

    /// One market tick: move prices, match limit orders, broadcast.
    pub fn tick(&self) -> Vec<Trade> {
        let (prices, quotes) = {
            let mut market = self.market.write().unwrap();
            market.step(1.0);
            (market.prices(), market.quotes())
        };
        let now = Utc::now();
        let (trades, portfolio) = {
            let mut account = self.account.lock().unwrap();
            let trades = account.on_prices(&prices, now);
            account.record_equity(&prices, now);
            (trades, account.summary(&prices))
        };
        for trade in &trades {
            tracing::info!(
                "limit fill: {:?} {} {} @ {:.2}",
                trade.side, trade.qty, trade.symbol, trade.price
            );
            self.notify(Event::Fill { trade: trade.clone() });
        }
        self.notify(Event::Tick { quotes, portfolio });
        trades
    }
}

/// Run the market forever, ticking every `interval`.
pub async fn run_market(state: SharedState, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    let mut n: u64 = 0;
    loop {
        ticker.tick().await;
        let trades = state.tick();
        n += 1;
        // Save immediately after fills, otherwise every ~10 ticks for equity history.
        if !trades.is_empty() || n.is_multiple_of(10) {
            state.save();
        }
    }
}
