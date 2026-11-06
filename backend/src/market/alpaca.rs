//! Alpaca adapter: US equities (market data API v2).
//!
//! * REST `GET /v2/stocks/snapshots`: last trade, today's bar, previous close
//! * REST `GET /v2/stocks/{symbol}/bars?timeframe=1Min`: intraday history
//! * REST `GET /v2/assets/{symbol}` (trading API): company name
//! * WebSocket `/v2/{feed}`: `trades` and `dailyBars` subscriptions
//!
//! Needs a (free) Alpaca account. Keys are read from the environment:
//! `ALPACA_API_KEY_ID` / `ALPACA_API_SECRET_KEY` (or Alpaca's own
//! `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`). The free plan's `iex` feed is
//! real-time but covers only IEX volume. Use `sip` with a paid plan.
//!
//! Docs: <https://docs.alpaca.markets/docs/about-market-data-api>

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

use super::provider::{MarketDataProvider, MarketSink};
use super::{AssetClass, HISTORY_LEN, Instrument, MarketEvent, PricePoint, PriceUpdate};
use crate::net::{self, WsStream};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlpacaConfig {
    #[serde(default = "default_symbols")]
    pub symbols: Vec<String>,
    /// `iex` (free, real-time IEX only), `sip` (paid, all exchanges),
    /// `delayed_sip` (free, 15-min delayed), or `test` (fake symbol `FAKEPACA`, always on).
    #[serde(default = "default_feed")]
    pub feed: String,
    /// Smallest tradable quantity (1 = whole shares).
    #[serde(default = "default_qty_step")]
    pub qty_step: f64,
    #[serde(default = "default_data_url")]
    pub data_url: String,
    #[serde(default = "default_stream_url")]
    pub stream_url: String,
    #[serde(default = "default_trading_url")]
    pub trading_url: String,
    /// Names of the env vars holding the credentials.
    #[serde(default = "default_key_env")]
    pub key_id_env: String,
    #[serde(default = "default_secret_env")]
    pub secret_key_env: String,
}

impl Default for AlpacaConfig {
    fn default() -> Self {
        Self {
            symbols: default_symbols(),
            feed: default_feed(),
            qty_step: default_qty_step(),
            data_url: default_data_url(),
            stream_url: default_stream_url(),
            trading_url: default_trading_url(),
            key_id_env: default_key_env(),
            secret_key_env: default_secret_env(),
        }
    }
}

fn default_symbols() -> Vec<String> {
    ["AAPL", "MSFT", "NVDA", "AMZN", "TSLA", "SPY"].map(String::from).to_vec()
}
fn default_feed() -> String {
    "iex".into()
}
fn default_qty_step() -> f64 {
    1.0
}
fn default_data_url() -> String {
    "https://data.alpaca.markets".into()
}
fn default_stream_url() -> String {
    "wss://stream.data.alpaca.markets".into()
}
fn default_trading_url() -> String {
    "https://paper-api.alpaca.markets".into()
}
fn default_key_env() -> String {
    "ALPACA_API_KEY_ID".into()
}
fn default_secret_env() -> String {
    "ALPACA_API_SECRET_KEY".into()
}

/// Send a ping this often; the market is silent outside trading hours.
const PING_EVERY: Duration = Duration::from_secs(30);
/// Give up if nothing at all (not even a pong) arrives for this long.
const DEAD_AFTER: Duration = Duration::from_secs(90);
/// How far back to fetch minute bars (covers weekends and holidays).
const HISTORY_LOOKBACK_DAYS: i64 = 5;

pub struct AlpacaProvider {
    cfg: AlpacaConfig,
    key_id: String,
    secret: String,
    http: reqwest::Client,
    names: Mutex<HashMap<String, String>>,
}

fn env_secret(primary: &str, fallback: &str) -> Option<String> {
    [primary, fallback].iter().find_map(|n| std::env::var(n).ok()).filter(|v| !v.trim().is_empty())
}

impl AlpacaProvider {
    pub fn new(mut cfg: AlpacaConfig) -> anyhow::Result<Self> {
        let key_id = env_secret(&cfg.key_id_env, "APCA_API_KEY_ID")
            .with_context(|| format!("alpaca: set {} to enable US stocks", cfg.key_id_env))?;
        let secret = env_secret(&cfg.secret_key_env, "APCA_API_SECRET_KEY")
            .with_context(|| format!("alpaca: set {} to enable US stocks", cfg.secret_key_env))?;
        Self::with_keys(std::mem::take(&mut cfg), key_id, secret)
    }

