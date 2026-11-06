//! Market data: a provider-agnostic price store fed by pluggable adapters.
//!
//! ```text
//!  ┌───────────────┐  MarketEvent  ┌─────┐  apply   ┌─────────────┐
//!  │ Coinbase      │──────────────▶│     │─────────▶│             │◀── API / engine
//!  │ Alpaca        │──────────────▶│ hub │          │ MarketStore │
//!  │ Simulated ... │──────────────▶│     │          │             │
//!  └───────────────┘               └─────┘          └─────────────┘
//!     provider.rs                  hub.rs            (this file)
//! ```
//!
//! To add a data source: implement [`provider::MarketDataProvider`] in a new
//! module and add a variant to [`config::ProviderConfig`].

pub mod alpaca;
pub mod coinbase;
pub mod config;
pub mod hub;
pub mod provider;
pub mod simulated;

use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Max price-history points kept per symbol (12h at 1-minute resolution).
pub const HISTORY_LEN: usize = 720;

pub fn round_cents(x: f64) -> f64 {
    // `+ 0.0` normalises -0.0 (e.g. from an empty float sum) to 0.0.
    (x * 100.0).round() / 100.0 + 0.0
}

/// Round a price to 8 decimals: enough for sub-cent crypto, stable for display.
pub fn round_price(x: f64) -> f64 {
    (x * 1e8).round() / 1e8 + 0.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetClass {
    Equity,
    Crypto,
}

/// Static description of a tradable symbol, declared by its provider.
#[derive(Debug, Clone, Serialize)]
pub struct Instrument {
    pub symbol: String,
    pub name: String,
    pub asset_class: AssetClass,
    /// Id of the provider that owns this symbol (e.g. "coinbase").
    pub source: String,
    /// What `reference` means for this market, e.g. "Prev close" or "24h open".
    pub reference_label: String,
    /// Smallest tradable quantity increment (1 = whole shares).
    pub qty_step: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PricePoint {
    pub t: DateTime<Utc>,
    pub price: f64,
}

/// A price change for one symbol. Optional stats overwrite the stored ones
/// when present; otherwise high/low are extended from the price.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceUpdate {
    pub symbol: String,
    pub price: f64,
    pub at: DateTime<Utc>,
    pub reference: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
}

impl PriceUpdate {
    pub fn new(symbol: impl Into<String>, price: f64, at: DateTime<Utc>) -> Self {
        Self { symbol: symbol.into(), price, at, reference: None, high: None, low: None }
    }
}

/// What providers emit.
#[derive(Debug, Clone, PartialEq)]
pub enum MarketEvent {
    /// Full refresh (on connect/reconnect): replaces history and stats.
    Snapshot {
        update: PriceUpdate,
        history: Vec<PricePoint>,
        /// Display name / lot size discovered from the provider, if any.
        name: Option<String>,
        qty_step: Option<f64>,
    },
    /// A live trade or tick.
    Tick(PriceUpdate),
}

impl MarketEvent {
    pub fn symbol(&self) -> &str {
        match self {
            MarketEvent::Snapshot { update, .. } | MarketEvent::Tick(update) => &update.symbol,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    #[serde(flatten)]
    pub instrument: Instrument,
    pub price: f64,
    pub reference: f64,
    pub high: f64,
    pub low: f64,
    pub change: f64,
    pub change_pct: f64,
    pub updated_at: DateTime<Utc>,
}

/// What the trading engine needs to know to accept an order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tradable {
    pub price: f64,
    pub qty_step: f64,
}

#[derive(Debug)]
struct Ticker {
    instrument: Instrument,
    /// History bucket width in seconds; ticks inside a bucket update its last point.
    resolution: i64,
    price: Option<f64>,
    reference: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    updated_at: Option<DateTime<Utc>>,
    history: VecDeque<PricePoint>,
}

#[derive(Debug, Default)]
pub struct MarketStore {
    tickers: BTreeMap<String, Ticker>,
}

fn valid_price(p: f64) -> bool {
    p.is_finite() && p > 0.0
}

impl MarketStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a symbol. Returns `false` (and keeps the existing one) if another
    /// provider already owns it.
    pub fn register(&mut self, instrument: Instrument, resolution_secs: i64) -> bool {
        if self.tickers.contains_key(&instrument.symbol) {
            return false;
        }
        self.tickers.insert(
            instrument.symbol.clone(),
            Ticker {
                instrument,
                resolution: resolution_secs.max(1),
                price: None,
                reference: None,
                high: None,
                low: None,
                updated_at: None,
                history: VecDeque::new(),
            },
        );
        true
    }

    /// Owner of a symbol, if registered.
    pub fn source_of(&self, symbol: &str) -> Option<&str> {
        self.tickers.get(symbol).map(|t| t.instrument.source.as_str())
    }

    pub fn apply(&mut self, event: MarketEvent) {
        match event {
            MarketEvent::Snapshot { update, history, name, qty_step } => {
                let Some(t) = self.tickers.get_mut(&update.symbol) else { return };
                if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
                    t.instrument.name = n;
                }
                if let Some(s) = qty_step.filter(|s| valid_price(*s)) {
                    t.instrument.qty_step = s;
                }
                let mut h: Vec<PricePoint> = history.into_iter().filter(|p| valid_price(p.price)).collect();
                h.sort_by_key(|p| p.t);
                let skip = h.len().saturating_sub(HISTORY_LEN);
                t.history = h.into_iter().skip(skip).collect();
                t.high = None;
                t.low = None;
                Self::apply_update(t, update);
            }
            MarketEvent::Tick(update) => {
                if let Some(t) = self.tickers.get_mut(&update.symbol) {
                    Self::apply_update(t, update);
                }
            }
        }
    }

    fn apply_update(t: &mut Ticker, u: PriceUpdate) {
        if !valid_price(u.price) {
            return;
        }
        // Ignore stale/out-of-order ticks.
        if t.updated_at.is_some_and(|prev| u.at < prev) {
            return;
        }
        let price = round_price(u.price);
        t.price = Some(price);
        t.updated_at = Some(u.at);
        if let Some(r) = u.reference.filter(|r| valid_price(*r)) {
            t.reference = Some(r);
        } else if t.reference.is_none() {
            t.reference = t.history.front().map(|p| p.price).or(Some(price));
        }
        t.high = Some(u.high.filter(|h| valid_price(*h)).unwrap_or(t.high.map_or(price, |h| h.max(price))));
        t.low = Some(u.low.filter(|l| valid_price(*l)).unwrap_or(t.low.map_or(price, |l| l.min(price))));

        let bucket = |at: DateTime<Utc>| at.timestamp().div_euclid(t.resolution);
        match t.history.back_mut() {
            Some(last) if bucket(last.t) == bucket(u.at) => last.price = price,
            Some(last) if last.t > u.at => {}
            _ => {
                if t.history.len() >= HISTORY_LEN {
                    t.history.pop_front();
                }
                t.history.push_back(PricePoint { t: u.at, price });
            }
        }
    }

    pub fn price(&self, symbol: &str) -> Option<f64> {
        self.tickers.get(symbol).and_then(|t| t.price)
    }

    pub fn tradable(&self, symbol: &str) -> Option<Tradable> {
        let t = self.tickers.get(symbol)?;
        Some(Tradable { price: t.price?, qty_step: t.instrument.qty_step })
    }

    pub fn contains(&self, symbol: &str) -> bool {
        self.tickers.contains_key(symbol)
    }

    pub fn prices(&self) -> BTreeMap<String, f64> {
        self.tickers.iter().filter_map(|(k, t)| t.price.map(|p| (k.clone(), p))).collect()
    }

    /// Quotes for every symbol that has received a price.
    pub fn quotes(&self) -> Vec<Quote> {
        self.tickers.values().filter_map(Self::quote_of).collect()
    }

    pub fn quote(&self, symbol: &str) -> Option<Quote> {
        self.tickers.get(symbol).and_then(Self::quote_of)
    }

    fn quote_of(t: &Ticker) -> Option<Quote> {
        let price = t.price?;
        let reference = t.reference.unwrap_or(price);
        let change = round_price(price - reference);
        Some(Quote {
            instrument: t.instrument.clone(),
            price,
            reference,
            high: t.high.unwrap_or(price),
            low: t.low.unwrap_or(price),
            change,
            change_pct: if reference > 0.0 { change / reference * 100.0 } else { 0.0 },
            updated_at: t.updated_at?,
        })
    }

    pub fn history(&self, symbol: &str) -> Option<Vec<PricePoint>> {
        self.tickers.get(symbol).map(|t| t.history.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn inst(sym: &str, source: &str) -> Instrument {
        Instrument {
            symbol: sym.into(),
            name: sym.into(),
            asset_class: AssetClass::Crypto,
            source: source.into(),
            reference_label: "24h open".into(),
            qty_step: 0.0001,
        }
    }

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().to_utc()
    }

    #[test]
    fn duplicate_symbols_are_rejected() {
        let mut s = MarketStore::new();
        assert!(s.register(inst("BTC-USD", "coinbase"), 60));
        assert!(!s.register(inst("BTC-USD", "other"), 60));
        assert_eq!(s.source_of("BTC-USD"), Some("coinbase"));
    }

    #[test]
    fn unpriced_symbols_have_no_quote_and_are_not_tradable() {
        let mut s = MarketStore::new();
        s.register(inst("BTC-USD", "coinbase"), 60);
        assert!(s.quotes().is_empty());
        assert!(s.tradable("BTC-USD").is_none());
    }

    #[test]
    fn snapshot_then_ticks_bucket_into_history() {
        let mut s = MarketStore::new();
        s.register(inst("BTC-USD", "coinbase"), 60);
        let history = (0..5).map(|i| PricePoint { t: t0() + Duration::minutes(i), price: 100.0 + i as f64 }).collect();
        let mut update = PriceUpdate::new("BTC-USD", 104.5, t0() + Duration::seconds(4 * 60 + 30));
        update.reference = Some(95.0);
        s.apply(MarketEvent::Snapshot { update, history, name: Some("Bitcoin".into()), qty_step: Some(1e-8) });

        let q = s.quote("BTC-USD").unwrap();
        assert_eq!(q.instrument.name, "Bitcoin");
        assert_eq!(q.instrument.qty_step, 1e-8);
        assert_eq!(q.reference, 95.0);
        assert_eq!(s.history("BTC-USD").unwrap().len(), 5, "same-minute tick replaced last point");

        // Same bucket -> replaces; next minute -> appends; older -> ignored.
        s.apply(MarketEvent::Tick(PriceUpdate::new("BTC-USD", 105.0, t0() + Duration::seconds(4 * 60 + 50))));
        assert_eq!(s.history("BTC-USD").unwrap().len(), 5);
        s.apply(MarketEvent::Tick(PriceUpdate::new("BTC-USD", 106.0, t0() + Duration::minutes(5))));
        assert_eq!(s.history("BTC-USD").unwrap().len(), 6);
        s.apply(MarketEvent::Tick(PriceUpdate::new("BTC-USD", 1.0, t0())));
        assert_eq!(s.price("BTC-USD"), Some(106.0));

        let q = s.quote("BTC-USD").unwrap();
        assert_eq!(q.high, 106.0);
        assert!((q.change - 11.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_prices_are_ignored() {
        let mut s = MarketStore::new();
        s.register(inst("X", "p"), 60);
        s.apply(MarketEvent::Tick(PriceUpdate::new("X", f64::NAN, t0())));
        s.apply(MarketEvent::Tick(PriceUpdate::new("X", -1.0, t0())));
        assert_eq!(s.price("X"), None);
        // Unknown symbols are ignored too.
        s.apply(MarketEvent::Tick(PriceUpdate::new("Y", 1.0, t0())));
        assert!(!s.contains("Y"));
    }
}
