//! Sequenced dashboard event bus with a bounded replay ring buffer.
//!
//! Wraps `tokio::sync::broadcast::Sender<DashboardEvent>` with:
//! * A monotonic `u64` sequence number assigned at publish time.
//! * A bounded in-memory ring buffer of the last N events, used to replay
//!   missed events to a reconnecting client that presents `?since=<seq>`.
//! * A drop-in `send(event)` method so the 30+ existing publisher call sites
//!   don't need to change — only the field type on `AppState` flips.
//!
//! The bus is single-process: events are replayed only to clients reconnecting
//! against the same dashboard instance. If the process restarts, seq restarts
//! at 1 and clients will see every event that arrives after their reconnect
//! (no false-positive replay); a missed window is indistinguishable from a
//! dashboard restart and is handled the same way — the UI simply misses a
//! handful of events. For our usage (best-effort live streaming on top of a
//! durable DB) that's the right trade-off.

use networker_common::messages::DashboardEvent;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Maximum number of recent events held in the replay ring. At ~50 events/s
/// during a busy benchmark this buys ~40s of recent history — more than
/// enough for a WS reconnect after a transient network blip.
pub const EVENT_LOG_CAPACITY: usize = 2048;

/// A dashboard event tagged with a monotonically increasing sequence number.
///
/// Serialises as a flat object: `{"seq": 123, "type": "...", ...event-fields}`
/// thanks to `#[serde(flatten)]`. Browsers key off `seq` to request replay
/// on reconnect (`?since=<last_seen_seq>`).
#[derive(Clone, Debug, serde::Serialize)]
pub struct SeqEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub event: DashboardEvent,
}

/// Sequenced broadcast bus for `DashboardEvent`s with a bounded replay log.
///
/// Cheap to clone (all state is behind `Arc`s). `send()` is sync and never
/// blocks — ring-buffer locking uses `std::sync::Mutex` with microsecond-scope
/// critical sections.
#[derive(Clone)]
pub struct EventBus {
    seq: Arc<AtomicU64>,
    tx: broadcast::Sender<SeqEvent>,
    log: Arc<Mutex<VecDeque<SeqEvent>>>,
}

