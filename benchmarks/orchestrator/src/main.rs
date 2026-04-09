#[allow(dead_code)]
mod callback;
mod collector;
mod config;
mod cost;
mod deployer;
mod executor;
mod progress;
mod provisioner;
mod reporter;
mod runner;
pub mod ssh;
mod token_manager;
mod types;
mod validator;
mod vm_tiers;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Parser)]
#[command(
    name = "alethabench",
    about = "AletheBench — cross-language network API benchmark orchestrator",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a benchmark suite from a config file.
    Run {
        /// Path to the benchmark config JSON file.
        #[arg(short, long)]
        config: PathBuf,

        /// Comma-separated list of languages to include (default: all).
        #[arg(short, long, value_delimiter = ',')]
        languages: Option<Vec<String>>,

        /// Print the test matrix and exit without running.
        #[arg(long)]
        dry_run: bool,

        /// Quick mode: 1000 requests, concurrency [1,10], repeat 1.
        #[arg(long)]
        quick: bool,

        /// Generate an HTML report alongside JSON.
        #[arg(long)]
        html: bool,

        /// Randomize benchmark case order before execution.
        #[arg(long)]
        randomize_cases: bool,

        /// Deterministic seed used when randomizing benchmark case order.
        #[arg(long)]
        random_seed: Option<u64>,

        /// Automatically schedule bounded reruns for cases that fail publication checks.
        #[arg(long)]
        auto_rerun_poor_quality: bool,

        /// Target repeat count for publication-oriented reruns.
        #[arg(long)]
        auto_rerun_target_repeat_count: Option<u32>,

        /// Maximum additional repeats per case when auto rerun is enabled.
        #[arg(long)]
        auto_rerun_max_additional_repeats: Option<u32>,

        /// Maximum allowed relative margin of error before a case is rerun.
        #[arg(long)]
        auto_rerun_max_relative_margin: Option<f64>,

        /// Azure VM size for provisioning.
        #[arg(long, default_value = "Standard_D2s_v3")]
        vm_size: String,

        /// Operating system: ubuntu or windows.
        #[arg(long, default_value = "ubuntu")]
        os: String,

        /// Dashboard callback URL for progress reporting.
        #[arg(long)]
        callback_url: Option<String>,

        /// Bearer token for dashboard callback authentication.
        /// Also reads from BENCH_CALLBACK_TOKEN env var.
        #[arg(long, env = "BENCH_CALLBACK_TOKEN")]
        callback_token: Option<String>,
    },

    /// List available languages defined in a config file.
    List {
        /// Path to the benchmark config JSON file.
        #[arg(short, long)]
        config: PathBuf,
    },

    /// Show results from a previous run.
    Results {
        /// UUID of the run to display.
        #[arg(long)]
        run_id: Option<String>,

        /// Show the most recent run.
        #[arg(long)]
        latest: bool,

        /// Output format: json, table, html.
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// Compare two or more benchmark runs.
    Compare {
        /// Comma-separated list of run UUIDs to compare.
        #[arg(long, value_delimiter = ',')]
        runs: Vec<String>,
    },

    /// Export a benchmark run as a shareable publication bundle.
    Export {
        /// UUID of the run to export.
        #[arg(long)]
        run_id: Option<String>,

        /// Show the most recent run.
        #[arg(long)]
        latest: bool,

        /// Output directory for the export bundle.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// Validate a deployed API against the AletheBench spec.
    Validate {
        /// IP address of the server to validate.
        #[arg(long)]
        ip: String,

        /// Language label for the report (e.g. "rust", "go").
        #[arg(long, default_value = "unknown")]
        language: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // For the Run subcommand, peek at the config file for logs_db_url before
    // initialising tracing so we can enable DB log shipping from the start.
    let logs_db_url = if let Command::Run { ref config, .. } = cli.command {
        std::fs::read_to_string(config)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("logs_db_url")?.as_str().map(String::from))
    } else {
        None
    };

    let mut builder = networker_log::LogBuilder::new("orchestrator")
        .with_console(networker_log::Stream::Stderr);
    if let Some(ref url) = logs_db_url {
        builder = builder.with_db(url);
    }
    let _log_guard = builder.init().await?;

    match cli.command {
        Command::Run {
            config: config_path,
            languages,
            dry_run,
            quick,
            html,
            randomize_cases,
            random_seed,
            auto_rerun_poor_quality,
            auto_rerun_target_repeat_count,
            auto_rerun_max_additional_repeats,
            auto_rerun_max_relative_margin,
            vm_size,
            os,
            callback_url,
            callback_token,
        } => {
            cmd_run(
                config_path,
                languages,
                dry_run,
                quick,
                html,
                randomize_cases,
                random_seed,
                auto_rerun_poor_quality,
                auto_rerun_target_repeat_count,
                auto_rerun_max_additional_repeats,
                auto_rerun_max_relative_margin,
                vm_size,
                os,
                callback_url,
                callback_token,
            )
            .await
        }

        Command::List { config } => cmd_list(config),

        Command::Results {
            run_id,
            latest,
            format,
        } => cmd_results(run_id, latest, format),

        Command::Compare { runs } => cmd_compare(runs),

        Command::Export {
            run_id,
            latest,
            output_dir,
        } => cmd_export(run_id, latest, output_dir),

        Command::Validate { ip, language } => cmd_validate(ip, language).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    config_path: PathBuf,
    languages: Option<Vec<String>>,
    dry_run: bool,
    quick: bool,
    html: bool,
    randomize_cases: bool,
    random_seed: Option<u64>,
    auto_rerun_poor_quality: bool,
    auto_rerun_target_repeat_count: Option<u32>,
    auto_rerun_max_additional_repeats: Option<u32>,
    auto_rerun_max_relative_margin: Option<f64>,
    vm_size: String,
    os: String,
    callback_url: Option<String>,
    callback_token: Option<String>,
) -> Result<()> {
    // If callback_url is set, use dashboard config format (has config_id, no 'name' field)
    if let (Some(ref url), Some(ref token)) = (&callback_url, &callback_token) {
        let dashboard_config = config::DashboardBenchmarkConfig::load(&config_path)
            .with_context(|| format!("loading config from {}", config_path.display()))?;

        tracing::info!(
            "Dashboard benchmark config detected (config_id={}), using executor",
            dashboard_config.config_id
        );

        let callback_client = std::sync::Arc::new(callback::CallbackClient::new(
            url,
            token,
            &dashboard_config.config_id,
        ));

        // PID file
        let pid_path = format!("/tmp/alethabench-{}.pid", dashboard_config.config_id);
        std::fs::write(&pid_path, std::process::id().to_string()).ok();

        // Cancellation channel
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

        // Spawn heartbeat with cancellation
        let hb_client = callback_client.clone();
        let hb_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let _ = hb_client.heartbeat().await;
                if hb_client.check_cancelled().await.unwrap_or(false) {
                    let _ = cancel_tx.send(true);
                    break;
                }
            }
        });

        // Use the benchmarks directory as bench_dir
        let bench_dir = config_path.parent().unwrap_or(std::path::Path::new("."));

        let result = crate::executor::execute_dashboard_benchmark(
            &dashboard_config,
            &callback_client,
            &cancel_rx,
            bench_dir,
        )
        .await;

        hb_handle.abort();

        // Cleanup
        std::fs::remove_file(&pid_path).ok();
        return result;
    }

    let mut cfg = config::BenchmarkConfig::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    if let Some(langs) = &languages {
        cfg.filter_languages(langs);
        anyhow::ensure!(
            !cfg.languages.is_empty(),
            "no languages matched the filter: {:?}",
            langs
        );
    }

    if quick {
        cfg.apply_quick();
        tracing::info!("Quick mode: 1000 requests, concurrency [1,10], repeat 1");
    }

    let case_randomization_seed = resolve_case_randomization_seed(randomize_cases, random_seed);
    let matrix = cfg.test_matrix_with_seed(case_randomization_seed);
    let auto_rerun_settings = resolve_auto_rerun_settings(
        auto_rerun_poor_quality,
        auto_rerun_target_repeat_count,
        auto_rerun_max_additional_repeats,
        auto_rerun_max_relative_margin,
    )?;
    let estimated_cost = cost::estimate_cost(&cfg);

    if let Some(seed) = case_randomization_seed {
        tracing::info!("Case randomization enabled with seed {}", seed);
    }

    tracing::info!(
        "Benchmark: {} | {} languages | {} test cases | est. ${:.2}",
        cfg.name,
        cfg.languages.len(),
        matrix.len(),
        estimated_cost
    );

    if dry_run {
        // -- Compute summary statistics --
        let num_languages = {
            let mut seen = std::collections::HashSet::new();
            cfg.languages
                .iter()
                .filter(|l| seen.insert(l.name.clone()))
                .count()
        };
        let num_vms = num_languages; // one VM per language
        let num_concurrency = cfg.concurrency_levels.len();

        // Estimated wall-clock time: each test case runs warmup + benchmark
        // requests. Assume ~5000 RPS baseline, so each case takes
        // (warmup + requests) / 5000 seconds, plus 30s overhead per VM for
        // deploy/validate.
        let requests_per_case = cfg.warmup_requests + cfg.total_requests;
        let secs_per_case = (requests_per_case as f64 / 5000.0).max(1.0);
        let total_case_secs = secs_per_case * matrix.len() as f64;
        let vm_overhead_secs = num_vms as f64 * 30.0; // deploy + validate per VM
        let total_wall_secs = total_case_secs + vm_overhead_secs;
        let wall_minutes = total_wall_secs / 60.0;

        // -- Test matrix table --
        println!("\n--- Test Matrix (dry run) ---\n");
        println!(
            "{:<4} {:<12} {:<12} {:<12} {:<8}",
            "#", "Language", "Runtime", "Concurrency", "Repeat"
        );
        println!("{}", "-".repeat(52));
        for (i, tc) in matrix.iter().enumerate() {
            println!(
                "{:<4} {:<12} {:<12} {:<12} {:<8}",
                i + 1,
                tc.language.name,
                tc.language.runtime,
                tc.concurrency,
                tc.repeat_index + 1
            );
        }

        // -- Per-language breakdown --
        println!("\n--- Per-Language Breakdown ---\n");
        println!(
            "{:<12} {:<12} {:<8} {:<12} {:<10}",
            "Language", "Runtime", "Cases", "Requests", "Est. Cost"
        );
        println!("{}", "-".repeat(56));
        let mut seen = std::collections::HashSet::new();
        for lang in &cfg.languages {
            if !seen.insert(lang.name.clone()) {
                continue;
            }
            let lang_cases = matrix
                .iter()
                .filter(|tc| tc.language.name == lang.name)
                .count();
            let lang_requests = lang_cases as u64 * cfg.total_requests;
            let lang_cost = lang_cases as f64 * 0.05; // same heuristic as cost module
            println!(
                "{:<12} {:<12} {:<8} {:<12} ${:<9.2}",
                lang.name, lang.runtime, lang_cases, lang_requests, lang_cost
            );
        }

        // -- Summary --
        println!("\n--- Summary ---\n");
        println!("  Languages:          {num_languages}");
        println!("  VMs needed:         {num_vms}");
        println!("  Concurrency levels: {num_concurrency}");
        println!("  Repeats:            {}", cfg.repeat);
        println!("  Total test cases:   {}", matrix.len());
        println!("  Requests per case:  {}", cfg.total_requests);
        println!("  Warmup per case:    {}", cfg.warmup_requests);
        println!(
            "  Case order:         {}",
            case_order_label(case_randomization_seed)
        );
        println!(
            "  Auto reruns:        {}",
            auto_rerun_label(auto_rerun_settings)
        );
        if let Some(baseline_language) = &cfg.baseline_language {
            let baseline_label = cfg
                .baseline_runtime
                .as_ref()
                .map(|runtime| format!("{baseline_language}/{runtime}"))
                .unwrap_or_else(|| baseline_language.clone());
            println!("  Baseline:           {baseline_label}");
        }
        println!("  Estimated cost:     ${estimated_cost:.2}");
        println!("  Estimated time:     {wall_minutes:.1} minutes");

        if quick {
            println!("\n  [quick mode active: 1000 requests, concurrency [1,10], repeat 1]");
        }

        return Ok(());
    }

    // -- Cancellation channel: SIGTERM or dashboard cancel both trigger this --
    let (cancel_tx, cancel_rx) = watch::channel(false);

    // SIGTERM handler — signals cancellation on graceful shutdown request.
    {
        let cancel_tx = cancel_tx.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
                sigterm.recv().await;
            }
            #[cfg(not(unix))]
            {
                // On non-Unix, fall back to ctrl-c only.
                let _ = tokio::signal::ctrl_c().await;
            }
            tracing::warn!("Received termination signal, requesting graceful shutdown");
            let _ = cancel_tx.send(true);
        });
    }

    // -- Resolve the benchmarks directory (parent of orchestrator/) --
    let bench_dir = config_path
        .canonicalize()
        .ok()
        .and_then(|p| {
            p.ancestors()
                .find(|a| a.join("shared").is_dir())
                .map(|a| a.to_path_buf())
        })
        .unwrap_or_else(|| {
            // Fallback: assume benchmarks/ is two levels up from the config
            config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf()
        });

    // -- Dashboard executor path --
    // When both --callback-url and --callback-token are provided, try to parse
    // the config as a DashboardBenchmarkConfig and run the cell-based executor.
    if let (Some(ref url), Some(ref token)) = (&callback_url, &callback_token) {
        let dashboard_config = config::DashboardBenchmarkConfig::load(&config_path)
            .with_context(|| format!("loading config from {}", config_path.display()))?;

        tracing::info!(
            "Dashboard benchmark config detected (config_id={}), using executor",
            dashboard_config.config_id
        );

        let callback_client = Arc::new(callback::CallbackClient::new(
            url,
            token,
            &dashboard_config.config_id,
        ));

            // PID file
            let pid_path = PathBuf::from(format!(
                "/tmp/alethabench-{}.pid",
                dashboard_config.config_id
            ));
            if let Err(e) = std::fs::write(&pid_path, std::process::id().to_string()) {
                tracing::warn!("Failed to write PID file: {e}");
            } else {
                tracing::info!("PID file written: {}", pid_path.display());
            }

            // Heartbeat background task
            let heartbeat_handle = {
                let client = callback_client.clone();
                let cancel_tx = cancel_tx.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    interval.tick().await; // skip immediate first tick
                    loop {
                        interval.tick().await;
                        if let Err(e) = client.heartbeat().await {
                            tracing::warn!("Heartbeat failed: {e}");
                        }
                        match client.check_cancelled().await {
                            Ok(true) => {
                                tracing::warn!("Dashboard requested cancellation");
                                let _ = cancel_tx.send(true);
                                break;
                            }
                            Ok(false) => {}
                            Err(e) => tracing::warn!("Cancellation check failed: {e}"),
                        }
                    }
                })
            };

            let result = executor::execute_dashboard_benchmark(
                &dashboard_config,
                &callback_client,
                &cancel_rx,
                &bench_dir,
            )
            .await;

        heartbeat_handle.abort();
        let _ = std::fs::remove_file(&pid_path);

        return result;
    }

    // -- Original orchestrator flow (callback + heartbeat + PID) --
    let callback_client = match (&callback_url, &callback_token) {
        (Some(url), Some(token)) => {
            let config_id = config_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            Some(Arc::new(callback::CallbackClient::new(
                url, token, &config_id,
            )))
        }
        _ => None,
    };

    // PID file — write if we have a callback (dashboard-launched run).
    let pid_file_path = callback_client.as_ref().map(|_| {
        let config_id = config_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let path = PathBuf::from(format!("/tmp/alethabench-{config_id}.pid"));
        if let Err(e) = std::fs::write(&path, std::process::id().to_string()) {
            tracing::warn!("Failed to write PID file {}: {e}", path.display());
        } else {
            tracing::info!("PID file written: {}", path.display());
        }
        path
    });

    // Heartbeat background task — runs every 60s, also checks cancellation.
    let heartbeat_handle = if let Some(client) = callback_client.clone() {
        let cancel_tx = cancel_tx.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await; // skip immediate first tick
            loop {
                interval.tick().await;
                if let Err(e) = client.heartbeat().await {
                    tracing::warn!("Heartbeat failed: {e}");
                }
                match client.check_cancelled().await {
                    Ok(true) => {
                        tracing::warn!("Dashboard requested cancellation");
                        let _ = cancel_tx.send(true);
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!("Cancellation check failed: {e}"),
                }
            }
        }))
    } else {
        None
    };

    // Helper closure to clean up PID file.
    let cleanup_pid = |path: &Option<PathBuf>| {
        if let Some(ref p) = path {
            if let Err(e) = std::fs::remove_file(p) {
                tracing::warn!("Failed to remove PID file {}: {e}", p.display());
            } else {
                tracing::debug!("PID file removed: {}", p.display());
            }
        }
    };

    // -- Execute benchmark --
    let mut run = types::BenchmarkRun::new(&config_path.to_string_lossy());
    run.case_randomization_enabled = case_randomization_seed.is_some();
    run.case_randomization_seed = case_randomization_seed;
    run.auto_rerun_policy = auto_rerun_settings.map(|settings| types::BenchmarkAutoRerunPolicy {
        target_repeat_count: settings.target_repeat_count,
        max_additional_repeats: settings.max_additional_repeats,
        max_relative_margin_of_error: settings.max_relative_margin_of_error,
    });
    run.baseline = cfg
        .baseline_language
        .as_ref()
        .map(|language| types::BenchmarkBaseline {
            language: language.clone(),
            runtime: cfg.baseline_runtime.clone(),
        });
    let total_steps = matrix.len()
        + auto_rerun_settings
            .map(|settings| max_auto_rerun_steps(&matrix, settings.max_additional_repeats))
            .unwrap_or(0);
    let progress = progress::ProgressReporter::new(total_steps as u32);

    // Group test cases by language to share a VM per language
    let unique_languages: Vec<config::LanguageEntry> = {
        let mut seen = std::collections::HashSet::new();
        matrix
            .iter()
            .filter(|test_case| {
                seen.insert(format!(
                    "{}|{}",
                    test_case.language.name, test_case.language.runtime
                ))
            })
            .cloned()
            .map(|test_case| test_case.language)
            .collect()
    };

    for lang in &unique_languages {
        // Check cancellation before starting each language.
        if *cancel_rx.borrow() {
            tracing::warn!("Cancellation requested, stopping benchmark");
            break;
        }

        let vm_name = format!("ab-{}", lang.name);
        let label_prefix = format!("{}/{}", lang.name, lang.runtime);

        // a. Find or provision VM
        tracing::info!("--- Language: {} ({}) ---", lang.name, lang.runtime);
        let vm = match provisioner::find_existing_vm(&vm_name).await? {
            Some(mut existing) => {
                tracing::info!("Reusing existing VM {}", existing.name);
                if existing.ip.is_empty() {
                    provisioner::start_vm(&existing).await?;
                    provisioner::refresh_ip(&mut existing).await?;
                }
                existing
            }
            None => provisioner::provision_vm("azure", "eastus", &os, &vm_size, &vm_name)
                .await
                .with_context(|| format!("provisioning VM for {}", lang.name))?,
        };

        // b. Deploy API + validate
        if let Err(e) = deployer::deploy_api(&vm, &lang.name, &bench_dir).await {
            tracing::error!("Deploy failed for {}: {:#}", label_prefix, e);
            progress.fail(&label_prefix, &e);
            continue;
        }

        if let Err(e) = deployer::validate_api(&vm).await {
            tracing::error!("Validation failed for {}: {:#}", label_prefix, e);
            progress.fail(&label_prefix, &e);
            continue;
        }

        // c. Measure binary size + idle memory
        let binary_metrics = collector::measure_binary_size(&vm, &lang.name)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Binary size measurement failed: {e}");
                types::BinaryMetrics::default()
            });

        // d. For each concurrency level, run cold/warm cycle + collect metrics
        let test_params = runner::TestParams {
            warmup_requests: cfg.warmup_requests,
            benchmark_requests: cfg.total_requests,
            timeout_secs: cfg.timeout_secs,
        };

        let language_cases: Vec<&config::TestCase> = matrix
            .iter()
            .filter(|test_case| {
                test_case.language.name == lang.name && test_case.language.runtime == lang.runtime
            })
            .collect();

        for test_case in language_cases {
            run_case_cycle(
                &mut run,
                &progress,
                &vm,
                &test_params,
                &binary_metrics,
                cfg.total_requests,
                &label_prefix,
                &lang.name,
                &lang.runtime,
                test_case.concurrency,
                test_case.repeat_index,
                false,
            )
            .await;
        }

        if let Some(settings) = auto_rerun_settings {
            loop {
                let rerun_targets = collect_auto_rerun_targets(
                    &run.results,
                    &run.scheduled_reruns,
                    &lang.name,
                    &lang.runtime,
                    cfg.repeat,
                    settings,
                );
                if rerun_targets.is_empty() {
                    break;
                }

                tracing::info!(
                    "Scheduling {} publication-quality rerun(s) for {}/{}",
                    rerun_targets.len(),
                    lang.name,
                    lang.runtime
                );

                for target in rerun_targets {
                    run.scheduled_reruns.push(types::BenchmarkScheduledRerun {
                        language: lang.name.clone(),
                        runtime: lang.runtime.clone(),
                        concurrency: target.concurrency,
                        repeat_index: target.repeat_index,
                        reasons: target.reasons.clone(),
                    });
                    run_case_cycle(
                        &mut run,
                        &progress,
                        &vm,
                        &test_params,
                        &binary_metrics,
                        cfg.total_requests,
                        &label_prefix,
                        &lang.name,
                        &lang.runtime,
                        target.concurrency,
                        target.repeat_index,
                        true,
                    )
                    .await;
                }
            }
        }

        // e. Stop API + stop VM
        if let Err(e) = deployer::stop_api(&vm).await {
            tracing::warn!("Failed to stop API on {}: {e}", vm.name);
        }
        if let Err(e) = provisioner::stop_vm(&vm).await {
            tracing::warn!("Failed to stop VM {}: {e}", vm.name);
        }
    }

    run.finish();
    progress.finish();

    // Stop heartbeat task.
    if let Some(handle) = heartbeat_handle {
        handle.abort();
    }

    // Remove PID file.
    cleanup_pid(&pid_file_path);

    // -- Write results --
    let output_dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let json_path = output_dir.join(format!("results-{}.json", run.id));
    reporter::generate_json(&run, &json_path)?;

    if html {
        let html_path = output_dir.join(format!("results-{}.html", run.id));
        reporter::generate_html(&run, &html_path)?;
    }

    // -- Print summary --
    println!("\n=== Benchmark Summary ===\n");
    println!("Run:       {}", run.id);
    println!(
        "Languages: {}",
        unique_languages
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(baseline) = &run.baseline {
        let baseline_label = baseline
            .runtime
            .as_ref()
            .map(|runtime| format!("{}/{}", baseline.language, runtime))
            .unwrap_or_else(|| baseline.language.clone());
        println!("Baseline:  {baseline_label}");
    }
    println!(
        "Case order: {}",
        case_order_label(run.case_randomization_seed)
    );
    println!("Auto reruns: {}", auto_rerun_label(auto_rerun_settings));
    println!("Executed reruns: {}", run.scheduled_reruns.len());
    println!("Results:   {} data points", run.results.len());
    println!("Output:    {}", json_path.display());

    if !run.results.is_empty() {
        println!(
            "\n{:<16} {:<8} {:<8} {:>10} {:>10} {:>10}",
            "Language", "C", "Scenario", "RPS", "p50 ms", "p99 ms"
        );
        println!("{}", "-".repeat(74));
        for r in &run.results {
            println!(
                "{:<16} {:<8} {:<8} {:>10.1} {:>10.2} {:>10.2}",
                r.language,
                r.concurrency,
                r.scenario,
                r.network.rps,
                r.network.latency_p50_ms,
                r.network.latency_p99_ms
            );
        }
    }

    Ok(())
}

