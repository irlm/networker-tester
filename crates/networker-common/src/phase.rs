//! Unified lifecycle enums shared across orchestrator, dashboard, agent, and CLI.
//!
//! `Phase` tracks where a persistent tester job is in its lifecycle; `Outcome`
//! records the terminal result. Both serialize as lowercase strings so they can
//! travel over the WebSocket protocol and be stored directly in the database.

use serde::{Deserialize, Serialize};

/// Lifecycle phase of a persistent tester job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Queued,
    Starting,
    Deploy,
    Running,
    Collect,
    Done,
}

impl Phase {
    /// Returns the lowercase string form, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Starting => "starting",
            Phase::Deploy => "deploy",
            Phase::Running => "running",
            Phase::Collect => "collect",
            Phase::Done => "done",
        }
    }
}

/// Terminal outcome of a persistent tester job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    #[serde(rename = "partial_success")]
    PartialSuccess,
    Failure,
    Cancelled,
}

impl Outcome {
    /// Returns the lowercase string form, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::PartialSuccess => "partial_success",
            Outcome::Failure => "failure",
            Outcome::Cancelled => "cancelled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&Phase::Deploy).unwrap(), "\"deploy\"");
    }

    #[test]
    fn phase_round_trips() {
        let phase: Phase = serde_json::from_str("\"running\"").unwrap();
        assert_eq!(phase, Phase::Running);
    }

    #[test]
    fn outcome_serializes_as_lowercase() {
        assert_eq!(
            serde_json::to_string(&Outcome::PartialSuccess).unwrap(),
            "\"partial_success\""
        );
    }

    #[test]
    fn phase_as_str_matches_serde() {
        // Exhaustive match forces updates here when new variants are added.
        let phases = [
            Phase::Queued,
            Phase::Starting,
            Phase::Deploy,
            Phase::Running,
            Phase::Collect,
            Phase::Done,
        ];
        for p in phases {
            // Exhaustiveness check: if a new variant is added, this match will fail
            // to compile until the array above is updated too.
            match p {
                Phase::Queued
                | Phase::Starting
                | Phase::Deploy
                | Phase::Running
                | Phase::Collect
                | Phase::Done => {}
            }
            assert_eq!(
                format!("\"{}\"", p.as_str()),
                serde_json::to_string(&p).unwrap()
            );
        }

        let outcomes = [
            Outcome::Success,
            Outcome::PartialSuccess,
            Outcome::Failure,
            Outcome::Cancelled,
        ];
        for o in outcomes {
            match o {
                Outcome::Success
                | Outcome::PartialSuccess
                | Outcome::Failure
                | Outcome::Cancelled => {}
            }
            assert_eq!(
                format!("\"{}\"", o.as_str()),
                serde_json::to_string(&o).unwrap()
            );
        }
    }
}
