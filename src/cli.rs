use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::analysis::{Summary, summarize};
use crate::parser::parse_plog;
use crate::tui;

#[derive(Debug, Parser)]
#[command(version, about = "Visualize configurable CPU and RTL pipeline logs")]
pub struct Args {
    /// Open a PLog file in the terminal timeline view.
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a PLog file.
    Validate { path: PathBuf },

    /// Print a text summary for a PLog file.
    Report { path: PathBuf },
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    match (args.command, args.path) {
        (Some(Command::Validate { path }), None) => {
            load_trace(&path)?;
            println!("valid: {}", path.display());
        }
        (Some(Command::Report { path }), None) => {
            let trace = load_trace(&path)?;
            print_report(&path, &summarize(&trace));
        }
        (None, Some(path)) => {
            let trace = load_trace(&path)?;
            tui::run(&path, trace)?;
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

fn load_trace(path: &Path) -> Result<crate::model::Trace> {
    let input =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_plog(&input).with_context(|| format!("failed to parse {}", path.display()))
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