fn resolve_case_randomization_seed(randomize_cases: bool, random_seed: Option<u64>) -> Option<u64> {
    if !randomize_cases && random_seed.is_none() {
        return None;
    }

    Some(random_seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15)
    }))
}

fn case_order_label(seed: Option<u64>) -> String {
    seed.map(|seed| format!("randomized (seed {seed})"))
        .unwrap_or_else(|| "config order".to_string())
}

#[derive(Clone, Copy)]
struct AutoRerunSettings {
    target_repeat_count: u32,
    max_additional_repeats: u32,
    max_relative_margin_of_error: f64,
}

#[derive(Clone)]
struct AutoRerunTarget {
    concurrency: u32,
    repeat_index: u32,
    reasons: Vec<String>,
}

fn resolve_auto_rerun_settings(
    auto_rerun_poor_quality: bool,
    auto_rerun_target_repeat_count: Option<u32>,
    auto_rerun_max_additional_repeats: Option<u32>,
    auto_rerun_max_relative_margin: Option<f64>,
) -> Result<Option<AutoRerunSettings>> {
    if !auto_rerun_poor_quality
        && auto_rerun_target_repeat_count.is_none()
        && auto_rerun_max_additional_repeats.is_none()
        && auto_rerun_max_relative_margin.is_none()
    {
        return Ok(None);
    }

    let target_repeat_count = auto_rerun_target_repeat_count.unwrap_or(3);
    anyhow::ensure!(
        target_repeat_count > 0,
        "auto rerun target repeat count must be > 0"
    );

    let max_additional_repeats = auto_rerun_max_additional_repeats.unwrap_or(2);
    anyhow::ensure!(
        max_additional_repeats > 0,
        "auto rerun max additional repeats must be > 0"
    );

    let max_relative_margin_of_error = auto_rerun_max_relative_margin.unwrap_or(0.05);
    anyhow::ensure!(
        max_relative_margin_of_error.is_finite() && max_relative_margin_of_error > 0.0,
        "auto rerun max relative margin must be a positive finite number"
    );

    Ok(Some(AutoRerunSettings {
        target_repeat_count,
        max_additional_repeats,
        max_relative_margin_of_error,
    }))
}

