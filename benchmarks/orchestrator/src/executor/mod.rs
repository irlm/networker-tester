//! Dashboard-triggered benchmark execution, split by phase: status/DB writes
//! (`status`), VM lifecycle and Azure helpers (`vm`), SSH deploy/exec helpers
//! (`ssh_exec`), the testbed cycle (`cycle`), and the per-language benchmark
//! runs (`benchmark`).

mod benchmark;
mod cycle;
mod ssh_exec;
mod status;
mod vm;

pub use self::cycle::execute_dashboard_benchmark;
