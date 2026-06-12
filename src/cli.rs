use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::analysis::{SpanStats, Summary, summarize};
use crate::plog_io::{DEFAULT_MAX_INPUT_BYTES, compress_plog_file, read_plog_trace};
use crate::tui::{self, ColorMode, Theme};

#[derive(Debug, Parser)]
#[command(version, about = "Visualize configurable CPU and RTL pipeline logs")]
pub struct Args {
    /// Open a PLog file in the terminal timeline view.
    path: Option<PathBuf>,

    /// Disable stage colors in the terminal timeline view.
    #[arg(long)]
    no_color: bool,

    /// Load a JSON stage color theme for the terminal timeline view.
    #[arg(long, value_name = "PATH")]
    theme: Option<PathBuf>,

    /// Maximum uncompressed PLog input size to read.
    #[arg(long, default_value_t = default_max_input_mib(), value_name = "MIB")]
    max_input_mib: u64,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a PLog file.
    Validate { path: PathBuf },

    /// Print a text summary for a PLog file.
    Report { path: PathBuf },

    /// Compress a PLog file as .zst and remove the original file.
    Compress { path: PathBuf },
}

pub fn run() -> Result<()> {
    let args = Args::parse();
    let max_input_bytes = args
        .max_input_mib
        .checked_mul(1024 * 1024)
        .context("--max-input-mib is too large")?;

    match (args.command, args.path) {
        (Some(Command::Validate { path }), None) => {
            load_trace(&path, max_input_bytes)?;
            println!("valid: {}", path.display());
        }
        (Some(Command::Report { path }), None) => {
            let trace = load_trace(&path, max_input_bytes)?;
            print_report(&path, &summarize(&trace));
        }
        (Some(Command::Compress { path }), None) => {
            let output_path = compress_plog_file(&path)?;
            println!("compressed: {}", output_path.display());
        }
        (None, Some(path)) => {
            let theme = load_theme(args.theme.as_deref(), args.no_color)?;
            tui::run_path(&path, theme, max_input_bytes)?;
        }
        (None, None) => {
            Args::parse_from(["pipeview", "--help"]);
        }
        (Some(_), Some(path)) => {
            anyhow::bail!(
                "unexpected positional path `{}` before subcommand",
                path.display()
            );
        }
    }

    Ok(())
}

fn load_trace(path: &Path, max_input_bytes: u64) -> Result<crate::model::Trace> {
    read_plog_trace(path, max_input_bytes)
}

fn default_max_input_mib() -> u64 {
    DEFAULT_MAX_INPUT_BYTES / (1024 * 1024)
}

fn load_theme(path: Option<&Path>, no_color: bool) -> Result<Theme> {
    if no_color {
        return Ok(Theme::new(ColorMode::None));
    }

    let Some(path) = path else {
        return Ok(Theme::new(ColorMode::Default));
    };

    let input = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Theme::from_json_str(&input).with_context(|| format!("failed to parse {}", path.display()))
}

fn print_report(path: &Path, summary: &Summary) {
    println!("file: {}", path.display());
    println!("instructions: {}", summary.instruction_count);
    println!("retired: {}", summary.retired_count);
    println!("spans: {}", summary.span_count);
    println!("stages: {}", summary.stage_count);
    println!("lanes: {}", summary.lane_count);

    match (summary.cycle_start, summary.cycle_end) {
        (Some(start), Some(end)) => {
            println!("cycle_start: {start}");
            println!("cycle_end: {end}");
            println!("cycle_count: {}", summary.cycle_count);
        }
        _ => {
            println!("cycle_start: n/a");
            println!("cycle_end: n/a");
            println!("cycle_count: 0");
        }
    }

    match summary.ipc {
        Some(ipc) => println!("ipc: {ipc:.3}"),
        None => println!("ipc: n/a"),
    }

    if summary.stall_reasons.is_empty() {
        println!("stall_reasons: none");
    } else {
        println!("stall_reasons:");
        for (reason, count) in &summary.stall_reasons {
            println!("  {reason}: {count}");
        }
    }

    if summary.bottlenecks.is_empty() {
        println!("bottlenecks: none");
    } else {
        println!("bottlenecks:");
        for (reason, count) in &summary.bottlenecks {
            println!("  {reason}: {count}");
        }
    }

    if summary.top_bottlenecks.is_empty() {
        println!("top_bottlenecks: none");
    } else {
        println!("top_bottlenecks:");
        for entry in &summary.top_bottlenecks {
            println!("  {}: {}", entry.key, entry.count);
        }
    }

    if summary.flush_reasons.is_empty() {
        println!("flush_reasons: none");
    } else {
        println!("flush_reasons:");
        for (reason, count) in &summary.flush_reasons {
            println!("  {reason}: {count}");
        }
    }

    if summary.replay_reasons.is_empty() {
        println!("replay_reasons: none");
    } else {
        println!("replay_reasons:");
        for (reason, count) in &summary.replay_reasons {
            println!("  {reason}: {count}");
        }
    }

    if summary.status_counts.is_empty() {
        println!("statuses: none");
    } else {
        println!("statuses:");
        for (status, count) in &summary.status_counts {
            println!("  {status}: {count}");
        }
    }

    if summary.stage_occupancy.is_empty() {
        println!("stage_occupancy: none");
    } else {
        println!("stage_occupancy:");
        for (stage, cycles) in &summary.stage_occupancy {
            println!("  {stage}: {cycles}");
        }
    }

    print_span_stats("stage_stats", &summary.stage_stats);
    print_span_stats("lane_stats", &summary.lane_stats);

    match &summary.retired_latency {
        Some(latency) => {
            println!("retired_latency:");
            println!("  count: {}", latency.count);
            println!("  min: {}", latency.min);
            println!("  max: {}", latency.max);
            println!("  average: {:.3}", latency.average);
        }
        None => println!("retired_latency: none"),
    }
}

fn print_span_stats(label: &str, stats: &std::collections::BTreeMap<String, SpanStats>) {
    if stats.is_empty() {
        println!("{label}: none");
        return;
    }

    println!("{label}:");
    for (key, stats) in stats {
        println!(
            "  {key}: total={} spans={} average={:.3} max={}",
            stats.total_cycles, stats.span_count, stats.average_duration, stats.max_duration
        );
    }
}