fn auto_rerun_label(settings: Option<AutoRerunSettings>) -> String {
    settings
        .map(|settings| {
            format!(
                "enabled (target repeats {}, max additional repeats {}, max relative margin {:.0}%)",
                settings.target_repeat_count,
                settings.max_additional_repeats,
                settings.max_relative_margin_of_error * 100.0
            )
        })
        .unwrap_or_else(|| "disabled".to_string())
}

fn max_auto_rerun_steps(matrix: &[config::TestCase], max_additional_repeats: u32) -> usize {
    let unique_cases = matrix
        .iter()
        .map(|test_case| {
            format!(
                "{}|{}|{}",
                test_case.language.name, test_case.language.runtime, test_case.concurrency
            )
        })
        .collect::<std::collections::HashSet<_>>()
        .len();
    unique_cases * max_additional_repeats as usize
}

fn collect_auto_rerun_targets(
    results: &[types::BenchmarkResult],
    scheduled_reruns: &[types::BenchmarkScheduledRerun],
    language: &str,
    runtime: &str,
    initial_repeat_count: u32,
    settings: AutoRerunSettings,
) -> Vec<AutoRerunTarget> {
    let language_results: Vec<types::BenchmarkResult> = results
        .iter()
        .filter(|result| result.language == language && result.runtime == runtime)
        .cloned()
        .collect();

    let summaries = reporter::summarise_results(&language_results);
    let mut targets = Vec::new();

    for summary in summaries {
        let mut reasons = Vec::new();
        for scenario in [summary.warm.as_ref(), summary.cold.as_ref()]
            .into_iter()
            .flatten()
        {
            if scenario.repeat_count < settings.target_repeat_count {
                reasons.push(format!(
                    "{} repeat count {} below target {}",
                    scenario.scenario, scenario.repeat_count, settings.target_repeat_count
                ));
            }
            if scenario.rps.quality_tier == "unreliable"
                || scenario.latency_p99_ms.quality_tier == "unreliable"
            {
                reasons.push(format!("{} variance remains unreliable", scenario.scenario));
            }
            if scenario.rps.relative_margin_of_error > settings.max_relative_margin_of_error
                || scenario.latency_p99_ms.relative_margin_of_error
                    > settings.max_relative_margin_of_error
            {
                reasons.push(format!(
                    "{} confidence interval exceeds {:.0}% target",
                    scenario.scenario,
                    settings.max_relative_margin_of_error * 100.0
                ));
            }
        }

        reasons.sort();
        reasons.dedup();
        if reasons.is_empty() {
            continue;
        }

        let next_repeat_index = summary
            .warm
            .as_ref()
            .into_iter()
            .flat_map(|scenario| scenario.repeat_indices.iter().copied())
            .chain(
                summary
                    .cold
                    .as_ref()
                    .into_iter()
                    .flat_map(|scenario| scenario.repeat_indices.iter().copied()),
            )
            .chain(
                scheduled_reruns
                    .iter()
                    .filter(|rerun| {
                        rerun.language == summary.language
                            && rerun.runtime == summary.runtime
                            && rerun.concurrency == summary.concurrency
                    })
                    .map(|rerun| rerun.repeat_index),
            )
            .max()
            .map(|index| index + 1)
            .unwrap_or(initial_repeat_count);

        let additional_repeats_used = next_repeat_index.saturating_sub(initial_repeat_count);
        if additional_repeats_used >= settings.max_additional_repeats {
            continue;
        }

        targets.push(AutoRerunTarget {
            concurrency: summary.concurrency,
            repeat_index: next_repeat_index,
            reasons,
        });
    }

    targets.sort_by_key(|target| (target.concurrency, target.repeat_index));
    targets
}