    pub fn with_keys(mut cfg: AlpacaConfig, key_id: String, secret: String) -> anyhow::Result<Self> {
        cfg.symbols = cfg.symbols.iter().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();
        cfg.symbols.dedup();
        if cfg.symbols.is_empty() {
            bail!("alpaca: no symbols configured");
        }
        if !(cfg.qty_step.is_finite() && cfg.qty_step > 0.0) {
            bail!("alpaca: qty_step must be positive");
        }
        Ok(Self { cfg, key_id, secret, http: net::http_client(), names: Mutex::new(HashMap::new()) })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, base: &str, path: &str, query: &[(&str, String)]) -> anyhow::Result<T> {
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let res = self
            .http
            .get(&url)
            .query(query)
            .header("APCA-API-KEY-ID", &self.key_id)
            .header("APCA-API-SECRET-KEY", &self.secret)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            bail!("GET {url}: {status} {}", body.chars().take(200).collect::<String>());
        }
        res.json().await.with_context(|| format!("decode {url}"))
    }

    async fn name_of(&self, symbol: &str) -> Option<String> {
        if let Some(n) = self.names.lock().unwrap().get(symbol) {
            return Some(n.clone());
        }
        let asset: Asset = self.get(&self.cfg.trading_url, &format!("/v2/assets/{symbol}"), &[]).await.ok()?;
        self.names.lock().unwrap().insert(symbol.into(), asset.name.clone());
        Some(asset.name)
    }

    async fn history(&self, symbol: &str) -> anyhow::Result<Vec<PricePoint>> {
        let start = (Utc::now() - chrono::Duration::days(HISTORY_LOOKBACK_DAYS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let resp: BarsResponse = self
            .get(
                &self.cfg.data_url,
                &format!("/v2/stocks/{symbol}/bars"),
                &[
                    ("timeframe", "1Min".into()),
                    ("start", start),
                    ("limit", HISTORY_LEN.to_string()),
                    ("sort", "desc".into()),
                    ("feed", self.rest_feed().into()),
                ],
            )
            .await?;
        Ok(resp.bars.unwrap_or_default().into_iter().map(|b| PricePoint { t: b.t, price: b.c }).collect())
    }

    /// The `test` stream has no REST equivalent; fall back to IEX for REST.
    fn rest_feed(&self) -> &str {
        if self.cfg.feed == "test" { "iex" } else { &self.cfg.feed }
    }

    async fn snapshots(&self) -> anyhow::Result<Vec<MarketEvent>> {
        let snaps: HashMap<String, Snapshot> = self
            .get(
                &self.cfg.data_url,
                "/v2/stocks/snapshots",
                &[("symbols", self.cfg.symbols.join(",")), ("feed", self.rest_feed().into())],
            )
            .await?;
        let mut events = Vec::new();
        for symbol in &self.cfg.symbols {
            let Some(snap) = snaps.get(symbol) else {
                tracing::warn!("alpaca: no snapshot for {symbol} (unknown symbol or no data on feed {})", self.cfg.feed);
                continue;
            };
            let history = self.history(symbol).await.unwrap_or_else(|e| {
                tracing::warn!("alpaca: history for {symbol} failed: {e:#}");
                Vec::new()
            });
            if let Some(ev) = snapshot_event(symbol, snap, history, self.name_of(symbol).await) {
                events.push(ev);
            }
        }
        Ok(events)
    }

    async fn connect(&self) -> anyhow::Result<WsStream> {
        let url = format!("{}/v2/{}", self.cfg.stream_url.trim_end_matches('/'), self.cfg.feed);
        let mut ws = net::connect_ws(&url).await?;
        // Expect [{"T":"success","msg":"connected"}], then authenticate.
        expect_control(&mut ws, "connected").await?;
        let auth = serde_json::json!({ "action": "auth", "key": self.key_id, "secret": self.secret });
        ws.send(Message::Text(auth.to_string().into())).await?;
        expect_control(&mut ws, "authenticated").await?;
        let sub = serde_json::json!({ "action": "subscribe", "trades": self.cfg.symbols, "dailyBars": self.cfg.symbols });
        ws.send(Message::Text(sub.to_string().into())).await?;
        Ok(ws)
    }
}

async fn expect_control(ws: &mut WsStream, want: &str) -> anyhow::Result<()> {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .with_context(|| format!("timed out waiting for '{want}'"))?;
        let text = match msg {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(f))) => bail!("server closed connection: {f:?}"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => bail!("connection closed while waiting for '{want}'"),
        };
        for m in parse_messages(&text)? {
            match m {
                StreamMsg::Success(msg) if msg == want => return Ok(()),
                StreamMsg::Error(e) => bail!("{e}"),
                _ => {}
            }
        }
    }
}

