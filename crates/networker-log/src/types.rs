use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Log severity level, stored as SMALLINT 1–5 in the database.
///
/// The integer mapping is intentionally stable — do **not** reorder.
/// Error=1 (highest severity) … Trace=5 (lowest severity).
/// DB values: Error=1 (highest severity) … Trace=5 (lowest severity).
/// Ord/PartialOrd reflects severity ascending: Trace < Debug < Info < Warn < Error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(i16)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    /// Severity rank used for ordering (Trace=0 … Error=4).
    fn severity_rank(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Debug => 1,
            Self::Info => 2,
            Self::Warn => 3,
            Self::Error => 4,
        }
    }
}

impl PartialOrd for Level {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Level {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.severity_rank().cmp(&other.severity_rank())
    }
}

impl Level {
    /// Convert the database SMALLINT representation back to a `Level`.
    pub fn from_db(value: i16) -> Option<Self> {
        match value {
            1 => Some(Self::Error),
            2 => Some(Self::Warn),
            3 => Some(Self::Info),
            4 => Some(Self::Debug),
            5 => Some(Self::Trace),
            _ => None,
        }
    }

    /// Return the stable SMALLINT value written to the database.
    pub fn as_db(self) -> i16 {
        self as i16
    }

    /// Convert from a [`tracing::Level`] reference.
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::TRACE => Self::Trace,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::INFO => Self::Info,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::ERROR => Self::Error,
        }
    }

    /// Convert to the corresponding [`tracing::Level`].
    pub fn to_tracing(self) -> tracing::Level {
        match self {
            Self::Trace => tracing::Level::TRACE,
            Self::Debug => tracing::Level::DEBUG,
            Self::Info => tracing::Level::INFO,
            Self::Warn => tracing::Level::WARN,
            Self::Error => tracing::Level::ERROR,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => f.write_str("TRACE"),
            Self::Debug => f.write_str("DEBUG"),
            Self::Info => f.write_str("INFO"),
            Self::Warn => f.write_str("WARN"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}

/// Case-insensitive, whitespace-tolerant `FromStr` for human-facing input.
impl FromStr for Level {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "ERROR" | "ERR" | "FATAL" | "1" => Ok(Self::Error),
            "WARN" | "WARNING" | "2" => Ok(Self::Warn),
            "INFO" | "INF" | "3" | "INFORMATION" => Ok(Self::Info),
            "DEBUG" | "DBG" | "4" => Ok(Self::Debug),
            "TRACE" | "TRC" | "5" => Ok(Self::Trace),
            other => Err(anyhow::anyhow!("unknown log level: {:?}", other)),
        }
    }
}

/// A single structured log entry destined for the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Wall-clock timestamp (UTC).
    pub ts: DateTime<Utc>,
    /// Originating service name, e.g. `"networker-dashboard"`.
    pub service: String,
    /// Severity level.
    pub level: Level,
    /// Human-readable message string.
    pub message: String,
    /// Optional dashboard config / probe-set identifier.
    pub config_id: Option<Uuid>,
    /// Optional project identifier (14-char base36 or UUID).
    pub project_id: Option<String>,
    /// Optional distributed trace identifier.
    pub trace_id: Option<Uuid>,
    /// Arbitrary structured fields serialised as a JSON object.
    pub fields: Option<serde_json::Value>,
}

