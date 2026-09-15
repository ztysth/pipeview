use std::time::{Duration, Instant};

use pipeview::analysis::summarize;
use pipeview::parser::parse_plog;
use pipeview::tui::{TraceView, build_timeline_rows};

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

#[test]
fn large_fixture_trace_view_indexes_spans_without_copying_them() {
    let trace = parse_plog(CLASSIC_5STAGE).expect("large fixture parses");
    let start = Instant::now();
    let view = TraceView::new(&trace);
    let elapsed = start.elapsed();
    let indexed_spans = view
        .rows()
        .iter()
        .map(|row| row.span_count())
        .sum::<usize>();

    assert_eq!(view.rows().len(), trace.instructions.len());
    assert_eq!(indexed_spans, trace.spans.len());
    assert!(
        elapsed < Duration::from_secs(1),
        "trace view build took {elapsed:?}"
    );

    eprintln!(
        "trace_view: instructions={} trace_spans={} indexed_spans={} elapsed_ms={:.3}",
        trace.instructions.len(),
        trace.spans.len(),
        indexed_spans,
        elapsed.as_secs_f64() * 1000.0
    );
}

#[test]
fn large_fixture_detail_lookups_scale_with_the_selected_instruction() {
    let trace = parse_plog(CLASSIC_5STAGE).expect("large fixture parses");
    let view = TraceView::new(&trace);
    let sampled = view
        .rows()
        .iter()
        .step_by((view.rows().len() / 256).max(1))
        .map(|row| row.inst_id())
        .collect::<Vec<_>>();

    assert!(sampled.len() > 16, "sampled only {} rows", sampled.len());

    let start = Instant::now();
    let mut visited_spans = 0;
    for &inst_id in &sampled {
        let detail = view
            .instruction_detail(&trace, inst_id)
            .expect("sampled instruction has a detail entry");
        assert_eq!(detail.inst_id, inst_id);
        visited_spans += detail.spans.len();
    }
    let elapsed = start.elapsed();
    let per_query = elapsed / sampled.len() as u32;

    assert!(
        per_query < Duration::from_millis(2),
        "detail lookup averaged {per_query:?} over {} queries",
        sampled.len()
    );

    eprintln!(
        "detail_lookup: queries={} visited_spans={} trace_spans={} per_query_us={:.3}",
        sampled.len(),
        visited_spans,
        trace.spans.len(),
        per_query.as_secs_f64() * 1_000_000.0
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
