//! Simulated market: a set of instruments whose prices follow a
//! geometric random walk (GBM). Swappable for a real data feed later —
//! the rest of the app only depends on `quotes()` / `price()` / `history()`.

use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, Utc};
use rand::Rng;
use rand::distr::{Distribution, StandardUniform};
use serde::Serialize;

/// How many ticks of price history to keep per symbol.
pub const HISTORY_LEN: usize = 600;

#[derive(Debug, Clone, Serialize)]
pub struct Instrument {
    pub symbol: String,
    pub name: String,
    pub sector: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PricePoint {
    pub t: DateTime<Utc>,
    pub price: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    pub symbol: String,
    pub name: String,
    pub sector: String,
    pub price: f64,
    /// Price at session open (start of the simulation).
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub change: f64,
    pub change_pct: f64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
struct Ticker {
    instrument: Instrument,
    price: f64,
    open: f64,
    high: f64,
    low: f64,
    /// Annualised drift and volatility used for the random walk.
    drift: f64,
    vol: f64,
    history: VecDeque<PricePoint>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct Market {
    tickers: BTreeMap<String, Ticker>,
}

pub fn round_cents(x: f64) -> f64 {
    // `+ 0.0` normalises -0.0 (e.g. from an empty float sum) to 0.0.
    (x * 100.0).round() / 100.0 + 0.0
}

/// Standard normal sample via Box–Muller (avoids pulling in rand_distr).
fn std_normal<R: Rng + ?Sized>(rng: &mut R) -> f64 {
    let u1: f64 = StandardUniform.sample(rng);
    let u2: f64 = StandardUniform.sample(rng);
    let u1 = u1.max(f64::MIN_POSITIVE);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

impl Market {
    /// Default universe of fictional instruments, resuming from `resume`
    /// prices (e.g. saved at the last shutdown) where given.
    pub fn simulated(resume: &BTreeMap<String, f64>) -> Self {
        let seed: &[(&str, &str, &str, f64, f64, f64)] = &[
            // symbol, name, sector, start price, drift, vol
            ("NOVA", "Nova Computing", "Technology", 182.40, 0.12, 0.35),
            ("QBIT", "Qubit Labs", "Technology", 64.15, 0.20, 0.60),
            ("ORBT", "Orbital Freight", "Industrials", 41.80, 0.05, 0.30),
            ("HLTH", "Helix Health", "Healthcare", 128.90, 0.07, 0.25),
            ("GRNE", "Greenline Energy", "Energy", 23.55, 0.09, 0.45),
            ("BANK", "Bastion Bank", "Financials", 57.30, 0.04, 0.20),
            ("SHOP", "Cartwheel Retail", "Consumer", 96.70, 0.06, 0.30),
            ("AQUA", "Aquifer Utilities", "Utilities", 34.10, 0.03, 0.12),
            ("BTCX", "Bitcoin Index", "Crypto", 68250.00, 0.15, 0.70),
            ("ETHX", "Ether Index", "Crypto", 3420.00, 0.15, 0.80),
        ];
        let now = Utc::now();
        let tickers = seed
            .iter()
            .map(|&(sym, name, sector, price, drift, vol)| {
                let mut history = VecDeque::with_capacity(HISTORY_LEN);
                history.push_back(PricePoint { t: now, price });
                (
                    sym.to_string(),
                    Ticker {
                        instrument: Instrument {
                            symbol: sym.into(),
                            name: name.into(),
                            sector: sector.into(),
                        },
                        price,
                        open: price,
                        high: price,
                        low: price,
                        drift,
                        vol,
                        history,
                        updated_at: now,
                    },
                )
            })
            .collect();
        let mut m = Market { tickers };
        // Back-fill some history so charts aren't empty on first load.
        let mut rng = rand::rng();
        let step = chrono::Duration::seconds(1);
        let start = now - step * (HISTORY_LEN as i32 - 1);
        for t in m.tickers.values_mut() {
            t.history.clear();
            t.updated_at = start;
        }
        for i in 0..HISTORY_LEN {
            m.step_with(&mut rng, start + step * i as i32, 1.0);
        }
        for (sym, t) in m.tickers.iter_mut() {
            // Rescale the synthetic back-fill so it ends at the resumed price.
            if let Some(&target) = resume.get(sym).filter(|p| p.is_finite() && **p > 0.0) {
                let k = target / t.price;
                for p in t.history.iter_mut() {
                    p.price = round_cents((p.price * k).max(0.01));
                }
                t.price = target;
            }
            t.open = t.history.front().map(|p| p.price).unwrap_or(t.price);
            t.high = t.history.iter().map(|p| p.price).fold(f64::MIN, f64::max);
            t.low = t.history.iter().map(|p| p.price).fold(f64::MAX, f64::min);
        }
        m
    }

    /// Advance every price by one tick of `dt_secs` simulated seconds.
    pub fn step(&mut self, dt_secs: f64) {
        let mut rng = rand::rng();
        self.step_with(&mut rng, Utc::now(), dt_secs);
    }

    fn step_with<R: Rng + ?Sized>(&mut self, rng: &mut R, now: DateTime<Utc>, dt_secs: f64) {
        // Accelerate time so the demo moves visibly: 1 tick ≈ 5 trading minutes.
        const SECS_PER_YEAR: f64 = 252.0 * 6.5 * 3600.0;
        let dt = dt_secs * 300.0 / SECS_PER_YEAR;
        for t in self.tickers.values_mut() {
            let z = std_normal(rng);
            let growth = ((t.drift - 0.5 * t.vol * t.vol) * dt + t.vol * dt.sqrt() * z).exp();
            t.price = round_cents((t.price * growth).max(0.01));
            t.high = t.high.max(t.price);
            t.low = t.low.min(t.price);
            t.updated_at = now;
            if t.history.len() == HISTORY_LEN {
                t.history.pop_front();
            }
            t.history.push_back(PricePoint { t: now, price: t.price });
        }
    }

    pub fn price(&self, symbol: &str) -> Option<f64> {
        self.tickers.get(symbol).map(|t| t.price)
    }

    pub fn contains(&self, symbol: &str) -> bool {
        self.tickers.contains_key(symbol)
    }

    pub fn instrument(&self, symbol: &str) -> Option<&Instrument> {
        self.tickers.get(symbol).map(|t| &t.instrument)
    }

    pub fn prices(&self) -> BTreeMap<String, f64> {
        self.tickers.iter().map(|(k, t)| (k.clone(), t.price)).collect()
    }

    pub fn quotes(&self) -> Vec<Quote> {
        self.tickers
            .values()
            .map(|t| {
                let change = round_cents(t.price - t.open);
                Quote {
                    symbol: t.instrument.symbol.clone(),
                    name: t.instrument.name.clone(),
                    sector: t.instrument.sector.clone(),
                    price: t.price,
                    open: t.open,
                    high: t.high,
                    low: t.low,
                    change,
                    change_pct: if t.open > 0.0 { change / t.open * 100.0 } else { 0.0 },
                    updated_at: t.updated_at,
                }
            })
            .collect()
    }

    pub fn history(&self, symbol: &str) -> Option<Vec<PricePoint>> {
        self.tickers.get(symbol).map(|t| t.history.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_market_has_history_and_positive_prices() {
        let mut m = Market::simulated(&BTreeMap::new());
        for _ in 0..1000 {
            m.step(1.0);
        }
        for q in m.quotes() {
            assert!(q.price > 0.0, "{} went non-positive", q.symbol);
            assert!(q.low <= q.price && q.price <= q.high);
            assert_eq!(m.history(&q.symbol).unwrap().len(), HISTORY_LEN);
        }
    }

    #[test]
    fn resumes_from_saved_prices() {
        let saved = BTreeMap::from([("NOVA".to_string(), 250.0)]);
        let m = Market::simulated(&saved);
        assert_eq!(m.price("NOVA"), Some(250.0));
        assert_eq!(m.history("NOVA").unwrap().last().unwrap().price, 250.0);
        let q = m.quotes().into_iter().find(|q| q.symbol == "NOVA").unwrap();
        assert!(q.low <= 250.0 && 250.0 <= q.high);
    }

    #[test]
    fn normal_sampler_is_roughly_standard() {
        let mut rng = rand::rng();
        let n = 20_000;
        let xs: Vec<f64> = (0..n).map(|_| std_normal(&mut rng)).collect();
        let mean = xs.iter().sum::<f64>() / n as f64;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 1.0).abs() < 0.08, "var {var}");
    }
}