impl LogEntry {
    /// Construct a new entry with `fields` defaulting to `None`.
    pub fn new(service: impl Into<String>, level: Level, message: impl Into<String>) -> Self {
        Self {
            ts: Utc::now(),
            service: service.into(),
            level,
            message: message.into(),
            config_id: None,
            project_id: None,
            trace_id: None,
            fields: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Level DB mapping ──────────────────────────────────────────────────────

    #[test]
    fn level_db_round_trip() {
        for (level, expected_db) in [
            (Level::Error, 1i16),
            (Level::Warn, 2),
            (Level::Info, 3),
            (Level::Debug, 4),
            (Level::Trace, 5),
        ] {
            assert_eq!(level.as_db(), expected_db, "as_db mismatch for {level}");
            assert_eq!(
                Level::from_db(expected_db),
                Some(level),
                "from_db mismatch for {expected_db}"
            );
        }
    }

    #[test]
    fn level_from_db_out_of_range_returns_none() {
        for bad in [0i16, 6, -1, 100] {
            assert!(Level::from_db(bad).is_none(), "expected None for {bad}");
        }
    }

    // ── Level ordering ────────────────────────────────────────────────────────

    #[test]
    fn level_ordering_is_ascending_severity() {
        assert!(Level::Trace < Level::Debug);
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }

    // ── from_str loose parsing ────────────────────────────────────────────────

    #[test]
    fn from_str_canonical_names() {
        assert_eq!("TRACE".parse::<Level>().unwrap(), Level::Trace);
        assert_eq!("DEBUG".parse::<Level>().unwrap(), Level::Debug);
        assert_eq!("INFO".parse::<Level>().unwrap(), Level::Info);
        assert_eq!("WARN".parse::<Level>().unwrap(), Level::Warn);
        assert_eq!("ERROR".parse::<Level>().unwrap(), Level::Error);
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!("trace".parse::<Level>().unwrap(), Level::Trace);
        assert_eq!("Debug".parse::<Level>().unwrap(), Level::Debug);
        assert_eq!("iNfO".parse::<Level>().unwrap(), Level::Info);
        assert_eq!("warn".parse::<Level>().unwrap(), Level::Warn);
        assert_eq!("error".parse::<Level>().unwrap(), Level::Error);
    }

    #[test]
    fn from_str_aliases() {
        assert_eq!("TRC".parse::<Level>().unwrap(), Level::Trace);
        assert_eq!("DBG".parse::<Level>().unwrap(), Level::Debug);
        assert_eq!("INFORMATION".parse::<Level>().unwrap(), Level::Info);
        assert_eq!("WARNING".parse::<Level>().unwrap(), Level::Warn);
        assert_eq!("ERR".parse::<Level>().unwrap(), Level::Error);
        assert_eq!("FATAL".parse::<Level>().unwrap(), Level::Error);
    }

    #[test]
    fn from_str_leading_trailing_whitespace() {
        assert_eq!("  info  ".parse::<Level>().unwrap(), Level::Info);
    }

    #[test]
    fn from_str_unknown_returns_err() {
        assert!("VERBOSE".parse::<Level>().is_err());
        assert!("".parse::<Level>().is_err());
        assert!("6".parse::<Level>().is_err());
    }

    // ── tracing::Level mapping ────────────────────────────────────────────────

    #[test]
    fn to_tracing_maps_all_variants() {
        assert_eq!(Level::Trace.to_tracing(), tracing::Level::TRACE);
        assert_eq!(Level::Debug.to_tracing(), tracing::Level::DEBUG);
        assert_eq!(Level::Info.to_tracing(), tracing::Level::INFO);
        assert_eq!(Level::Warn.to_tracing(), tracing::Level::WARN);
        assert_eq!(Level::Error.to_tracing(), tracing::Level::ERROR);
    }

    // ── LogEntry construction ─────────────────────────────────────────────────

    #[test]
    fn log_entry_new_defaults() {
        let entry = LogEntry::new("svc", Level::Info, "hello");
        assert_eq!(entry.service, "svc");
        assert_eq!(entry.level, Level::Info);
        assert_eq!(entry.message, "hello");
        assert!(entry.config_id.is_none());
        assert!(entry.project_id.is_none());
        assert!(entry.trace_id.is_none());
        assert!(entry.fields.is_none());
    }

    #[test]
    fn log_entry_round_trip_json() {
        let entry = LogEntry::new("dashboard", Level::Warn, "test message");
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.level, entry.level);
        assert_eq!(decoded.message, entry.message);
    }
}
