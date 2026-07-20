//! In-process, bounded safety limiters (contract v1 §6): token-bucket rate
//! limits (per-IP + global), concurrency caps, and the optional byte budget.
//!
//! All state is `O(bounded)`: the per-IP table is capped with LRU eviction so
//! an address-spraying attacker cannot grow memory (contract §6.2).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{ByteBudget, RateLimit};

/// A monotonic token bucket. `tokens` refills at `rps` per second up to `burst`.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
    last_touched: Instant,
}

impl Bucket {
    fn new(burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
            last_touched: now,
        }
    }

    /// Try to take one token. Returns `Ok(())` or `Err(retry_after)`.
    fn take(&mut self, limit: RateLimit, now: Instant) -> Result<(), Duration> {
        let burst = limit.burst as f64;
        let rps = limit.rps.max(1) as f64;
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rps).min(burst);
        self.last = now;
        self.last_touched = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let deficit = 1.0 - self.tokens;
            let secs = (deficit / rps).max(0.001);
            Err(Duration::from_secs_f64(secs))
        }
    }
}

/// Outcome of a rate-limit check.
pub enum RateOutcome {
    Allowed,
    Limited { retry_after: Duration },
}

/// Per-IP + global token-bucket rate limiter with a bounded IP table.
pub struct RateLimiter {
    per_ip: RateLimit,
    global: RateLimit,
    max_entries: usize,
    inner: Mutex<RateInner>,
}

struct RateInner {
    ips: HashMap<IpAddr, Bucket>,
    global: Bucket,
}

impl RateLimiter {
    pub fn new(per_ip: RateLimit, global: RateLimit, max_entries: usize) -> Self {
        let now = Instant::now();
        Self {
            per_ip,
            global,
            max_entries: max_entries.max(1),
            inner: Mutex::new(RateInner {
                ips: HashMap::new(),
                global: Bucket::new(global.burst as f64, now),
            }),
        }
    }

    /// Check both the global and the per-IP bucket. Both must have a token.
    /// The per-IP table evicts the least-recently-touched entry when full.
    pub fn check(&self, ip: IpAddr) -> RateOutcome {
        let now = Instant::now();
        // Poison recovery: a poisoned lock still yields usable state; we
        // fail-closed at the call site by treating a panic as internal, but the
        // mutex here only guards plain data, so recover the guard.
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Err(retry) = inner.global.take(self.global, now) {
            return RateOutcome::Limited { retry_after: retry };
        }

        // Evict LRU if we would exceed the cap and this is a new IP.
        if !inner.ips.contains_key(&ip) && inner.ips.len() >= self.max_entries {
            if let Some(oldest) = inner
                .ips
                .iter()
                .min_by_key(|(_, b)| b.last_touched)
                .map(|(k, _)| *k)
            {
                inner.ips.remove(&oldest);
            }
        }

        let per_ip = self.per_ip;
        let bucket = inner
            .ips
            .entry(ip)
            .or_insert_with(|| Bucket::new(per_ip.burst as f64, now));
        match bucket.take(per_ip, now) {
            Ok(()) => RateOutcome::Allowed,
            Err(retry) => RateOutcome::Limited { retry_after: retry },
        }
    }
}

/// Shared atomics for the concurrency caps.
struct Counters {
    in_flight: AtomicU32,
    transfers: AtomicU32,
}

