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
        } => cmd_run(config_path, languages, dry_run, quick).await,

        Command::List { config } => cmd_list(config),

        Command::Results {
            run_id,
            latest,
            format,
        } => cmd_results(run_id, latest, format),

        Command::Compare { runs } => cmd_compare(runs),
    }
}

async fn cmd_run(
    config_path: PathBuf,
    languages: Option<Vec<String>>,
    dry_run: bool,
    quick: bool,
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

    // -- Execute benchmark --
    let mut run = types::BenchmarkRun::new(&config_path.to_string_lossy());
    let progress = progress::ProgressReporter::new(matrix.len() as u32);

    for tc in &matrix {
        let label = format!(
            "{}/{} c={}",
            tc.language.name, tc.language.runtime, tc.concurrency
        );
        let target_url = format!("http://127.0.0.1:{}{}", tc.language.port, cfg.endpoint);

        match runner::run_benchmark(&target_url, tc, cfg.total_requests, cfg.warmup_requests).await
        {
            Ok(network) => {
                let result = types::BenchmarkResult {
                    language: tc.language.name.clone(),
                    runtime: tc.language.runtime.clone(),
                    concurrency: tc.concurrency,
                    repeat_index: tc.repeat_index,
                    network,
                    resources: types::ResourceMetrics::default(),
                    startup: types::StartupMetrics::default(),
                    binary: types::BinaryMetrics::default(),
                };
                run.results.push(result);
                progress.tick(&label);
            }
            Err(e) => {
                progress.fail(&label, &e);
            }
        }
    }

    run.finish();
    progress.finish();

    // Write results
    let output_dir = config_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let json_path = output_dir.join(format!("results-{}.json", run.id));
    reporter::generate_json(&run, &json_path)?;

    Ok(())
}

fn cmd_list(config_path: PathBuf) -> Result<()> {
    let cfg = config::BenchmarkConfig::load(&config_path)?;

    println!("\nAvailable languages:\n");
    println!("{:<12} {:<12} {:<6} {}", "Language", "Runtime", "Port", "Path");
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
    // Stub — results storage not yet implemented
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
    // Stub — comparison not yet implemented
    tracing::warn!("compare command is a stub (runs={:?})", runs);
    println!("Comparison not yet implemented.");
    Ok(())
}
