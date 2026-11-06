//! Shared application state: the composition root's output.

use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use sqlx::PgPool;

use crate::config::Config;
use crate::events::EventBus;
use crate::market::MarketStore;
use crate::market::hub::StatusMap;
use crate::security::rate_limit::RateLimiter;

pub type SharedState = Arc<AppState>;

pub struct Limits {
    /// All API requests, per client IP.
    pub api: RateLimiter<IpAddr>,
    /// Login/registration attempts, per client IP.
    pub auth_ip: RateLimiter<IpAddr>,
    /// Login attempts, per target email (slows distributed guessing).
    pub auth_email: RateLimiter<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            api: RateLimiter::new(120, 20, Duration::from_secs(1)),
            auth_ip: RateLimiter::new(10, 10, Duration::from_secs(60)),
            auth_email: RateLimiter::new(5, 5, Duration::from_secs(300)),
        }
    }
}

pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub market: Arc<RwLock<MarketStore>>,
    pub providers: StatusMap,
    pub events: EventBus,
    pub limits: Limits,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> SharedState {
        Arc::new(AppState {
            config: Arc::new(config),
            db,
            market: Arc::new(RwLock::new(MarketStore::new())),
            providers: StatusMap::default(),
            events: EventBus::new(),
            limits: Limits::default(),
        })
    }
}