#[async_trait]
impl MarketDataProvider for AlpacaProvider {
    fn id(&self) -> &str {
        "alpaca"
    }

    fn instruments(&self) -> Vec<Instrument> {
        self.cfg
            .symbols
            .iter()
            .map(|s| Instrument {
                symbol: s.clone(),
                name: s.clone(),
                asset_class: AssetClass::Equity,
                source: "alpaca".into(),
                reference_label: "Prev close".into(),
                qty_step: self.cfg.qty_step,
            })
            .collect()
    }

    async fn run(&self, sink: MarketSink) -> anyhow::Result<()> {
        let mut ws = self.connect().await?;
        for ev in self.snapshots().await? {
            sink.send(ev).await?;
        }

        let mut ping = tokio::time::interval(PING_EVERY);
        let mut last_rx = Instant::now();
        loop {
            tokio::select! {
                msg = ws.next() => {
                    last_rx = Instant::now();
                    let text = match msg {
                        Some(Ok(Message::Text(t))) => t,
                        Some(Ok(Message::Close(f))) => bail!("server closed connection: {f:?}"),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => return Err(e.into()),
                        None => return Ok(()),
                    };
                    for m in parse_messages(&text)? {
                        match m {
                            StreamMsg::Update(u) => sink.send(MarketEvent::Tick(u)).await?,
                            StreamMsg::Error(e) => bail!("{e}"),
                            StreamMsg::Success(_) | StreamMsg::Other => {}
                        }
                    }
                }
                _ = ping.tick() => {
                    if last_rx.elapsed() > DEAD_AFTER {
                        bail!("no data from alpaca for {DEAD_AFTER:?}");
                    }
                    ws.send(Message::Ping(Vec::new().into())).await?;
                }
            }
        }
    }
}

// ---- wire format -------------------------------------------------------------

#[derive(Deserialize)]
struct Asset {
    name: String,
}

#[derive(Deserialize)]
struct BarsResponse {
    bars: Option<Vec<Bar>>,
}

#[derive(Deserialize)]
struct Bar {
    t: DateTime<Utc>,
    #[serde(default)]
    h: f64,
    #[serde(default)]
    l: f64,
    c: f64,
}

#[derive(Deserialize)]
struct TradeRef {
    p: f64,
    t: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    latest_trade: Option<TradeRef>,
    daily_bar: Option<Bar>,
    prev_daily_bar: Option<Bar>,
}

fn snapshot_event(symbol: &str, s: &Snapshot, history: Vec<PricePoint>, name: Option<String>) -> Option<MarketEvent> {
    let (price, at) = match (&s.latest_trade, &s.daily_bar) {
        (Some(t), _) => (t.p, t.t),
        (None, Some(b)) => (b.c, b.t),
        (None, None) => return None,
    };
    let mut update = PriceUpdate::new(symbol, price, at);
    update.reference = s.prev_daily_bar.as_ref().map(|b| b.c);
    update.high = s.daily_bar.as_ref().map(|b| b.h);
    update.low = s.daily_bar.as_ref().map(|b| b.l);
    Some(MarketEvent::Snapshot { update, history, name, qty_step: None })
}

#[derive(Deserialize)]
#[serde(tag = "T")]
enum RawMsg {
    #[serde(rename = "success")]
    Success { msg: String },
    #[serde(rename = "error")]
    Error { code: i64, msg: String },
    #[serde(rename = "t")]
    Trade {
        #[serde(rename = "S")]
        symbol: String,
        p: f64,
        t: DateTime<Utc>,
    },
    #[serde(rename = "d")]
    DailyBar {
        #[serde(rename = "S")]
        symbol: String,
        h: f64,
        l: f64,
        c: f64,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, PartialEq)]
enum StreamMsg {
    Success(String),
    Error(String),
    Update(PriceUpdate),
    Other,
}

