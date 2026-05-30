use std::time::{Duration, Instant};

use pipeview::analysis::summarize;
use pipeview::parser::parse_plog;
use pipeview::tui::build_timeline_rows;

const CLASSIC_5STAGE: &str = include_str!("../examples/classic_5stage_bottleneck.plog");
const CLASSIC_OOO: &str = include_str!("../examples/classic_ooo_bottleneck.plog");

#[test]
fn large_fixtures_parse_and_summarize_within_smoke_threshold() {
    let five_stage = time_parse_and_report("classic_5stage_bottleneck", CLASSIC_5STAGE);
    let ooo = time_parse_and_report("classic_ooo_bottleneck", CLASSIC_OOO);

    assert!(
        five_stage < Duration::from_secs(5),
        "classic_5stage_bottleneck took {five_stage:?}"
    );
    assert!(
        ooo < Duration::from_secs(5),
        "classic_ooo_bottleneck took {ooo:?}"
    );
}

#[test]
fn large_fixture_timeline_rows_build_without_cycle_expansion() {
    let trace = parse_plog(CLASSIC_5STAGE).expect("large fixture parses");
    let start = Instant::now();
    let rows = build_timeline_rows(&trace);
    let elapsed = start.elapsed();
    let stored_spans = rows.iter().map(|row| row.spans.len()).sum::<usize>();

    assert_eq!(rows.len(), trace.instructions.len());
    assert_eq!(stored_spans, trace.spans.len());
    assert!(
        elapsed < Duration::from_secs(1),
        "timeline row build took {elapsed:?}"
    );

    eprintln!(
        "timeline_rows: instructions={} trace_spans={} stored_spans={} elapsed_ms={:.3}",
        trace.instructions.len(),
        trace.spans.len(),
        stored_spans,
        elapsed.as_secs_f64() * 1000.0
    );
}

fn time_parse_and_report(name: &str, input: &str) -> Duration {
    let start = Instant::now();
    let trace = parse_plog(input).expect("large fixture parses");
    let summary = summarize(&trace);
    let elapsed = start.elapsed();

    assert!(summary.instruction_count > 1_000);
    assert!(summary.span_count > 10_000);
    assert!(summary.ipc.is_some());
    assert!(!summary.bottlenecks.is_empty());

    eprintln!(
        "{name}: records={} instructions={} spans={} elapsed_ms={:.3}",
        input.lines().count(),
        summary.instruction_count,
        summary.span_count,
        elapsed.as_secs_f64() * 1000.0
    );

    elapsed
}
