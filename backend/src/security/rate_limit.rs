//! In-memory keyed token-bucket rate limiter.
//!
//! Per-process: with several instances, put a shared limiter (e.g. at the
//! reverse proxy or Redis) in front as well.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

pub struct RateLimiter<K> {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<K, Bucket>>,
}

impl<K: Eq + Hash + Clone> RateLimiter<K> {
    /// Allow bursts of `capacity`, refilling `per_period` tokens every `period`.
    pub fn new(capacity: u32, per_period: u32, period: Duration) -> Self {
        Self {
            capacity: capacity as f64,
            refill_per_sec: per_period as f64 / period.as_secs_f64(),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Take one token. `Err(retry_after)` when the key is over its limit.
    pub fn check(&self, key: &K) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: &K, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap();
        // Bound memory: drop full (idle) buckets when the map grows large.
        if buckets.len() > 100_000 {
            let cap = self.capacity;
            let rate = self.refill_per_sec;
            buckets.retain(|_, b| b.tokens + now.duration_since(b.updated).as_secs_f64() * rate < cap);
        }
        let b = buckets.entry(key.clone()).or_insert(Bucket { tokens: self.capacity, updated: now });
        let elapsed = now.saturating_duration_since(b.updated).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        b.updated = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / self.refill_per_sec).max(Duration::from_secs(1)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_then_refills() {
        let rl = RateLimiter::new(3, 1, Duration::from_secs(1));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(rl.check_at(&"ip", t0).is_ok());
        }
        assert!(rl.check_at(&"ip", t0).is_err());
        assert!(rl.check_at(&"other", t0).is_ok(), "keys are independent");
        assert!(rl.check_at(&"ip", t0 + Duration::from_millis(1100)).is_ok());
        assert!(rl.check_at(&"ip", t0 + Duration::from_millis(1100)).is_err());
    }
}
