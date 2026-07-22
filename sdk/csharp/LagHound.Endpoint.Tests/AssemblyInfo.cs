using Xunit;

// Serialize the entire assembly. These tests exercise a SUT with process-global
// state that cannot be isolated per-test:
//   * KillSwitch is a static (reads LAGHOUND_DISABLED); KillSwitch_Makes_
//     Everything_Bare_404 flips it process-wide. Under xUnit's default
//     per-class parallelism, a concurrently-running ConformanceTests download
//     would see the kill switch and return a bare 404 (flake: "Expected OK,
//     Actual NotFound").
//   * Mount_Without_Token_Fails_Closed mutates the LAGHOUND_TOKEN env var.
//   * Large_Download_Does_Not_Balloon_Memory measures GC.GetTotalAllocatedBytes,
//     which is process-wide cumulative — concurrent tests' allocations would
//     inflate the delta and falsely trip the < 32 MiB assertion.
// [Collection] only serializes within one class; only assembly-wide
// serialization removes the cross-class races. Cost is negligible (~30 tests).
[assembly: CollectionBehavior(DisableTestParallelization = true)]
