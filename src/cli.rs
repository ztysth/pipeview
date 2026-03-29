use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::analysis::{Summary, summarize};
use crate::parser::parse_plog;

#[derive(Debug, Parser)]
#[command(version, about = "Visualize configurable CPU and RTL pipeline logs")]
pub struct Args {
    #[command(subcommand)]
    command: Command,
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

    match args.command {
        Command::Validate { path } => {
            load_trace(&path)?;
            println!("valid: {}", path.display());
        }
        Command::Report { path } => {
            let trace = load_trace(&path)?;
            print_report(&path, &summarize(&trace));
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
}
