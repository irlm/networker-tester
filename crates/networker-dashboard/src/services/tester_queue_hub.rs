//! In-process tester queue pub/sub hub.
//!
//! Publishers (background services) call `notify(project_id, tester_id, trigger)`,
//! which constructs a `TesterQueueUpdate` and broadcasts to subscribers keyed by
//! `(project_id, tester_id)`. Subscribers are WebSocket handlers (Task 21) that
//! hold a `tokio::sync::mpsc::Sender<TesterMessage>`.

#![allow(dead_code)] // publishers wired in Task 34

use std::collections::HashMap;
use std::sync::Arc;

use networker_common::tester_messages::{QueueEntry, TesterMessage};
use tokio::sync::{mpsc, RwLock};

pub const DEFAULT_MAX_SUBS_PER_PROJECT: usize = 50;

type SubKey = (String, String);
type SubEntry = (u64, mpsc::Sender<TesterMessage>);

/// One subscription = one open WS connection asking for updates on a (project, tester).
pub struct Subscription {
    pub id: u64,
    pub project_id: String,
    pub tester_id: String,
    pub sender: mpsc::Sender<TesterMessage>,
}

pub struct TesterQueueHub {
    inner: Arc<RwLock<HubState>>,
    max_subs_per_project: usize,
}

struct HubState {
    next_id: u64,
    /// keyed by (project_id, tester_id) → list of (sub_id, sender)
    subscribers: HashMap<SubKey, Vec<SubEntry>>,
    /// monotonic seq per tester
    seq: HashMap<String, u64>,
    /// count of subscriptions per project (for rate limiting)
    project_sub_counts: HashMap<String, usize>,
}

impl TesterQueueHub {
    pub fn new() -> Self {
        let max = std::env::var("DASHBOARD_MAX_SUBS_PER_PROJECT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_SUBS_PER_PROJECT);
        Self {
            inner: Arc::new(RwLock::new(HubState {
                next_id: 1,
                subscribers: HashMap::new(),
                seq: HashMap::new(),
                project_sub_counts: HashMap::new(),
            })),
            max_subs_per_project: max,
        }
    }

    /// Subscribe a sender to (project_id, tester_id). Returns the subscription id
    /// (used for later unsubscribe) and the current `seq` value for snapshot use.
    /// Returns `Err` if the project subscription limit is exceeded.
    pub async fn subscribe(
        &self,
        project_id: &str,
        tester_id: &str,
        sender: mpsc::Sender<TesterMessage>,
    ) -> anyhow::Result<(u64, u64)> {
        let mut g = self.inner.write().await;
        let project_count = g
            .project_sub_counts
            .entry(project_id.to_string())
            .or_insert(0);
        if *project_count >= self.max_subs_per_project {
            anyhow::bail!(
                "subscription limit reached for project {} ({})",
                project_id,
                self.max_subs_per_project
            );
        }
        *project_count += 1;

        let id = g.next_id;
        g.next_id += 1;

        let key = (project_id.to_string(), tester_id.to_string());
        g.subscribers.entry(key).or_default().push((id, sender));

        let seq = *g.seq.entry(tester_id.to_string()).or_insert(0);
        Ok((id, seq))
    }

    pub async fn unsubscribe(&self, project_id: &str, tester_id: &str, sub_id: u64) {
        let mut g = self.inner.write().await;
        let key = (project_id.to_string(), tester_id.to_string());
        if let Some(list) = g.subscribers.get_mut(&key) {
            list.retain(|(id, _)| *id != sub_id);
            if list.is_empty() {
                g.subscribers.remove(&key);
            }
        }
        if let Some(c) = g.project_sub_counts.get_mut(project_id) {
            if *c > 0 {
                *c -= 1;
            }
        }
    }

    /// Publish a queue update to all subscribers of (project_id, tester_id).
    /// Bumps the tester's seq counter.
    ///
    /// RR-018: the sender list is snapshot-cloned under the lock, the lock
    /// is released, and then we iterate and call `try_send` without the
    /// lock held. The previous implementation held a write lock across
    /// every try_send, meaning a single slow subscriber could block every
    /// other publisher on the hub.
    ///
    /// RR-010: a subscriber whose channel is `Full` is now treated as
    /// dead. We drop its sender (which closes the channel from the
    /// receiver's side on next poll) and increment a dropped-slow
    /// counter via a tracing event. The WS handler will observe the
    /// closed channel, detect the seq gap on reconnect, and request a
    /// snapshot replay.
    pub async fn notify(
        &self,
        project_id: &str,
        tester_id: &str,
        trigger: &str,
        running: Option<QueueEntry>,
        queued: Vec<QueueEntry>,
    ) {
        let key = (project_id.to_string(), tester_id.to_string());

        // Phase 1: under the lock, bump seq, clone the sender list, drop lock.
        let (message, senders_snapshot) = {
            let mut g = self.inner.write().await;
            let seq_ref = g.seq.entry(tester_id.to_string()).or_insert(0);
            *seq_ref += 1;
            let new_seq = *seq_ref;

            let message = TesterMessage::TesterQueueUpdate {
                project_id: project_id.to_string(),
                tester_id: tester_id.to_string(),
                seq: new_seq,
                trigger: trigger.to_string(),
                running,
                queued,
            };

            let senders: Vec<SubEntry> = g.subscribers.get(&key).cloned().unwrap_or_default();
            (message, senders)
        };

        if senders_snapshot.is_empty() {
            return;
        }

        // Phase 2: try_send outside the lock. Collect dead ids for pruning.
        let mut dead: Vec<u64> = Vec::new();
        let mut dropped_slow = 0usize;
        for (id, sender) in &senders_snapshot {
            match sender.try_send(message.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => dead.push(*id),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        target: "tester_queue_hub_slow_subscriber",
                        sub_id = id,
                        project_id = project_id,
                        tester_id = tester_id,
                        "dropping slow subscriber — will force snapshot replay on reconnect"
                    );
                    dropped_slow += 1;
                    dead.push(*id);
                }
            }
        }

