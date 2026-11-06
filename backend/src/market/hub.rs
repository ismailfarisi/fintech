//! Runs providers: registers their symbols, supervises reconnects, and applies
//! their events to the shared [`MarketStore`].

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::provider::{MarketDataProvider, MarketSink};
use super::{MarketEvent, MarketStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderState {
    Connecting,
    Live,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub id: String,
    pub state: ProviderState,
    pub symbols: Vec<String>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub reconnects: u32,
}

pub type StatusMap = Arc<RwLock<BTreeMap<String, ProviderStatus>>>;

/// Register every provider's instruments and start streaming.
/// Returns the task handles (useful for tests; normally left running).
pub fn start(
    providers: Vec<Arc<dyn MarketDataProvider>>,
    store: Arc<RwLock<MarketStore>>,
    status: StatusMap,
) -> Vec<JoinHandle<()>> {
    let (tx, mut rx) = mpsc::channel::<(String, MarketEvent)>(4096);
    let mut handles = Vec::new();

    for p in providers {
        let id = p.id().to_string();
        let mut symbols = Vec::new();
        {
            let mut s = store.write().unwrap();
            for inst in p.instruments() {
                let sym = inst.symbol.clone();
                if s.register(inst, p.resolution_secs()) {
                    symbols.push(sym);
                } else {
                    tracing::warn!(
                        "{id}: symbol {sym} already provided by {}; skipping",
                        s.source_of(&sym).unwrap_or("?")
                    );
                }
            }
        }
        tracing::info!("market data: {id} serving {}", symbols.join(", "));
        status.write().unwrap().insert(
            id.clone(),
            ProviderStatus {
                id: id.clone(),
                state: ProviderState::Connecting,
                symbols,
                last_event_at: None,
                last_error: None,
                reconnects: 0,
            },
        );
        let sink = MarketSink { provider: id, tx: tx.clone() };
        handles.push(tokio::spawn(supervise(p, sink, status.clone())));
    }
    drop(tx);

    // Single consumer: apply events in arrival order.
    handles.push(tokio::spawn(async move {
        while let Some((provider, event)) = rx.recv().await {
            {
                let mut s = store.write().unwrap();
                // A provider may only update symbols it owns.
                if s.source_of(event.symbol()) != Some(provider.as_str()) {
                    continue;
                }
                s.apply(event);
            }
            if let Some(st) = status.write().unwrap().get_mut(&provider) {
                st.state = ProviderState::Live;
                st.last_event_at = Some(Utc::now());
            }
        }
    }));
    handles
}

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A session longer than this counts as healthy and resets the backoff.
const HEALTHY_SESSION: Duration = Duration::from_secs(60);

async fn supervise(provider: Arc<dyn MarketDataProvider>, sink: MarketSink, status: StatusMap) {
    let id = provider.id().to_string();
    let mut backoff = MIN_BACKOFF;
    loop {
        let started = Instant::now();
        let result = provider.run(sink.clone()).await;
        if sink.tx.is_closed() {
            return;
        }
        let message = match result {
            Ok(()) => "connection closed".to_string(),
            Err(e) => format!("{e:#}"),
        };
        if started.elapsed() > HEALTHY_SESSION {
            backoff = MIN_BACKOFF;
        }
        tracing::warn!("market data: {id}: {message}; reconnecting in {backoff:?}");
        if let Some(st) = status.write().unwrap().get_mut(&id) {
            st.state = ProviderState::Reconnecting;
            st.last_error = Some(message);
            st.reconnects += 1;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::{AssetClass, Instrument, PriceUpdate};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Fails on the first run, then emits one tick and stays connected.
    struct Flaky {
        id: &'static str,
        runs: AtomicU32,
        symbols: Vec<&'static str>,
    }

    #[async_trait]
    impl MarketDataProvider for Flaky {
        fn id(&self) -> &str {
            self.id
        }
        fn instruments(&self) -> Vec<Instrument> {
            self.symbols
                .iter()
                .map(|s| Instrument {
                    symbol: s.to_string(),
                    name: s.to_string(),
                    asset_class: AssetClass::Equity,
                    source: self.id.into(),
                    reference_label: "Prev close".into(),
                    qty_step: 1.0,
                })
                .collect()
        }
        async fn run(&self, sink: MarketSink) -> anyhow::Result<()> {
            if self.runs.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("boom");
            }
            for s in &self.symbols {
                sink.send(MarketEvent::Tick(PriceUpdate::new(*s, 10.0, Utc::now()))).await?;
            }
            // Also try to update a symbol owned by someone else.
            sink.send(MarketEvent::Tick(PriceUpdate::new("OWNED", 999.0, Utc::now()))).await?;
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn reconnects_after_failure_and_enforces_ownership() {
        let store = Arc::new(RwLock::new(MarketStore::new()));
        let status: StatusMap = Default::default();
        let a = Arc::new(Flaky { id: "a", runs: AtomicU32::new(0), symbols: vec!["AAA", "OWNED"] });
        let b = Arc::new(Flaky { id: "b", runs: AtomicU32::new(0), symbols: vec!["OWNED", "BBB"] });
        let handles = start(vec![a, b], store.clone(), status.clone());

        // First run fails, hub retries after 1s backoff.
        tokio::time::timeout(Duration::from_secs(5), async {
            while store.read().unwrap().price("BBB").is_none() || store.read().unwrap().price("AAA").is_none() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("providers should recover");

        let s = store.read().unwrap();
        assert_eq!(s.source_of("OWNED"), Some("a"), "first provider keeps a duplicate symbol");
        let st = status.read().unwrap();
        assert_eq!(st["a"].state, ProviderState::Live);
        assert_eq!(st["a"].reconnects, 1);
        assert_eq!(st["a"].last_error.as_deref(), Some("boom"));
        assert_eq!(st["b"].symbols, vec!["BBB"]);
        drop((s, st));
        for h in handles {
            h.abort();
        }
    }
}