async fn run_case_cycle(
    run: &mut types::BenchmarkRun,
    progress: &progress::ProgressReporter,
    vm: &provisioner::VmInfo,
    test_params: &runner::TestParams,
    binary_metrics: &types::BinaryMetrics,
    total_requests: u64,
    label_prefix: &str,
    language: &str,
    runtime: &str,
    concurrency: u32,
    repeat_index: u32,
    is_auto_rerun: bool,
) {
    let suffix = if is_auto_rerun { " [auto-rerun]" } else { "" };
    let step_label = format!(
        "{label_prefix} c={concurrency} repeat={}{}",
        repeat_index + 1,
        suffix
    );

    match runner::run_cold_warm_cycle(
        vm,
        test_params,
        concurrency,
        language,
        runtime,
        repeat_index,
    )
    .await
    {
        Ok((mut cold, mut warm)) => {
            cold.binary = binary_metrics.clone();
            warm.binary = binary_metrics.clone();

            let estimated_duration = (total_requests as f64 / 1000.0).max(5.0) as u64;
            if let Ok(samples) =
                collector::collect_during_test(vm, estimated_duration.min(60), None).await
            {
                let agg = collector::aggregate_metrics(&samples);
                warm.resources = agg;
            }

            run.results.push(cold);
            run.results.push(warm);
            progress.tick(&step_label);
        }
        Err(e) => {
            tracing::error!("Benchmark failed for {step_label}: {e:#}");
            progress.fail(&step_label, &e);
        }
    }
}

