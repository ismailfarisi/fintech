//! Which providers to run, read from `markets.toml`.
//!
//! ```toml
//! [[provider]]
//! kind = "coinbase"
//! products = ["BTC-USD", "ETH-USD"]
//!
//! [[provider]]
//! kind = "alpaca"
//! symbols = ["AAPL", "MSFT"]
//! ```
//!
//! Adding a provider = implement `MarketDataProvider` + add a variant here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;

use super::alpaca::{AlpacaConfig, AlpacaProvider};
use super::coinbase::{CoinbaseConfig, CoinbaseProvider};
use super::provider::MarketDataProvider;
use super::simulated::{SimulatedConfig, SimulatedProvider};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketsConfig {
    #[serde(rename = "provider", default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProviderConfig {
    Simulated(SimulatedConfig),
    Coinbase(CoinbaseConfig),
    Alpaca(AlpacaConfig),
}

/// Inputs a provider may need at construction time.
#[derive(Default)]
pub struct BuildContext {
    /// Last known prices, used by the simulator to resume (empty by default).
    pub saved_prices: BTreeMap<String, f64>,
}

impl ProviderConfig {
    pub fn build(&self, ctx: &BuildContext) -> anyhow::Result<Arc<dyn MarketDataProvider>> {
        Ok(match self {
            ProviderConfig::Simulated(c) => Arc::new(SimulatedProvider::new(c.clone(), &ctx.saved_prices)),
            ProviderConfig::Coinbase(c) => Arc::new(CoinbaseProvider::new(c.clone())?),
            ProviderConfig::Alpaca(c) => Arc::new(AlpacaProvider::new(c.clone())?),
        })
    }
}

impl MarketsConfig {
    /// Used when no config file exists: live crypto, plus stocks if Alpaca
    /// keys are set (Alpaca is skipped with a warning otherwise).
    pub fn default_live() -> Self {
        Self {
            providers: vec![
                ProviderConfig::Coinbase(CoinbaseConfig::default()),
                ProviderConfig::Alpaca(AlpacaConfig::default()),
            ],
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        toml::from_str(text).context("invalid markets config")
    }

    /// Load `path`, or the default if it doesn't exist.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("{} not found; using default providers", path.display());
                Ok(Self::default_live())
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Build every provider, skipping (with a warning) any that can't start,
    /// e.g. Alpaca without keys. Falls back to the simulator if none remain.
    pub fn build_all(&self, ctx: &BuildContext) -> Vec<Arc<dyn MarketDataProvider>> {
        let mut out: Vec<Arc<dyn MarketDataProvider>> = Vec::new();
        for cfg in &self.providers {
            match cfg.build(ctx) {
                Ok(p) => out.push(p),
                Err(e) => tracing::warn!("market data provider disabled: {e:#}"),
            }
        }
        if out.is_empty() {
            tracing::warn!("no market data providers available; using the simulator");
            out.push(Arc::new(SimulatedProvider::new(SimulatedConfig::default(), &ctx.saved_prices)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_provider_kinds_with_defaults() {
        let cfg = MarketsConfig::parse(
            r#"
            [[provider]]
            kind = "coinbase"
            products = ["btc-usd"]

            [[provider]]
            kind = "alpaca"
            symbols = ["AAPL"]
            feed = "sip"

            [[provider]]
            kind = "simulated"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.len(), 3);
        let ProviderConfig::Coinbase(c) = &cfg.providers[0] else { panic!() };
        assert_eq!(c.ws_url, "wss://ws-feed.exchange.coinbase.com");
        let ProviderConfig::Alpaca(a) = &cfg.providers[1] else { panic!() };
        assert_eq!((a.feed.as_str(), a.qty_step), ("sip", 1.0));
    }

    #[test]
    fn shipped_markets_toml_is_valid() {
        let cfg = MarketsConfig::parse(include_str!("../../markets.toml")).unwrap();
        assert!(matches!(cfg.providers[0], ProviderConfig::Coinbase(_)));
        assert!(matches!(cfg.providers[1], ProviderConfig::Alpaca(_)));
    }

    #[test]
    fn rejects_unknown_kinds_and_typos() {
        assert!(MarketsConfig::parse("[[provider]]\nkind = \"nasdaq\"").is_err());
        assert!(MarketsConfig::parse("[[provider]]\nkind = \"coinbase\"\nproduct = [\"BTC-USD\"]").is_err());
    }

    #[test]
    fn falls_back_to_simulator_when_nothing_can_start() {
        let cfg = MarketsConfig::parse(
            "[[provider]]\nkind = \"alpaca\"\nkey_id_env = \"LEDGER_NOPE\"\nsecret_key_env = \"LEDGER_NOPE2\"",
        )
        .unwrap();
        if std::env::var("APCA_API_KEY_ID").is_err() {
            let ps = cfg.build_all(&BuildContext::default());
            assert_eq!(ps.len(), 1);
            assert_eq!(ps[0].id(), "simulated");
        }
    }
}
