//! Coinbase Exchange adapter: public market data, no API key needed.
//!
//! * REST `GET /products/{id}`, `/currencies/{base}`: name and lot size
//! * REST `GET /products/{id}/stats`: 24h open/high/low/last
//! * REST `GET /products/{id}/candles?granularity=60`: last 300 one-minute candles
//! * WebSocket `ticker` channel for live trades, `heartbeat` for liveness
//!
//! Docs: <https://docs.cdp.coinbase.com/exchange/>

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

use super::provider::{MarketDataProvider, MarketSink};
use super::{AssetClass, Instrument, MarketEvent, PricePoint, PriceUpdate};
use crate::net;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoinbaseConfig {
    /// Product ids, e.g. ["BTC-USD", "ETH-USD"].
    #[serde(default = "default_products")]
    pub products: Vec<String>,
    #[serde(default = "default_rest_url")]
    pub rest_url: String,
    #[serde(default = "default_ws_url")]
    pub ws_url: String,
}

impl Default for CoinbaseConfig {
    fn default() -> Self {
        Self { products: default_products(), rest_url: default_rest_url(), ws_url: default_ws_url() }
    }
}

fn default_products() -> Vec<String> {
    ["BTC-USD", "ETH-USD", "SOL-USD", "DOGE-USD"].map(String::from).to_vec()
}
fn default_rest_url() -> String {
    "https://api.exchange.coinbase.com".into()
}
fn default_ws_url() -> String {
    "wss://ws-feed.exchange.coinbase.com".into()
}

/// No message at all for this long means the feed is dead (heartbeats are 1/s).
const READ_TIMEOUT: Duration = Duration::from_secs(30);

pub struct CoinbaseProvider {
    cfg: CoinbaseConfig,
    http: reqwest::Client,
    /// product id -> (display name, base increment); fetched once.
    meta: Mutex<HashMap<String, (String, f64)>>,
}

impl CoinbaseProvider {
    pub fn new(mut cfg: CoinbaseConfig) -> anyhow::Result<Self> {
        cfg.products = cfg.products.iter().map(|p| p.trim().to_uppercase()).filter(|p| !p.is_empty()).collect();
        cfg.products.dedup();
        if cfg.products.is_empty() {
            bail!("coinbase: no products configured");
        }
        Ok(Self { cfg, http: net::http_client(), meta: Mutex::new(HashMap::new()) })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.cfg.rest_url.trim_end_matches('/'));
        let res = self.http.get(&url).send().await.with_context(|| format!("GET {url}"))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            bail!("GET {url}: {status} {}", body.chars().take(200).collect::<String>());
        }
        res.json().await.with_context(|| format!("decode {url}"))
    }

    async fn metadata(&self, product: &str) -> (Option<String>, Option<f64>) {
        if let Some((n, s)) = self.meta.lock().unwrap().get(product).cloned() {
            return (Some(n), Some(s));
        }
        let Ok(p) = self.get::<ProductInfo>(&format!("/products/{product}")).await else {
            return (None, None);
        };
        let step = p.base_increment.parse::<f64>().ok().filter(|s| *s > 0.0);
        let name = self
            .get::<CurrencyInfo>(&format!("/currencies/{}", p.base_currency))
            .await
            .map(|c| c.name)
            .unwrap_or(p.base_currency);
        if let Some(s) = step {
            self.meta.lock().unwrap().insert(product.to_string(), (name.clone(), s));
        }
        (Some(name), step)
    }

    async fn snapshot(&self, product: &str) -> anyhow::Result<MarketEvent> {
        let stats: Stats = self.get(&format!("/products/{product}/stats")).await?;
        let candles: Vec<Candle> = self.get(&format!("/products/{product}/candles?granularity=60")).await?;
        let (name, qty_step) = self.metadata(product).await;
        Ok(snapshot_event(product, &stats, &candles, Utc::now(), name, qty_step))
    }
}

#[async_trait]
impl MarketDataProvider for CoinbaseProvider {
    fn id(&self) -> &str {
        "coinbase"
    }

    fn instruments(&self) -> Vec<Instrument> {
        self.cfg
            .products
            .iter()
            .map(|p| Instrument {
                symbol: p.clone(),
                name: p.clone(),
                asset_class: AssetClass::Crypto,
                source: "coinbase".into(),
                reference_label: "24h open".into(),
                qty_step: 0.0001,
            })
            .collect()
    }

    async fn run(&self, sink: MarketSink) -> anyhow::Result<()> {
        // Subscribe first so no trades are missed while snapshots load; ticks
        // that arrive earlier than a snapshot are simply superseded by it.
        let mut ws = net::connect_ws(&self.cfg.ws_url).await?;
        let sub = serde_json::json!({
            "type": "subscribe",
            "product_ids": self.cfg.products,
            "channels": ["ticker", "heartbeat"],
        });
        ws.send(Message::Text(sub.to_string().into())).await?;

        for product in &self.cfg.products {
            match self.snapshot(product).await {
                Ok(ev) => sink.send(ev).await?,
                Err(e) => tracing::warn!("coinbase: snapshot for {product} failed: {e:#}"),
            }
        }

        loop {
            let msg = tokio::time::timeout(READ_TIMEOUT, ws.next()).await.context("no data from coinbase for 30s")?;
            let text = match msg {
                Some(Ok(Message::Text(t))) => t,
                Some(Ok(Message::Close(frame))) => bail!("server closed connection: {frame:?}"),
                Some(Ok(_)) => continue, // ping/pong/binary
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            };
            match parse_message(&text)? {
                Some(update) => sink.send(MarketEvent::Tick(update)).await?,
                None => continue,
            }
        }
    }
}