async fn cmd_validate(ip: String, language: String) -> Result<()> {
    tracing::info!("Validating API at {ip} (language={language})");

    let result = validator::validate_api(&ip, &language).await?;
    result.print_summary();

    if result.all_ok() {
        println!("\nResult: ALL CHECKS PASSED");
        Ok(())
    } else {
        let fail_count = result.errors.len();
        anyhow::bail!(
            "Validation failed: {fail_count} check(s) did not pass for {language} at {ip}"
        );
    }
}

fn cmd_list(config_path: PathBuf) -> Result<()> {
    let cfg = config::BenchmarkConfig::load(&config_path)?;

    println!("\nAvailable languages:\n");
    println!("{:<12} {:<12} {:<6} Path", "Language", "Runtime", "Port");
    println!("{}", "-".repeat(60));
    for lang in &cfg.languages {
        println!(
            "{:<12} {:<12} {:<6} {}",
            lang.name, lang.runtime, lang.port, lang.path
        );
    }
    println!("\nTotal: {} languages", cfg.languages.len());
    Ok(())
}

fn report_baseline_label(report: &types::BenchmarkReport) -> Option<String> {
    report.run.baseline.as_ref().map(|baseline| {
        baseline
            .runtime
            .as_ref()
            .map(|runtime| format!("{}/{}", baseline.language, runtime))
            .unwrap_or_else(|| baseline.language.clone())
    })
}

