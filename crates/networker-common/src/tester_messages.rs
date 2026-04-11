//! WebSocket protocol for tester queue + phase updates.

use serde::{Deserialize, Serialize};

use crate::phase::{Outcome, Phase};

/// Info about a benchmark currently running or waiting on a tester.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueEntry {
    pub config_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TesterMessage {
    SubscribeTesterQueue {
        project_id: String,
        tester_ids: Vec<String>,
    },
    UnsubscribeTesterQueue {
        tester_ids: Vec<String>,
    },
    TesterQueueSnapshot {
        project_id: String,
        tester_id: String,
        seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        running: Option<QueueEntry>,
        queued: Vec<QueueEntry>,
    },
    TesterQueueUpdate {
        project_id: String,
        tester_id: String,
        seq: u64,
        trigger: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        running: Option<QueueEntry>,
        queued: Vec<QueueEntry>,
    },
    PhaseUpdate {
        project_id: String,
        entity_type: String,
        entity_id: String,
        seq: u64,
        phase: Phase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        applied_stages: Vec<Phase>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::{Outcome, Phase};

    fn rt(m: &TesterMessage) -> TesterMessage {
        let json = serde_json::to_string(m).expect("serialize");
        serde_json::from_str::<TesterMessage>(&json).expect("deserialize")
    }

    #[test]
    fn subscribe_round_trip() {
        let m = TesterMessage::SubscribeTesterQueue {
            project_id: "p-abc".into(),
            tester_ids: vec!["t-1".into(), "t-2".into()],
        };
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn unsubscribe_round_trip() {
        let m = TesterMessage::UnsubscribeTesterQueue {
            tester_ids: vec!["t-1".into()],
        };
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn snapshot_round_trip() {
        let m = TesterMessage::TesterQueueSnapshot {
            project_id: "p".into(),
            tester_id: "t".into(),
            seq: 42,
            running: Some(QueueEntry {
                config_id: "c-1".into(),
                name: "bench A".into(),
                position: None,
                eta_seconds: None,
            }),
            queued: vec![QueueEntry {
                config_id: "c-2".into(),
                name: "bench B".into(),
                position: Some(1),
                eta_seconds: Some(600),
            }],
        };
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn update_round_trip() {
        let m = TesterMessage::TesterQueueUpdate {
            project_id: "p".into(),
            tester_id: "t".into(),
            seq: 7,
            trigger: "benchmark_completed".into(),
            running: None,
            queued: vec![],
        };
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn phase_update_round_trip() {
        let m = TesterMessage::PhaseUpdate {
            project_id: "p".into(),
            entity_type: "benchmark".into(),
            entity_id: "cfg-1".into(),
            seq: 3,
            phase: Phase::Running,
            outcome: Some(Outcome::Success),
            message: Some("all good".into()),
            applied_stages: vec![Phase::Queued, Phase::Starting, Phase::Deploy, Phase::Running],
        };
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn tag_is_snake_case() {
        let m = TesterMessage::SubscribeTesterQueue {
            project_id: "p".into(),
            tester_ids: vec![],
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains(r#""type":"subscribe_tester_queue""#),
            "got {json}"
        );
    }
}
