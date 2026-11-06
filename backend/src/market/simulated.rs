//! Offline adapter: fictional instruments whose prices follow a geometric
//! random walk (GBM). Useful for demos, development without network access,
//! and tests.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use rand::distr::{Distribution, StandardUniform};
use serde::Deserialize;

use super::provider::{MarketDataProvider, MarketSink};
use super::{AssetClass, HISTORY_LEN, Instrument, MarketEvent, PricePoint, PriceUpdate, round_price};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatedConfig {
    /// Milliseconds between ticks.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
}

impl Default for SimulatedConfig {
    fn default() -> Self {
        Self { tick_ms: default_tick_ms() }
    }
}

fn default_tick_ms() -> u64 {
    1000
}

struct Walk {
    symbol: &'static str,
    name: &'static str,
    asset_class: AssetClass,
    price: f64,
    drift: f64,
    vol: f64,
}

pub struct SimulatedProvider {
    cfg: SimulatedConfig,
    walks: Mutex<Vec<Walk>>,
    /// Pre-generated history so charts aren't empty on first load.
    backfill: Mutex<BTreeMap<&'static str, Vec<PricePoint>>>,
}

/// Standard normal sample via Box–Muller (avoids pulling in rand_distr).
fn std_normal<R: Rng + ?Sized>(rng: &mut R) -> f64 {
    let u1: f64 = StandardUniform.sample(rng);
    let u2: f64 = StandardUniform.sample(rng);
    let u1 = u1.max(f64::MIN_POSITIVE);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

impl Walk {
    /// Advance one tick. Time is accelerated: 1 tick ≈ 5 trading minutes.
    fn step<R: Rng + ?Sized>(&mut self, rng: &mut R) {
        const SECS_PER_YEAR: f64 = 252.0 * 6.5 * 3600.0;
        let dt = 300.0 / SECS_PER_YEAR;
        let z = std_normal(rng);
        let growth = ((self.drift - 0.5 * self.vol * self.vol) * dt + self.vol * dt.sqrt() * z).exp();
        self.price = round_price((self.price * growth).max(0.01));
    }
}

impl SimulatedProvider {
    /// `resume` holds prices saved at the last shutdown, so the walk continues
    /// from where it left off instead of jumping back to seed prices.
    pub fn new(cfg: SimulatedConfig, resume: &BTreeMap<String, f64>) -> Self {
        let seed: &[(&str, &str, AssetClass, f64, f64, f64)] = &[
            // symbol, name, class, start price, drift, vol
            ("NOVA", "Nova Computing", AssetClass::Equity, 182.40, 0.12, 0.35),
            ("QBIT", "Qubit Labs", AssetClass::Equity, 64.15, 0.20, 0.60),
            ("ORBT", "Orbital Freight", AssetClass::Equity, 41.80, 0.05, 0.30),
            ("HLTH", "Helix Health", AssetClass::Equity, 128.90, 0.07, 0.25),
            ("GRNE", "Greenline Energy", AssetClass::Equity, 23.55, 0.09, 0.45),
            ("BANK", "Bastion Bank", AssetClass::Equity, 57.30, 0.04, 0.20),
            ("SHOP", "Cartwheel Retail", AssetClass::Equity, 96.70, 0.06, 0.30),
            ("AQUA", "Aquifer Utilities", AssetClass::Equity, 34.10, 0.03, 0.12),
        ];
        let mut walks: Vec<Walk> = seed
            .iter()
            .map(|&(symbol, name, asset_class, price, drift, vol)| Walk { symbol, name, asset_class, price, drift, vol })
            .collect();

        // Back-fill history one point per tick interval, ending now.
        let mut rng = rand::rng();
        let step = chrono::Duration::milliseconds(cfg.tick_ms.max(1) as i64);
        let now = Utc::now();
        let mut backfill = BTreeMap::new();
        for w in &mut walks {
            let mut pts = Vec::with_capacity(HISTORY_LEN);
            for i in 0..HISTORY_LEN {
                w.step(&mut rng);
                pts.push(PricePoint { t: now - step * (HISTORY_LEN - 1 - i) as i32, price: w.price });
            }
            // Rescale so the series ends at the resumed price.
            if let Some(&target) = resume.get(w.symbol).filter(|p| p.is_finite() && **p > 0.0) {
                let k = target / w.price;
                for p in &mut pts {
                    p.price = round_price((p.price * k).max(0.01));
                }
                w.price = target;
            }
            backfill.insert(w.symbol, pts);
        }
        Self { cfg, walks: Mutex::new(walks), backfill: Mutex::new(backfill) }
    }

    /// Initial snapshot events (also used by tests to seed a store synchronously).
    pub fn snapshots(&self) -> Vec<MarketEvent> {
        let walks = self.walks.lock().unwrap();
        let backfill = self.backfill.lock().unwrap();
        walks
            .iter()
            .map(|w| {
                let history = backfill.get(w.symbol).cloned().unwrap_or_default();
                let at = history.last().map_or_else(Utc::now, |p| p.t);
                let mut update = PriceUpdate::new(w.symbol, w.price, at);
                update.reference = history.first().map(|p| p.price);
                MarketEvent::Snapshot { update, history, name: None, qty_step: None }
            })
            .collect()
    }

    fn tick(&self, now: DateTime<Utc>) -> Vec<MarketEvent> {
        let mut rng = rand::rng();
        let mut walks = self.walks.lock().unwrap();
        walks
            .iter_mut()
            .map(|w| {
                w.step(&mut rng);
                MarketEvent::Tick(PriceUpdate::new(w.symbol, w.price, now))
            })
            .collect()
    }
}

#[async_trait]
impl MarketDataProvider for SimulatedProvider {
    fn id(&self) -> &str {
        "simulated"
    }

    fn instruments(&self) -> Vec<Instrument> {
        self.walks
            .lock()
            .unwrap()
            .iter()
            .map(|w| Instrument {
                symbol: w.symbol.into(),
                name: w.name.into(),
                asset_class: w.asset_class,
                source: "simulated".into(),
                reference_label: "Open".into(),
                qty_step: 1.0,
            })
            .collect()
    }

    fn resolution_secs(&self) -> i64 {
        (self.cfg.tick_ms / 1000).max(1) as i64
    }

    async fn run(&self, sink: MarketSink) -> anyhow::Result<()> {
        for ev in self.snapshots() {
            sink.send(ev).await?;
        }
        let mut interval = tokio::time::interval(Duration::from_millis(self.cfg.tick_ms.max(50)));
        interval.tick().await; // first tick fires immediately
        loop {
            interval.tick().await;
            for ev in self.tick(Utc::now()) {
                sink.send(ev).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_stays_positive() {
        let p = SimulatedProvider::new(SimulatedConfig::default(), &BTreeMap::new());
        for _ in 0..1000 {
            for ev in p.tick(Utc::now()) {
                let MarketEvent::Tick(u) = ev else { panic!() };
                assert!(u.price > 0.0);
            }
        }
    }

    #[test]
    fn resumes_from_saved_prices() {
        let saved = BTreeMap::from([("NOVA".to_string(), 250.0)]);
        let p = SimulatedProvider::new(SimulatedConfig::default(), &saved);
        let nova = p.snapshots().into_iter().find(|e| e.symbol() == "NOVA").unwrap();
        let MarketEvent::Snapshot { update, history, .. } = nova else { panic!() };
        assert_eq!(update.price, 250.0);
        assert_eq!(history.last().unwrap().price, 250.0);
        assert_eq!(history.len(), HISTORY_LEN);
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