fn case_key(summary: &types::BenchmarkCaseSummary) -> String {
    format!(
        "{}|{}|{}",
        summary.language.to_lowercase(),
        summary.runtime.to_lowercase(),
        summary.concurrency
    )
}

fn result_case_key(result: &types::BenchmarkResult) -> String {
    format!(
        "{}|{}|{}",
        result.language.to_lowercase(),
        result.runtime.to_lowercase(),
        result.concurrency
    )
}

fn render_results_table(report: &types::BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Run:       {}\n", report.run.id));
    out.push_str(&format!("Config:    {}\n", report.run.config_path));
    if let Some(baseline) = report_baseline_label(report) {
        out.push_str(&format!("Baseline:  {baseline}\n"));
    }
    out.push_str(&format!(
        "Cases:     {}\n",
        report.aggregation.case_summaries.len()
    ));
    out.push_str(&format!(
        "Generated: {}\n",
        report.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!(
        "Case order: {}\n",
        case_order_label(report.run.case_randomization_seed)
    ));
    out.push_str(&format!(
        "Auto reruns: {}\n",
        auto_rerun_label(
            report
                .run
                .auto_rerun_policy
                .as_ref()
                .map(|policy| AutoRerunSettings {
                    target_repeat_count: policy.target_repeat_count,
                    max_additional_repeats: policy.max_additional_repeats,
                    max_relative_margin_of_error: policy.max_relative_margin_of_error,
                })
        )
    ));
    out.push_str(&format!(
        "Executed reruns: {}\n",
        report.run.scheduled_reruns.len()
    ));
    out.push_str(&format!(
        "Publication: {}\n",
        if report.aggregation.publication_ready {
            "ready"
        } else {
            "rerun recommended"
        }
    ));
    if !report.aggregation.recommendations.is_empty() {
        out.push_str("Recommendations:\n");
        for recommendation in &report.aggregation.recommendations {
            out.push_str(&format!("  - {recommendation}\n"));
        }
    }

    out.push_str("\nCase summaries:\n\n");
    out.push_str(&format!(
        "{:<18} {:<8} {:<8} {:>12} {:>10} {:>10}\n",
        "Case", "C", "Repeats", "Warm med rps", "Warm CV%", "Warm p99"
    ));
    out.push_str(&format!("{}\n", "-".repeat(76)));
    for summary in &report.aggregation.case_summaries {
        let warm = summary.warm.as_ref();
        let repeats = warm
            .map(|scenario| scenario.repeat_count)
            .or_else(|| summary.cold.as_ref().map(|scenario| scenario.repeat_count))
            .unwrap_or(0);
        let warm_rps = warm.map(|scenario| scenario.rps.median).unwrap_or(0.0);
        let warm_cv = warm.map(|scenario| scenario.rps.cv * 100.0).unwrap_or(0.0);
        let warm_p99 = warm
            .map(|scenario| scenario.latency_p99_ms.median)
            .unwrap_or(0.0);
        out.push_str(&format!(
            "{:<18} {:<8} {:<8} {:>12.0} {:>10.1} {:>10.2}\n",
            summary.language, summary.concurrency, repeats, warm_rps, warm_cv, warm_p99
        ));
    }

    if !report.aggregation.comparisons.is_empty() {
        out.push_str("\nBaseline comparisons:\n\n");
        out.push_str(&format!(
            "{:<18} {:<8} {:>10} {:>12} {:<18}\n",
            "Case", "Scenario", "Ratio", "Delta %", "Verdict"
        ));
        out.push_str(&format!("{}\n", "-".repeat(74)));
        for comparison in &report.aggregation.comparisons {
            if !comparison.comparable {
                out.push_str(&format!(
                    "{:<18} {:<8} {:>10} {:>12} {:<18}\n",
                    comparison.language, "gated", "-", "-", "not comparable"
                ));
                out.push_str(&format!(
                    "  note: {}\n",
                    comparison.comparability_notes.join("; ")
                ));
                continue;
            }
            for scenario in [&comparison.warm, &comparison.cold].into_iter().flatten() {
                out.push_str(&format!(
                    "{:<18} {:<8} {:>10.2} {:>12.1} {:<18}\n",
                    comparison.language,
                    scenario.scenario,
                    scenario.throughput.ratio,
                    scenario.throughput.percent_delta,
                    scenario.throughput.verdict
                ));
            }
        }
    }

    out
}

fn render_compare_table(
    baseline_report: &types::BenchmarkReport,
    candidate_reports: &[types::BenchmarkReport],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Baseline run: {}\n", baseline_report.run.id));
    if let Some(baseline) = report_baseline_label(baseline_report) {
        out.push_str(&format!("Configured baseline: {baseline}\n"));
    }

    let baseline_cases: BTreeMap<String, &types::BenchmarkCaseSummary> = baseline_report
        .aggregation
        .case_summaries
        .iter()
        .map(|summary| (case_key(summary), summary))
        .collect();
    let baseline_results: BTreeMap<String, Vec<&types::BenchmarkResult>> = baseline_report
        .run
        .results
        .iter()
        .fold(BTreeMap::new(), |mut grouped, result| {
            grouped
                .entry(result_case_key(result))
                .or_insert_with(Vec::new)
                .push(result);
            grouped
        });

    for candidate in candidate_reports {
        let candidate_results: BTreeMap<String, Vec<&types::BenchmarkResult>> = candidate
            .run
            .results
            .iter()
            .fold(BTreeMap::new(), |mut grouped, result| {
                grouped
                    .entry(result_case_key(result))
                    .or_insert_with(Vec::new)
                    .push(result);
                grouped
            });
        out.push_str(&format!("\nAgainst run: {}\n\n", candidate.run.id));
        out.push_str(
            "Cross-run comparisons are gated when benchmark environment fingerprints differ materially.\n\n",
        );
        out.push_str(&format!(
            "{:<18} {:<8} {:>12} {:>12} {:>10}\n",
            "Case", "C", "Warm ratio", "p99 delta", "Warm CV%"
        ));
        out.push_str(&format!("{}\n", "-".repeat(70)));
        for summary in &candidate.aggregation.case_summaries {
            let case_key = case_key(summary);
            let Some(baseline_summary) = baseline_cases.get(&case_key) else {
                continue;
            };
            let Some(candidate_warm) = summary.warm.as_ref() else {
                continue;
            };
            let Some(baseline_warm) = baseline_summary.warm.as_ref() else {
                continue;
            };
            let comparability_notes = candidate_results
                .get(&case_key)
                .and_then(|candidate_results| {
                    baseline_results
                        .get(&case_key)
                        .and_then(|baseline_results| {
                            candidate_results.first().zip(baseline_results.first()).map(
                                |(candidate_result, baseline_result)| {
                                    reporter::environment_comparability_notes(
                                        &candidate_result.environment,
                                        &baseline_result.environment,
                                    )
                                },
                            )
                        })
                })
                .unwrap_or_else(|| {
                    vec!["candidate or baseline benchmark result is missing".to_string()]
                });
            if !comparability_notes.is_empty() {
                out.push_str(&format!(
                    "{:<18} {:<8} {:>12} {:>12} {:>10}\n",
                    summary.language, summary.concurrency, "-", "-", "-"
                ));
                out.push_str(&format!(
                    "  note: not comparable: {}\n",
                    comparability_notes.join("; ")
                ));
                continue;
            }

            let warm_ratio = if baseline_warm.rps.median.abs() > f64::EPSILON {
                candidate_warm.rps.median / baseline_warm.rps.median
            } else {
                0.0
            };
            let p99_delta =
                candidate_warm.latency_p99_ms.median - baseline_warm.latency_p99_ms.median;

            out.push_str(&format!(
                "{:<18} {:<8} {:>12.2} {:>12.2} {:>10.1}\n",
                summary.language,
                summary.concurrency,
                warm_ratio,
                p99_delta,
                candidate_warm.rps.cv * 100.0
            ));
        }
    }

    out
}

fn discover_report_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if matches!(dir_name, ".git" | "target" | "node_modules" | "obj") {
                continue;
            }
            discover_report_files(&path, files)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("results-") && name.ends_with(".json"))
            .unwrap_or(false)
        {
            files.push(path);
        }
    }

    Ok(())
}

