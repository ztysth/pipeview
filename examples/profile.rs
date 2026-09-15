//! Bounded, release-mode measurements without a terminal.
//! Run with a trace path, or --konata-scale for the active-window benchmark.
use std::fmt::Write;
use std::hint::black_box;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use pipeview::analysis::{SummaryOptions, summarize_for_tui, summarize_with_options};
use pipeview::konata::parse_konata_reader;
use pipeview::plog_io::{DEFAULT_MAX_INPUT_BYTES, InputFormat, read_trace};
use pipeview::tui::TraceView;

fn main() -> anyhow::Result<()> {
    let argument = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: profile <trace-path|--konata-scale>"))?;
    if argument == "--konata-scale" {
        return konata_scale();
    }

    let start = Instant::now();
    let trace = read_trace(
        Path::new(&argument),
        DEFAULT_MAX_INPUT_BYTES,
        InputFormat::Auto,
    )?;
    let parse_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    black_box(summarize_with_options(&trace, SummaryOptions::default()));
    let report_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    black_box(summarize_for_tui(&trace, SummaryOptions::default()));
    let tui_summary_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let view = TraceView::new(&trace);
    let view_ms = start.elapsed().as_secs_f64() * 1000.0;
    // One query per sampled instruction, without App's detail cache.
    let ids: Vec<_> = view
        .rows()
        .iter()
        .step_by(view.rows().len().div_ceil(1024).max(1))
        .map(|row| row.inst_id())
        .collect();
    let start = Instant::now();
    for &id in &ids {
        black_box(view.instruction_detail(&trace, id));
    }
    let detail_us =
        (!ids.is_empty()).then(|| start.elapsed().as_secs_f64() * 1e6 / ids.len() as f64);
    println!(
        "{}",
        serde_json::json!({
            "path": argument, "instructions": trace.instructions.len(),
            "spans": trace.spans.len(), "span_bytes": std::mem::size_of::<pipeview::model::Span>(),
            "parse_ms": parse_ms, "report_ms": report_ms, "tui_summary_ms": tui_summary_ms,
            "view_ms": view_ms, "detail_queries": ids.len(), "detail_us": detail_us,
        })
    );
    Ok(())
}

fn konata_scale() -> anyhow::Result<()> {
    for count in [1000, 2000, 4000, 8000, 16000] {
        let mut input = String::from("Kanata\t0004\nC=\t0\n");
        for id in 0..count {
            writeln!(input, "I\t{id}\t{id}\t0\nS\t{id}\t0\tIF")?;
        }
        input.push_str("C\t1\n");
        for id in 0..count {
            writeln!(input, "R\t{id}\t{id}\t0")?;
        }
        let mut samples = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            let trace = parse_konata_reader(Cursor::new(&input))?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(trace.spans.len(), count);
            black_box(trace);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({
                "active_instructions": count, "median_ms": samples[1], "samples_ms": samples,
            })
        );
    }
    Ok(())
}