impl EventBus {
    /// Create a new bus. `channel_capacity` sizes the underlying
    /// `tokio::sync::broadcast` ring — if a slow consumer lags past this
    /// point, it sees `RecvError::Lagged` and can reconnect with `?since=`
    /// to recover via the replay log.
    pub fn new(channel_capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(channel_capacity);
        Self {
            seq: Arc::new(AtomicU64::new(0)),
            tx,
            log: Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_LOG_CAPACITY))),
        }
    }

    /// Publish an event. Assigns a fresh seq, pushes to the replay ring
    /// (evicting the oldest entry if full), then broadcasts. Returns the
    /// number of active subscribers the event was delivered to (0 when no
    /// client is attached).
    ///
    /// Never blocks, never returns an error — the event reaching *nobody* is
    /// a legitimate steady state, not a failure condition.
    pub fn send(&self, event: DashboardEvent) -> usize {
        // Relaxed is sound: seq assignment is the single source of ordering
        // truth — we only need monotonicity per-publish, not synchronisation
        // with other memory. fetch_add + 1 yields the pre-increment value,
        // so we add 1 to get the post-increment (first seq = 1, not 0).
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let seqe = SeqEvent { seq, event };
        {
            // Critical section kept tiny — push_back / pop_front only.
            let mut log = self.log.lock().expect("event_bus log mutex poisoned");
            if log.len() >= EVENT_LOG_CAPACITY {
                log.pop_front();
            }
            log.push_back(seqe.clone());
        }
        self.tx.send(seqe).unwrap_or(0)
    }

    /// Subscribe to new events. Returns a `broadcast::Receiver<SeqEvent>`.
    /// Each subscriber independently sees `Lagged` if it falls behind the
    /// channel capacity; the reconnect-with-`since` flow is the recovery path.
    pub fn subscribe(&self) -> broadcast::Receiver<SeqEvent> {
        self.tx.subscribe()
    }

    /// Snapshot of replay log entries with `seq > since`, oldest first.
    /// Returns an empty vec when `since` is ahead of or equal to the highest
    /// buffered seq (nothing to replay).
    ///
    /// Callers should subscribe *before* calling this: any event published
    /// between subscribe and replay-read will be in both streams, and the
    /// caller must dedupe by keeping track of the highest seq replayed and
    /// skipping incoming live events with seq <= that value.
    pub fn replay_since(&self, since: u64) -> Vec<SeqEvent> {
        let log = self.log.lock().expect("event_bus log mutex poisoned");
        log.iter().filter(|e| e.seq > since).cloned().collect()
    }

    /// Current seq value (post-increment — i.e. the seq of the most recently
    /// published event, or 0 if none). Exposed mainly for tests.
    #[cfg(test)]
    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(id: &str) -> DashboardEvent {
        DashboardEvent::DeployLog {
            deployment_id: uuid::Uuid::new_v4(),
            line: format!("line-{id}"),
            stream: "stdout".to_string(),
        }
    }

    #[test]
    fn send_assigns_monotonic_seq_starting_at_one() {
        let bus = EventBus::new(64);
        // No subscribers yet — send still increments seq and buffers.
        bus.send(sample_event("a"));
        bus.send(sample_event("b"));
        bus.send(sample_event("c"));
        assert_eq!(bus.current_seq(), 3, "three sends should yield seq=3");

        let all = bus.replay_since(0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[1].seq, 2);
        assert_eq!(all[2].seq, 3);
    }

    #[test]
    fn replay_since_returns_only_newer_events() {
        let bus = EventBus::new(64);
        for i in 0..5 {
            bus.send(sample_event(&i.to_string()));
        }
        let replay = bus.replay_since(3);
        assert_eq!(replay.len(), 2, "seq 4 and 5 should be returned");
        assert_eq!(replay[0].seq, 4);
        assert_eq!(replay[1].seq, 5);
    }

    #[test]
    fn replay_since_empty_when_since_is_at_or_ahead_of_head() {
        let bus = EventBus::new(64);
        bus.send(sample_event("a"));
        bus.send(sample_event("b"));
        assert!(
            bus.replay_since(2).is_empty(),
            "since == head yields nothing"
        );
        assert!(
            bus.replay_since(99).is_empty(),
            "since past head yields nothing"
        );
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let bus = EventBus::new(64);
        // Fill past capacity. We rely on EVENT_LOG_CAPACITY directly so the
        // test stays honest about the ring's declared size.
        for i in 0..(EVENT_LOG_CAPACITY + 5) {
            bus.send(sample_event(&i.to_string()));
        }
        let retained = bus.replay_since(0);
        assert_eq!(
            retained.len(),
            EVENT_LOG_CAPACITY,
            "ring buffer must cap at EVENT_LOG_CAPACITY"
        );
        // The oldest retained event is the first one that wasn't evicted.
        let first_seq = retained.first().expect("buffer is non-empty").seq;
        let last_seq = retained.last().expect("buffer is non-empty").seq;
        assert_eq!(last_seq - first_seq, (EVENT_LOG_CAPACITY - 1) as u64);
        assert_eq!(
            last_seq,
            (EVENT_LOG_CAPACITY + 5) as u64,
            "highest seq reflects total sends"
        );
    }

    #[tokio::test]
    async fn subscribers_receive_seq_on_live_events() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        bus.send(sample_event("live-1"));
        bus.send(sample_event("live-2"));
        let first = rx.recv().await.expect("first event");
        let second = rx.recv().await.expect("second event");
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
    }

    #[tokio::test]
    async fn subscribe_before_replay_read_means_dedup_by_max_seq_is_sufficient() {
        // Documents the subscribe-then-replay contract relied on by the WS
        // hub: a client that subscribes first, then reads the replay, and
        // skips incoming events with seq <= max_replayed, sees every event
        // exactly once.
        let bus = EventBus::new(64);
        // A few events before anyone subscribes.
        bus.send(sample_event("old-1"));
        bus.send(sample_event("old-2"));

        let mut rx = bus.subscribe();
        // An event published AFTER subscribe but BEFORE the replay snapshot
        // appears in both the log and the channel — the dedup rule must
        // drop the duplicate from the channel side.
        bus.send(sample_event("racy"));
        let replay = bus.replay_since(0);
        let max_replayed = replay.iter().map(|e| e.seq).max().unwrap_or(0);

        // Live event AFTER the snapshot must arrive via rx only.
        bus.send(sample_event("fresh"));

        // Drain rx; apply the dedup rule.
        let mut from_live = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if e.seq > max_replayed {
                from_live.push(e);
            }
        }

        let replay_seqs: Vec<u64> = replay.iter().map(|e| e.seq).collect();
        let live_seqs: Vec<u64> = from_live.iter().map(|e| e.seq).collect();
        assert_eq!(replay_seqs, vec![1, 2, 3], "replay covers old+racy");
        assert_eq!(
            live_seqs,
            vec![4],
            "dedup leaves only the post-snapshot event"
        );
    }
}
