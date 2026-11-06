//! The adapter interface every market-data source implements.

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{Instrument, MarketEvent};

/// A market-data adapter (Coinbase, Alpaca, a simulator, ...).
///
/// Lifecycle, driven by [`super::hub`]:
/// 1. [`instruments`](Self::instruments) is called once to register symbols.
/// 2. [`run`](Self::run) is called to connect and stream. It should send a
///    [`MarketEvent::Snapshot`] per symbol first (so charts and stats are
///    populated after every reconnect), then [`MarketEvent::Tick`]s.
/// 3. When `run` returns (error or clean close), the hub waits with
///    exponential backoff and calls `run` again.
///
/// Adapters only need to translate their wire format into `MarketEvent`s.
/// Reconnects, status tracking and storage are handled by the hub.
#[async_trait]
pub trait MarketDataProvider: Send + Sync + 'static {
    /// Stable id, e.g. "coinbase". Used as `Instrument::source`.
    fn id(&self) -> &str;

    /// Symbols this provider serves.
    fn instruments(&self) -> Vec<Instrument>;

    /// Width of history buckets in seconds (live ticks within one bucket update
    /// its last point rather than adding a new one).
    fn resolution_secs(&self) -> i64 {
        60
    }

    /// Connect and stream events into `sink` until the connection ends.
    async fn run(&self, sink: MarketSink) -> anyhow::Result<()>;
}

/// Where providers send events. Cheap to clone.
#[derive(Clone)]
pub struct MarketSink {
    pub(super) provider: String,
    pub(super) tx: mpsc::Sender<(String, MarketEvent)>,
}

impl MarketSink {
    /// Send an event. Fails only if the hub has shut down.
    pub async fn send(&self, event: MarketEvent) -> anyhow::Result<()> {
        self.tx
            .send((self.provider.clone(), event))
            .await
            .map_err(|_| anyhow::anyhow!("market hub closed"))
    }

    /// A sink plus the receiving end, for tests of individual adapters.
    pub fn channel(provider: &str) -> (Self, mpsc::Receiver<(String, MarketEvent)>) {
        let (tx, rx) = mpsc::channel(1024);
        (Self { provider: provider.into(), tx }, rx)
    }
}