        if dropped_slow > 0 {
            tracing::warn!(
                target: "tester_queue_hub_dropped_slow",
                project_id = project_id,
                tester_id = tester_id,
                dropped = dropped_slow,
                "tester queue hub dropped {dropped_slow} slow subscribers"
            );
        }

        if dead.is_empty() {
            return;
        }

        // Phase 3: re-acquire lock briefly to prune dead subscribers. Note
        // that another task may have unsubscribed or modified the list
        // between phases 1 and 3; we tolerate that via `retain`.
        let mut g = self.inner.write().await;
        let mut pruned = 0usize;
        if let Some(list) = g.subscribers.get_mut(&key) {
            let before = list.len();
            list.retain(|(id, _)| !dead.contains(id));
            pruned = before - list.len();
            if list.is_empty() {
                g.subscribers.remove(&key);
            }
        }
        if pruned > 0 {
            if let Some(c) = g.project_sub_counts.get_mut(project_id) {
                *c = c.saturating_sub(pruned);
            }
        }
    }

    /// Current seq for a tester (for building snapshots).
    pub async fn current_seq(&self, tester_id: &str) -> u64 {
        let g = self.inner.read().await;
        g.seq.get(tester_id).copied().unwrap_or(0)
    }
}

impl Default for TesterQueueHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl TesterQueueHub {
    pub fn with_max_subs_per_project(max: usize) -> Self {
        let mut hub = Self::new();
        hub.max_subs_per_project = max;
        hub
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn subscribe_increments_and_assigns_id() {
        let hub = TesterQueueHub::new();
        let (tx, _rx) = mpsc::channel(4);
        let (id, seq) = hub.subscribe("p1", "t1", tx).await.unwrap();
        assert_eq!(id, 1);
        assert_eq!(seq, 0);
    }

    #[tokio::test]
    async fn subscribe_respects_project_limit() {
        let hub = TesterQueueHub::with_max_subs_per_project(2);
        let (tx1, _r1) = mpsc::channel(4);
        let (tx2, _r2) = mpsc::channel(4);
        let (tx3, _r3) = mpsc::channel(4);
        hub.subscribe("p1", "t1", tx1).await.unwrap();
        hub.subscribe("p1", "t2", tx2).await.unwrap();
        assert!(hub.subscribe("p1", "t3", tx3).await.is_err());
    }

    #[tokio::test]
    async fn notify_delivers_to_matching_subscribers() {
        let hub = TesterQueueHub::new();
        let (tx1, mut r1) = mpsc::channel(4);
        let (tx2, mut r2) = mpsc::channel(4);
        hub.subscribe("p1", "t1", tx1).await.unwrap();
        hub.subscribe("p1", "t1", tx2).await.unwrap();

        hub.notify("p1", "t1", "benchmark_queued", None, vec![])
            .await;

        let m1 = r1.recv().await.unwrap();
        let m2 = r2.recv().await.unwrap();
        match m1 {
            TesterMessage::TesterQueueUpdate { seq, .. } => assert_eq!(seq, 1),
            _ => panic!("expected TesterQueueUpdate"),
        }
        match m2 {
            TesterMessage::TesterQueueUpdate { seq, .. } => assert_eq!(seq, 1),
            _ => panic!("expected TesterQueueUpdate"),
        }
    }

    #[tokio::test]
    async fn notify_skips_other_projects() {
        let hub = TesterQueueHub::new();
        let (tx, mut rx) = mpsc::channel(4);
        hub.subscribe("p1", "t1", tx).await.unwrap();

        hub.notify("p2", "t1", "benchmark_queued", None, vec![])
            .await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
                .await
                .is_err(),
            "should not receive cross-project update"
        );
    }

    #[tokio::test]
    async fn unsubscribe_removes_sender() {
        let hub = TesterQueueHub::new();
        let (tx, mut rx) = mpsc::channel(4);
        let (id, _) = hub.subscribe("p1", "t1", tx).await.unwrap();
        hub.unsubscribe("p1", "t1", id).await;
        hub.notify("p1", "t1", "x", None, vec![]).await;
        // After unsubscribe, hub drops its sender → channel closes → recv returns None.
        // We must not observe a TesterQueueUpdate.
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        match result {
            Ok(None) => {} // channel closed, as expected
            Ok(Some(msg)) => panic!("expected no delivery after unsubscribe, got {msg:?}"),
            Err(_) => {} // timeout also acceptable (no delivery)
        }
    }
}
