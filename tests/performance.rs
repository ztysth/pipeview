use std::time::{Duration, Instant};

use pipeview::analysis::summarize;
use pipeview::parser::parse_plog;

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
