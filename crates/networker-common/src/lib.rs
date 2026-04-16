pub mod messages;
/// WebSocket protocol version (v0.28.0 — bumped to 2 with the TestConfig refactor).
pub const PROTOCOL_VERSION: u32 = 2;
pub mod phase;
pub mod protocol;
pub mod test_config;
pub mod tester_messages;

// Re-export core types from networker-tester for convenience.
pub use networker_tester::metrics::{
    DnsResult, ErrorCategory, ErrorRecord, HostInfo, HttpResult, NetworkBaseline, NetworkType,
    PageLoadResult, Protocol, RequestAttempt, ServerTimingResult, TcpResult, TlsResult, UdpResult,
    UdpThroughputResult,
};

// Canonical unified types (v0.28.0 — see .critique/refactor/03-spec.md).
// NOTE: `TestRun` here shadows the old `networker_tester::metrics::TestRun`.
// Downstream code that still wants the legacy result-storage type should use
// `networker_tester::metrics::TestRun` directly.
pub use test_config::{
    CaptureMode, EndpointRef, Methodology, Mode, OutlierPolicy, PublicationGates, QualityGates,
    RunStatus, TestConfig, TestRun, TestSchedule, Workload,
};
