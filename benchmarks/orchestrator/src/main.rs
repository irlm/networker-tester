mod collector;
mod config;
mod cost;
mod deployer;
mod progress;
mod provisioner;
mod reporter;
mod runner;
mod types;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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

        /// Azure VM size for provisioning.
        #[arg(long, default_value = "Standard_D2s_v3")]
        vm_size: String,

        /// Operating system: ubuntu or windows.
        #[arg(long, default_value = "ubuntu")]
        os: String,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            config: config_path,
            languages,
            dry_run,
            quick,
            html,
            vm_size,
            os,
        } => cmd_run(config_path, languages, dry_run, quick, html, vm_size, os).await,

        Command::List { config } => cmd_list(config),

        Command::Results {
            run_id,
            latest,
            format,
        } => cmd_results(run_id, latest, format),

        Command::Compare { runs } => cmd_compare(runs),
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    config_path: PathBuf,
    languages: Option<Vec<String>>,
    dry_run: bool,
    quick: bool,
    html: bool,
    vm_size: String,
    os: String,
) -> Result<()> {
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

    let matrix = cfg.test_matrix();
    let estimated_cost = cost::estimate_cost(&cfg);

    tracing::info!(
        "Benchmark: {} | {} languages | {} test cases | est. ${:.2}",
        cfg.name,
        cfg.languages.len(),
        matrix.len(),
        estimated_cost
    );

    if dry_run {
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
        println!(
            "\nTotal: {} cases | {} requests each | est. ${:.2}",
            matrix.len(),
            cfg.total_requests,
            estimated_cost
        );
        return Ok(());
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

    // -- Execute benchmark --
    let mut run = types::BenchmarkRun::new(&config_path.to_string_lossy());
    let total_steps = cfg.languages.len() * cfg.concurrency_levels.len();
    let progress = progress::ProgressReporter::new(total_steps as u32);

    // Group test cases by language to share a VM per language
    let unique_languages: Vec<config::LanguageEntry> = {
        let mut seen = std::collections::HashSet::new();
        cfg.languages
            .iter()
            .filter(|l| seen.insert(l.name.clone()))
            .cloned()
            .collect()
    };

    for lang in &unique_languages {
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
            None => provisioner::provision_vm("azure", &os, &vm_size, &vm_name)
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

        for &conc in &cfg.concurrency_levels {
            let step_label = format!("{label_prefix} c={conc}");

            match runner::run_cold_warm_cycle(&vm, &test_params, conc, &lang.name, &lang.runtime)
                .await
            {
                Ok((mut cold, mut warm)) => {
                    cold.binary = binary_metrics.clone();
                    warm.binary = binary_metrics.clone();

                    // Collect resource metrics during the warm phase
                    // (estimate duration from requests and expected RPS)
                    let estimated_duration = (cfg.total_requests as f64 / 1000.0).max(5.0) as u64;
                    if let Ok(samples) =
                        collector::collect_during_test(&vm, estimated_duration.min(60), None).await
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
    println!("Results:   {} data points", run.results.len());
    println!("Output:    {}", json_path.display());

    if !run.results.is_empty() {
        println!(
            "\n{:<16} {:<12} {:>10} {:>10} {:>10}",
            "Language", "Phase", "RPS", "p50 ms", "p99 ms"
        );
        println!("{}", "-".repeat(62));
        for r in &run.results {
            let phase = if r.network.total_requests <= 100 {
                "cold"
            } else {
                "warm"
            };
            println!(
                "{:<16} {:<12} {:>10.1} {:>10.2} {:>10.2}",
                r.language,
                phase,
                r.network.rps,
                r.network.latency_p50_ms,
                r.network.latency_p99_ms
            );
        }
    }

    Ok(())
}

fn cmd_list(config_path: PathBuf) -> Result<()> {
    let cfg = config::BenchmarkConfig::load(&config_path)?;

    println!("\nAvailable languages:\n");
    println!(
        "{:<12} {:<12} {:<6} Path",
        "Language", "Runtime", "Port"
    );
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

fn cmd_results(run_id: Option<String>, latest: bool, format: String) -> Result<()> {
    if !latest && run_id.is_none() {
        anyhow::bail!("Specify --run-id <UUID> or --latest");
    }
    // Stub -- results storage not yet implemented
    tracing::warn!(
        "results command is a stub (run_id={:?}, latest={}, format={})",
        run_id,
        latest,
        format
    );
    println!("Results storage not yet implemented. Run a benchmark to generate a JSON report.");
    Ok(())
}

fn cmd_compare(runs: Vec<String>) -> Result<()> {
    anyhow::ensure!(runs.len() >= 2, "Need at least 2 run IDs to compare");
    // Stub -- comparison not yet implemented
    tracing::warn!("compare command is a stub (runs={:?})", runs);
    println!("Comparison not yet implemented.");
    Ok(())
}
