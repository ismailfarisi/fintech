//! Shared application state, persistence and the order-matching loop.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::engine::{Account, PortfolioSummary, Trade};
use crate::market::hub::StatusMap;
use crate::market::{MarketStore, Quote};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub market: Arc<RwLock<MarketStore>>,
    pub providers: StatusMap,
    pub account: Mutex<Account>,
    pub events: broadcast::Sender<Event>,
    /// Prices loaded from the data file (lets the simulator resume).
    pub saved_prices: BTreeMap<String, f64>,
    data_path: Option<PathBuf>,
}

/// What's written to the data file.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    account: Account,
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
        let (account, saved_prices) = match snapshot {
            Some(s) => (s.account, s.prices),
            None => (Account::default(), BTreeMap::new()),
        };
        let (events, _) = broadcast::channel(256);
        Arc::new(AppState {
            market: Arc::new(RwLock::new(MarketStore::new())),
            providers: StatusMap::default(),
            account: Mutex::new(account),
            events,
            saved_prices,
            data_path,
        })
    }

    /// Persist the account atomically (write temp file, then rename).
    pub fn save(&self) {
        let Some(path) = &self.data_path else { return };
        let prices = self.valuation_prices();
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

    /// Prices for valuing positions: live where available, else last saved.
    /// (Order matching uses live prices only.)
    pub fn valuation_prices(&self) -> BTreeMap<String, f64> {
        let mut prices = self.saved_prices.clone();
        prices.extend(self.market.read().unwrap().prices());
        prices
    }

    /// One engine tick: match limit orders against live prices, broadcast.
    pub fn tick(&self) -> Vec<Trade> {
        let (live, quotes) = {
            let market = self.market.read().unwrap();
            (market.prices(), market.quotes())
        };
        let mut valuation = self.saved_prices.clone();
        valuation.extend(live.iter().map(|(k, v)| (k.clone(), *v)));
        let now = Utc::now();
        let (trades, portfolio) = {
            let mut account = self.account.lock().unwrap();
            let trades = account.on_prices(&live, now);
            account.record_equity(&valuation, now);
            (trades, account.summary(&valuation))
        };
        for trade in &trades {
            tracing::info!("limit fill: {:?} {} {} @ {}", trade.side, trade.qty, trade.symbol, trade.price);
            self.notify(Event::Fill { trade: trade.clone() });
        }
        self.notify(Event::Tick { quotes, portfolio });
        trades
    }
}

/// Run the engine forever, ticking every `interval`.
pub async fn run_engine(state: SharedState, interval: Duration) {
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
