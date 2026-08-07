using BenchmarkDotNet.Running;

// Standard BenchmarkDotNet switcher: `dotnet run -c Release -- --filter '*'`.
// CI passes `--job short` (see microbench-dotnet.yml); locally omit it for
// full statistical rigor.
BenchmarkSwitcher.FromAssembly(typeof(Networker.Benchmarks.ReportExportBenchmarks).Assembly)
    .Run(args);