fn resolve_report_path(identifier: &str) -> Result<PathBuf> {
    let candidate = PathBuf::from(identifier);
    if candidate.is_file() {
        return Ok(candidate);
    }

    let expected_name = if identifier.ends_with(".json") {
        identifier.to_string()
    } else {
        format!("results-{identifier}.json")
    };

    let mut files = Vec::new();
    discover_report_files(Path::new("."), &mut files)?;
    files
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name == expected_name)
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow::anyhow!("could not find report '{}'", identifier))
}

fn latest_report_path() -> Result<PathBuf> {
    let mut files = Vec::new();
    discover_report_files(Path::new("."), &mut files)?;
    let current_dir = std::env::current_dir()?;
    files
        .into_iter()
        .max_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no benchmark reports found under {}", current_dir.display())
        })
}

fn cmd_results(run_id: Option<String>, latest: bool, format: String) -> Result<()> {
    if !latest && run_id.is_none() {
        anyhow::bail!("Specify --run-id <UUID> or --latest");
    }

    let report_path = if latest {
        latest_report_path()?
    } else {
        resolve_report_path(run_id.as_deref().unwrap())?
    };
    let report = reporter::load_report(&report_path)?;
    match format.to_ascii_lowercase().as_str() {
        "json" => println!("{}", serde_json::to_string_pretty(&report)?),
        "html" => {
            let html_path = if report_path.extension().and_then(|ext| ext.to_str()) == Some("json")
            {
                let sibling = report_path.with_extension("html");
                if sibling.exists() {
                    sibling
                } else {
                    let temp_path = std::env::temp_dir()
                        .join(format!("alethabench-results-{}.html", report.run.id));
                    reporter::generate_html(&report.run, &temp_path)?;
                    temp_path
                }
            } else {
                report_path.clone()
            };
            println!("{}", html_path.display());
        }
        "table" => print!("{}", render_results_table(&report)),
        other => anyhow::bail!("unsupported results format '{}'", other),
    }
    Ok(())
}

fn cmd_compare(runs: Vec<String>) -> Result<()> {
    anyhow::ensure!(runs.len() >= 2, "Need at least 2 run IDs to compare");

    let mut reports = Vec::new();
    for identifier in runs {
        let path = resolve_report_path(&identifier)?;
        reports.push(reporter::load_report(&path)?);
    }

    let baseline_report = reports.remove(0);
    print!("{}", render_compare_table(&baseline_report, &reports));
    Ok(())
}

