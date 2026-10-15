//! A small in-memory token bucket per key (per IP for sign-in, per account for calls).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{ApiError, ApiResult};

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl RateLimiter {
    /// Allows `limit` requests per `window` for `key`, refilling continuously.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> ApiResult<()> {
        let now = Instant::now();
        let rate = limit as f64 / window.as_secs_f64();
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if buckets.len() > 50_000 {
            buckets.retain(|_, (_, last)| now.duration_since(*last) < window);
        }
        let (tokens, last) = buckets
            .entry(key.to_string())
            .or_insert((limit as f64, now));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * rate).min(limit as f64);
        *last = now;
        if *tokens < 1.0 {
            return Err(ApiError::rate_limited());
        }
        *tokens -= 1.0;
        Ok(())
    }
}
