//! In-process event bus: market ticks (global) and account changes (scoped).
//! WebSocket sessions subscribe and filter to the accounts they may see.

use serde::Serialize;
use tokio::sync::broadcast;

use crate::domain::ids::AccountId;
use crate::market::Quote;

#[derive(Debug, Clone)]
pub enum Event {
    /// Latest quotes (sent every engine tick).
    Quotes(std::sync::Arc<Vec<Quote>>),
    /// Something changed for one account (fill, order, cash movement).
    Account { account_id: AccountId, change: AccountChange },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountChange {
    OrderPlaced { order_id: String },
    OrderFilled { order_id: String, symbol: String, side: String, quantity: String, price: String },
    OrderCancelled { order_id: String },
    CashMoved,
    Closed,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    pub fn publish(&self, e: Event) {
        let _ = self.tx.send(e); // no subscribers is fine
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}