/// The stream sends JSON arrays of messages.
fn parse_messages(text: &str) -> anyhow::Result<Vec<StreamMsg>> {
    let raw: Vec<serde_json::Value> = serde_json::from_str(text).context("alpaca: malformed message")?;
    Ok(raw
        .into_iter()
        .map(|v| match serde_json::from_value::<RawMsg>(v) {
            Ok(RawMsg::Success { msg }) => StreamMsg::Success(msg),
            Ok(RawMsg::Error { code, msg }) => StreamMsg::Error(format!("{msg} (code {code})")),
            Ok(RawMsg::Trade { symbol, p, t }) => StreamMsg::Update(PriceUpdate::new(symbol, p, t)),
            Ok(RawMsg::DailyBar { symbol, h, l, c }) => {
                // A daily bar's timestamp is the start of the day; stamp it "now"
                // so it isn't discarded as older than the last trade.
                let mut u = PriceUpdate::new(symbol, c, Utc::now());
                u.high = Some(h);
                u.low = Some(l);
                StreamMsg::Update(u)
            }
            Ok(RawMsg::Other) | Err(_) => StreamMsg::Other,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_messages() {
        let msgs = parse_messages(
            r#"[{"T":"success","msg":"authenticated"},
                {"T":"t","S":"AAPL","i":52983525029461,"x":"V","p":226.55,"s":100,"t":"2026-11-06T15:51:44.208Z","c":["@"],"z":"C"},
                {"T":"d","S":"AAPL","o":220.1,"h":228.0,"l":219.5,"c":226.5,"v":100,"t":"2026-11-06T05:00:00Z"},
                {"T":"subscription","trades":["AAPL"]},
                {"T":"error","code":402,"msg":"auth failed"}]"#,
        )
        .unwrap();
        assert_eq!(msgs[0], StreamMsg::Success("authenticated".into()));
        let StreamMsg::Update(t) = &msgs[1] else { panic!() };
        assert_eq!((t.symbol.as_str(), t.price), ("AAPL", 226.55));
        let StreamMsg::Update(d) = &msgs[2] else { panic!() };
        assert_eq!((d.high, d.low, d.price), (Some(228.0), Some(219.5), 226.5));
        assert_eq!(msgs[3], StreamMsg::Other);
        assert_eq!(msgs[4], StreamMsg::Error("auth failed (code 402)".into()));
    }

    #[test]
    fn snapshot_uses_prev_close_as_reference() {
        let s: Snapshot = serde_json::from_str(
            r#"{"latestTrade":{"t":"2026-11-06T20:59:59Z","p":226.5,"s":5},
                "dailyBar":{"t":"2026-11-06T05:00:00Z","o":220,"h":228,"l":219,"c":226,"v":1},
                "prevDailyBar":{"t":"2026-11-05T05:00:00Z","o":218,"h":222,"l":217,"c":221,"v":1}}"#,
        )
        .unwrap();
        let ev = snapshot_event("AAPL", &s, vec![], Some("Apple Inc.".into())).unwrap();
        let MarketEvent::Snapshot { update, name, .. } = ev else { panic!() };
        assert_eq!(update.price, 226.5);
        assert_eq!(update.reference, Some(221.0));
        assert_eq!((update.high, update.low), (Some(228.0), Some(219.0)));
        assert_eq!(name.as_deref(), Some("Apple Inc."));
    }

    /// End-to-end against a local mock of Alpaca's REST + stream APIs.
    mod mock {
        use super::super::*;
        use axum::extract::ws::{Message as AxMsg, WebSocketUpgrade};
        use axum::extract::{Path, Query};
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::get;
        use axum::{Json, Router};
        use serde_json::json;

        const KEY: &str = "PKTEST";
        const SECRET: &str = "s3cret";

        fn authed(h: &HeaderMap) -> bool {
            h.get("APCA-API-KEY-ID").is_some_and(|v| v == KEY) && h.get("APCA-API-SECRET-KEY").is_some_and(|v| v == SECRET)
        }

        async fn serve() -> String {
            let app = Router::new()
                .route("/v2/stocks/snapshots", get(|h: HeaderMap, Query(q): Query<HashMap<String, String>>| async move {
                    if !authed(&h) { return StatusCode::UNAUTHORIZED.into_response(); }
                    assert_eq!(q["symbols"], "AAPL,MSFT");
                    assert_eq!(q["feed"], "iex");
                    Json(json!({
                        "AAPL": {
                            "latestTrade": {"t": "2026-11-06T20:59:00Z", "p": 226.5, "s": 1},
                            "dailyBar": {"t": "2026-11-06T05:00:00Z", "o": 220, "h": 228, "l": 219, "c": 226, "v": 1},
                            "prevDailyBar": {"t": "2026-11-05T05:00:00Z", "o": 218, "h": 222, "l": 217, "c": 221, "v": 1}
                        }
                        // MSFT deliberately missing: adapter should skip it.
                    })).into_response()
                }))
                .route("/v2/stocks/{sym}/bars", get(|h: HeaderMap, Path(sym): Path<String>| async move {
                    if !authed(&h) { return StatusCode::UNAUTHORIZED.into_response(); }
                    Json(json!({"symbol": sym, "next_page_token": null, "bars": [
                        {"t": "2026-11-06T20:58:00Z", "o": 1, "h": 1, "l": 1, "c": 226.1, "v": 1},
                        {"t": "2026-11-06T20:57:00Z", "o": 1, "h": 1, "l": 1, "c": 225.9, "v": 1}
                    ]})).into_response()
                }))
                .route("/v2/assets/{sym}", get(|Path(sym): Path<String>| async move {
                    Json(json!({"symbol": sym, "name": "Apple Inc. Common Stock"}))
                }))
                .route("/v2/iex", get(|ws: WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket| async move {
                        let send = |v: serde_json::Value| AxMsg::Text(v.to_string().into());
                        socket.send(send(json!([{"T": "success", "msg": "connected"}]))).await.unwrap();
                        let Some(Ok(AxMsg::Text(auth))) = socket.recv().await else { return };
                        let auth: serde_json::Value = serde_json::from_str(&auth).unwrap();
                        if auth["key"] != KEY || auth["secret"] != SECRET {
                            socket.send(send(json!([{"T": "error", "code": 402, "msg": "auth failed"}]))).await.unwrap();
                            return;
                        }
                        socket.send(send(json!([{"T": "success", "msg": "authenticated"}]))).await.unwrap();
                        let Some(Ok(AxMsg::Text(sub))) = socket.recv().await else { return };
                        let sub: serde_json::Value = serde_json::from_str(&sub).unwrap();
                        assert_eq!(sub["trades"], json!(["AAPL", "MSFT"]));
                        socket.send(send(json!([{"T": "subscription", "trades": ["AAPL", "MSFT"]}]))).await.unwrap();
                        socket.send(send(json!([
                            {"T": "t", "S": "AAPL", "p": 227.25, "s": 10, "t": "2026-11-06T20:59:30Z"},
                            {"T": "t", "S": "AAPL", "p": 227.5, "s": 10, "t": "2026-11-06T20:59:31Z"}
                        ]))).await.unwrap();
                        // Keep the connection open.
                        while socket.recv().await.is_some() {}
                    })
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            format!("127.0.0.1:{}", addr.port())
        }

        fn provider(addr: &str, secret: &str) -> AlpacaProvider {
            let cfg = AlpacaConfig {
                symbols: vec!["aapl".into(), "MSFT".into()],
                data_url: format!("http://{addr}"),
                trading_url: format!("http://{addr}"),
                stream_url: format!("ws://{addr}"),
                ..Default::default()
            };
            AlpacaProvider::with_keys(cfg, KEY.into(), secret.into()).unwrap()
        }

        #[tokio::test]
        async fn streams_snapshot_then_trades() {
            let addr = serve().await;
            let p = std::sync::Arc::new(provider(&addr, SECRET));
            let (sink, mut rx) = MarketSink::channel("alpaca");
            let task = tokio::spawn({ let p = p.clone(); async move { p.run(sink).await } });

            async fn recv(rx: &mut tokio::sync::mpsc::Receiver<(String, MarketEvent)>) -> MarketEvent {
                tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap().1
            }
            let snap = recv(&mut rx).await;
            let MarketEvent::Snapshot { update, history, name, .. } = snap else { panic!("{snap:?}") };
            assert_eq!((update.symbol.as_str(), update.price, update.reference), ("AAPL", 226.5, Some(221.0)));
            assert_eq!(history.len(), 2);
            assert_eq!(name.as_deref(), Some("Apple Inc. Common Stock"));

            let MarketEvent::Tick(t1) = recv(&mut rx).await else { panic!() };
            let MarketEvent::Tick(t2) = recv(&mut rx).await else { panic!() };
            assert_eq!((t1.price, t2.price), (227.25, 227.5));
            task.abort();
        }

        #[tokio::test]
        async fn bad_credentials_fail_fast() {
            let addr = serve().await;
            let p = provider(&addr, "wrong");
            let (sink, _rx) = MarketSink::channel("alpaca");
            let err = tokio::time::timeout(Duration::from_secs(5), p.run(sink)).await.unwrap().unwrap_err();
            assert!(err.to_string().contains("auth failed"), "{err}");
        }
    }

    #[test]
    fn missing_keys_is_a_clear_error() {
        let cfg = AlpacaConfig { key_id_env: "LEDGER_TEST_NO_SUCH_VAR".into(), ..Default::default() };
        // Only meaningful when the fallback var isn't set in the environment.
        if std::env::var("APCA_API_KEY_ID").is_err() {
            let err = AlpacaProvider::new(cfg).err().unwrap().to_string();
            assert!(err.contains("LEDGER_TEST_NO_SUCH_VAR"), "{err}");
        }
    }
}