fn cmd_export(run_id: Option<String>, latest: bool, output_dir: Option<PathBuf>) -> Result<()> {
    anyhow::ensure!(
        latest || run_id.is_some(),
        "Specify --run-id <UUID> or --latest"
    );

    let report_path = if latest {
        latest_report_path()?
    } else {
        resolve_report_path(run_id.as_deref().unwrap())?
    };
    let report = reporter::load_report(&report_path)?;
    let bundle_dir = output_dir.unwrap_or_else(|| {
        report_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("benchmark-export-{}", report.run.id))
    });
    reporter::export_bundle(&report, &bundle_dir)?;
    println!("{}", bundle_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_environment() -> types::BenchmarkEnvironmentFingerprint {
        types::BenchmarkEnvironmentFingerprint {
            client_os: Some("macos".into()),
            client_arch: Some("aarch64".into()),
            client_cpu_cores: Some(12),
            client_region: Some("us-east".into()),
            server_os: Some("ubuntu".into()),
            server_arch: Some("x86_64".into()),
            server_cpu_cores: Some(4),
            server_region: Some("eastus".into()),
            network_type: Some("LAN".into()),
            baseline_rtt_p50_ms: Some(0.9),
            baseline_rtt_p95_ms: Some(1.4),
        }
    }

    fn sample_run(warm_rps: f64) -> types::BenchmarkRun {
        types::BenchmarkRun {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            config_path: "benchmarks/config.json".into(),
            case_randomization_enabled: true,
            case_randomization_seed: Some(42),
            auto_rerun_policy: Some(types::BenchmarkAutoRerunPolicy {
                target_repeat_count: 3,
                max_additional_repeats: 2,
                max_relative_margin_of_error: 0.05,
            }),
            scheduled_reruns: vec![types::BenchmarkScheduledRerun {
                language: "go".into(),
                runtime: "gin".into(),
                concurrency: 10,
                repeat_index: 1,
                reasons: vec!["warm repeat count 1 below target 3".into()],
            }],
            baseline: Some(types::BenchmarkBaseline {
                language: "rust".into(),
                runtime: Some("axum".into()),
            }),
            results: vec![
                types::BenchmarkResult {
                    language: "rust".into(),
                    runtime: "axum".into(),
                    concurrency: 10,
                    repeat_index: 0,
                    scenario: "warm".into(),
                    environment: sample_environment(),
                    network: types::NetworkMetrics {
                        rps: 100_000.0,
                        latency_mean_ms: 1.0,
                        latency_p50_ms: 0.8,
                        latency_p99_ms: 3.0,
                        latency_p999_ms: 5.0,
                        latency_max_ms: 8.0,
                        bytes_transferred: 1_000_000,
                        error_count: 0,
                        total_requests: 10_000,
                        phase_model: "stability-check->pilot->measured".into(),
                        phases_present: vec![
                            "stability-check".into(),
                            "pilot".into(),
                            "measured".into(),
                        ],
                    },
                    resources: types::ResourceMetrics::default(),
                    startup: types::StartupMetrics::default(),
                    binary: types::BinaryMetrics::default(),
                },
                types::BenchmarkResult {
                    language: "go".into(),
                    runtime: "gin".into(),
                    concurrency: 10,
                    repeat_index: 0,
                    scenario: "warm".into(),
                    environment: sample_environment(),
                    network: types::NetworkMetrics {
                        rps: warm_rps,
                        latency_mean_ms: 1.2,
                        latency_p50_ms: 0.9,
                        latency_p99_ms: 3.5,
                        latency_p999_ms: 5.5,
                        latency_max_ms: 9.0,
                        bytes_transferred: 1_000_000,
                        error_count: 0,
                        total_requests: 10_000,
                        phase_model: "stability-check->pilot->measured".into(),
                        phases_present: vec![
                            "stability-check".into(),
                            "pilot".into(),
                            "measured".into(),
                        ],
                    },
                    resources: types::ResourceMetrics::default(),
                    startup: types::StartupMetrics::default(),
                    binary: types::BinaryMetrics::default(),
                },
            ],
        }
    }

    #[test]
    fn test_render_results_table_includes_baseline_and_case_summaries() {
        let report = reporter::report_from_run(&sample_run(90_000.0));
        let rendered = render_results_table(&report);

        assert!(rendered.contains("Baseline:  rust/axum"));
        assert!(rendered.contains("Case order: randomized (seed 42)"));
        assert!(rendered.contains("Auto reruns: enabled"));
        assert!(rendered.contains("Executed reruns: 1"));
        assert!(rendered.contains("Publication: rerun recommended"));
        assert!(rendered.contains("Recommendations:"));
        assert!(rendered.contains("Case summaries"));
        assert!(rendered.contains("Baseline comparisons"));
        assert!(rendered.contains("go"));
    }

    #[test]
    fn test_render_results_table_shows_gated_comparison_notes() {
        let mut run = sample_run(90_000.0);
        for result in run
            .results
            .iter_mut()
            .filter(|result| result.language == "go")
        {
            result.environment.server_region = Some("westus".into());
            result.environment.baseline_rtt_p50_ms = Some(2.2);
        }

        let report = reporter::report_from_run(&run);
        let rendered = render_results_table(&report);

        assert!(rendered.contains("not comparable"));
        assert!(rendered.contains("server region differs"));
    }

    #[test]
    fn test_render_compare_table_uses_first_report_as_baseline() {
        let baseline_report = reporter::report_from_run(&sample_run(90_000.0));
        let candidate_report = reporter::report_from_run(&sample_run(80_000.0));

        let rendered = render_compare_table(&baseline_report, &[candidate_report]);

        assert!(rendered.contains("Baseline run:"));
        assert!(rendered.contains("Against run:"));
        assert!(rendered.contains("Cross-run comparisons are gated"));
        assert!(rendered.contains("go"));
        assert!(rendered.contains("0.89"));
    }

    #[test]
    fn test_render_compare_table_gates_mismatched_environments() {
        let baseline_report = reporter::report_from_run(&sample_run(90_000.0));
        let mut candidate_run = sample_run(80_000.0);
        for result in candidate_run
            .results
            .iter_mut()
            .filter(|result| result.language == "go")
        {
            result.environment.server_region = Some("westus".into());
            result.environment.baseline_rtt_p50_ms = Some(2.0);
        }
        let candidate_report = reporter::report_from_run(&candidate_run);

        let rendered = render_compare_table(&baseline_report, &[candidate_report]);

        assert!(rendered.contains("not comparable"));
        assert!(rendered.contains("server region differs"));
    }

    #[test]
    fn test_resolve_case_randomization_seed_respects_flags() {
        assert_eq!(resolve_case_randomization_seed(false, None), None);
        assert_eq!(resolve_case_randomization_seed(false, Some(7)), Some(7));
        assert_eq!(resolve_case_randomization_seed(true, Some(99)), Some(99));
    }

    #[test]
    fn test_collect_auto_rerun_targets_flags_low_repeat_cases() {
        let targets = collect_auto_rerun_targets(
            &sample_run(90_000.0).results,
            &sample_run(90_000.0).scheduled_reruns,
            "go",
            "gin",
            1,
            AutoRerunSettings {
                target_repeat_count: 3,
                max_additional_repeats: 2,
                max_relative_margin_of_error: 0.05,
            },
        );

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].concurrency, 10);
        assert_eq!(targets[0].repeat_index, 2);
        assert!(targets[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("below target 3")));
    }

    #[test]
    fn test_discover_report_files_finds_nested_json_reports() {
        let root = std::env::temp_dir().join(format!("alethabench-discovery-{}", Uuid::new_v4()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("results-one.json"), "{}").unwrap();
        std::fs::write(nested.join("results-two.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target").join("results-skip.json"), "{}").unwrap();

        let mut files = Vec::new();
        discover_report_files(&root, &mut files).unwrap();
        files.sort();

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("results-one.json")));
        assert!(files.iter().any(|path| path.ends_with("results-two.json")));

        std::fs::remove_dir_all(&root).ok();
    }
}