/// An owned concurrency permit; releases its slot(s) on drop. Owning an `Arc`
/// (rather than borrowing) lets the permit live inside a streamed response body
/// so a transfer holds its slot for the whole transfer, not just handler setup.
pub struct Permit {
    counters: Arc<Counters>,
    transfer: bool,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.counters.in_flight.fetch_sub(1, Ordering::AcqRel);
        if self.transfer {
            self.counters.transfers.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Concurrency caps: overall in-flight and transfer-route in-flight (§6.3).
pub struct Concurrency {
    max: u32,
    max_transfers: u32,
    counters: Arc<Counters>,
}

impl Concurrency {
    pub fn new(max: u32, max_transfers: u32) -> Self {
        Self {
            max,
            max_transfers,
            counters: Arc::new(Counters {
                in_flight: AtomicU32::new(0),
                transfers: AtomicU32::new(0),
            }),
        }
    }

    /// Acquire a slot. `is_transfer` also consumes a transfer slot. Returns
    /// `None` if either relevant cap is already reached (caller -> 429).
    pub fn acquire(&self, is_transfer: bool) -> Option<Permit> {
        // Reserve the overall slot with a CAS loop.
        loop {
            let cur = self.counters.in_flight.load(Ordering::Acquire);
            if cur >= self.max {
                return None;
            }
            if self
                .counters
                .in_flight
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        if !is_transfer {
            return Some(Permit {
                counters: self.counters.clone(),
                transfer: false,
            });
        }
        // Reserve the transfer slot; roll back the overall slot on failure.
        loop {
            let cur = self.counters.transfers.load(Ordering::Acquire);
            if cur >= self.max_transfers {
                self.counters.in_flight.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
            if self
                .counters
                .transfers
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        Some(Permit {
            counters: self.counters.clone(),
            transfer: true,
        })
    }
}

/// Sliding-window byte budget for transfer routes (contract §6.4).
pub struct ByteBudgetTracker {
    budget: ByteBudget,
    inner: Mutex<BudgetInner>,
}

struct BudgetInner {
    window_start: Instant,
    used: u64,
}

impl ByteBudgetTracker {
    pub fn new(budget: ByteBudget) -> Self {
        Self {
            budget,
            inner: Mutex::new(BudgetInner {
                window_start: Instant::now(),
                used: 0,
            }),
        }
    }

    /// Reserve `bytes` against the current window. On exhaustion returns the
    /// window remainder as the `Retry-After` duration (contract §6.4).
    pub fn reserve(&self, bytes: u64) -> Result<(), Duration> {
        let now = Instant::now();
        let window = Duration::from_secs(self.budget.window_s.max(1));
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if now.saturating_duration_since(inner.window_start) >= window {
            inner.window_start = now;
            inner.used = 0;
        }
        if inner.used.saturating_add(bytes) > self.budget.bytes {
            let elapsed = now.saturating_duration_since(inner.window_start);
            let remainder = window.saturating_sub(elapsed);
            return Err(remainder);
        }
        inner.used = inner.used.saturating_add(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn per_ip_burst_then_limit() {
        let rl = RateLimiter::new(
            RateLimit { rps: 1, burst: 3 },
            RateLimit {
                rps: 1000,
                burst: 1000,
            },
            100,
        );
        let addr = ip(1);
        for _ in 0..3 {
            assert!(matches!(rl.check(addr), RateOutcome::Allowed));
        }
        assert!(matches!(rl.check(addr), RateOutcome::Limited { .. }));
    }

    #[test]
    fn ip_table_is_bounded() {
        let rl = RateLimiter::new(
            RateLimit { rps: 1, burst: 1 },
            RateLimit {
                rps: 100000,
                burst: 100000,
            },
            2,
        );
        for n in 0..10 {
            let _ = rl.check(ip(n));
        }
        let inner = rl.inner.lock().unwrap();
        assert!(inner.ips.len() <= 2);
    }

    #[test]
    fn concurrency_caps_overall_and_transfers() {
        let c = Concurrency::new(3, 1);
        let p1 = c.acquire(false).unwrap();
        let p2 = c.acquire(true).unwrap();
        // transfer cap = 1, so a second transfer is refused even with an
        // overall slot free.
        assert!(c.acquire(true).is_none());
        let _p3 = c.acquire(false).unwrap();
        // overall cap = 3, now full.
        assert!(c.acquire(false).is_none());
        drop(p1);
        drop(p2);
        assert!(c.acquire(true).is_some());
    }

    #[test]
    fn byte_budget_exhausts_and_reports_remainder() {
        let t = ByteBudgetTracker::new(ByteBudget {
            bytes: 100,
            window_s: 600,
        });
        assert!(t.reserve(60).is_ok());
        assert!(t.reserve(40).is_ok());
        let err = t.reserve(1).unwrap_err();
        assert!(err.as_secs() <= 600 && err.as_secs() > 0);
    }
}