// ---- wire format -------------------------------------------------------------

fn de_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    // Coinbase encodes numbers as strings.
    let s = String::deserialize(d)?;
    s.parse().map_err(serde::de::Error::custom)
}

fn de_opt_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.and_then(|s| s.parse().ok()))
}

#[derive(Deserialize)]
struct ProductInfo {
    base_currency: String,
    base_increment: String,
}

#[derive(Deserialize)]
struct CurrencyInfo {
    name: String,
}

#[derive(Deserialize)]
struct Stats {
    #[serde(deserialize_with = "de_f64")]
    open: f64,
    #[serde(deserialize_with = "de_f64")]
    high: f64,
    #[serde(deserialize_with = "de_f64")]
    low: f64,
    #[serde(deserialize_with = "de_f64")]
    last: f64,
}

/// `[time, low, high, open, close, volume]`, newest first. We only use time and close.
#[derive(Deserialize)]
struct Candle(i64, serde::de::IgnoredAny, serde::de::IgnoredAny, serde::de::IgnoredAny, f64, serde::de::IgnoredAny);

fn snapshot_event(
    product: &str,
    stats: &Stats,
    candles: &[Candle],
    now: DateTime<Utc>,
    name: Option<String>,
    qty_step: Option<f64>,
) -> MarketEvent {
    let history =
        candles.iter().filter_map(|c| DateTime::from_timestamp(c.0, 0).map(|t| PricePoint { t, price: c.4 })).collect();
    let mut update = PriceUpdate::new(product, stats.last, now);
    update.reference = Some(stats.open);
    update.high = Some(stats.high);
    update.low = Some(stats.low);
    MarketEvent::Snapshot { update, history, name, qty_step }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WsMessage {
    Ticker {
        product_id: String,
        #[serde(deserialize_with = "de_f64")]
        price: f64,
        #[serde(default, deserialize_with = "de_opt_f64")]
        open_24h: Option<f64>,
        #[serde(default, deserialize_with = "de_opt_f64")]
        high_24h: Option<f64>,
        #[serde(default, deserialize_with = "de_opt_f64")]
        low_24h: Option<f64>,
        time: Option<DateTime<Utc>>,
    },
    Error {
        message: String,
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Other, // subscriptions, heartbeat, ...
}

/// Translate one feed message. `Ok(None)` for messages we don't need.
fn parse_message(text: &str) -> anyhow::Result<Option<PriceUpdate>> {
    let msg: WsMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("coinbase: ignoring unparseable message ({e}): {text}");
            return Ok(None);
        }
    };
    match msg {
        WsMessage::Ticker { product_id, price, open_24h, high_24h, low_24h, time } => {
            let mut u = PriceUpdate::new(product_id, price, time.unwrap_or_else(Utc::now));
            u.reference = open_24h;
            u.high = high_24h;
            u.low = low_24h;
            Ok(Some(u))
        }
        WsMessage::Error { message, reason } => bail!("{message}: {reason}"),
        WsMessage::Other => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ticker() {
        let text = r#"{"type":"ticker","sequence":1,"product_id":"BTC-USD","price":"75786.03",
            "open_24h":"75709.41","volume_24h":"9110.2","low_24h":"74722","high_24h":"76306.26",
            "best_bid":"75786.03","best_ask":"75786.04","side":"sell","time":"2026-11-06T21:21:05.556088Z",
            "trade_id":1116557012,"last_size":"0.026"}"#;
        let u = parse_message(text).unwrap().unwrap();
        assert_eq!(u.symbol, "BTC-USD");
        assert_eq!(u.price, 75786.03);
        assert_eq!(u.reference, Some(75709.41));
        assert_eq!(u.low, Some(74722.0));
        assert_eq!(u.at, "2026-11-06T21:21:05.556088Z".parse::<DateTime<Utc>>().unwrap());
    }

    #[test]
    fn ignores_control_messages_and_surfaces_errors() {
        assert!(parse_message(r#"{"type":"subscriptions","channels":[]}"#).unwrap().is_none());
        assert!(parse_message(r#"{"type":"heartbeat","product_id":"BTC-USD","sequence":1}"#).unwrap().is_none());
        assert!(parse_message("not json").unwrap().is_none());
        let err = parse_message(
            r#"{"type":"error","message":"Failed to subscribe","reason":"BAD-X is not a valid product"}"#,
        );
        assert!(err.unwrap_err().to_string().contains("BAD-X"));
    }

    #[test]
    fn snapshot_from_rest_payloads() {
        let stats: Stats =
            serde_json::from_str(r#"{"open":"100","high":"110","low":"90","last":"105","volume":"1"}"#).unwrap();
        let candles: Vec<Candle> = serde_json::from_str("[[1700000060,1,2,1,104,5],[1700000000,1,2,1,103,5]]").unwrap();
        let ev = snapshot_event("BTC-USD", &stats, &candles, Utc::now(), Some("Bitcoin".into()), Some(1e-8));
        let MarketEvent::Snapshot { update, history, name, qty_step } = ev else { panic!() };
        assert_eq!((update.price, update.reference, update.high), (105.0, Some(100.0), Some(110.0)));
        assert_eq!(history.len(), 2);
        assert_eq!(name.as_deref(), Some("Bitcoin"));
        assert_eq!(qty_step, Some(1e-8));
    }
}
