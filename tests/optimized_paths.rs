use pipeview::analysis::{SummaryOptions, summarize, summarize_for_tui};
use pipeview::konata::{parse_konata_preview_reader, parse_konata_reader};
use pipeview::parser::parse_plog;
use pipeview::tui::TraceView;
use std::io::Cursor;

#[test]
fn span_names_share_storage_and_outlive_the_parser_and_original_trace() {
    let input = "PLOG\t1\nB\t0\t2\t99\tmain\tIF\nB\t2\t1\t99\tmain\tIF\nI\t99\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\n";
    let trace = parse_plog(input).unwrap();
    assert!(std::ptr::eq(
        trace.spans[0].stage.as_str(),
        trace.spans[1].stage.as_str()
    ));
    assert!(std::ptr::eq(
        trace.spans[0].lane.as_str(),
        trace.spans[1].lane.as_str()
    ));
    let cloned = trace.clone();
    drop(trace);
    assert_eq!(cloned.spans[0].stage, "IF");
    assert_eq!(cloned, parse_plog(input).unwrap());
    let view = TraceView::new(&cloned);
    assert_eq!(
        view.instruction_detail(&cloned, 99).unwrap().spans[0].stage,
        "IF"
    );
    assert!(std::mem::size_of::<pipeview::model::Span>() < 96);
}

#[test]
fn reports_preserve_colliding_keys_sparse_ids_and_out_of_order_spans() {
    let trace = parse_plog(concat!(
        "PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tevent:x\tA\nLANE\tevent\tB\nLANE\tmain\tMain\n",
        "I\t9000000000\nI\t7\n",
        "B\t8\t3\t7\tevent:x\tIF\treason=y\n",
        "B\t2\t4\t7\tevent\tIF\treason=x:y\n",
        "B\t3\t1\t9000000000\tmain\tIF\n",
        "E\t5\t7\tx\treason=y\n",
        "R\t12\t7\tretire\nR\t14\t7\tretire\n",
        "R\t1\t9000000000\tretire\nR\t8\t42\tretire\n",
    ))
    .unwrap();
    let summary = summarize(&trace);
    assert_eq!(summary.bottlenecks["event:x:y"], 8);
    assert_eq!(summary.stage_occupancy["IF"], 8);
    assert_eq!(summary.stage_stats["IF"].total_cycles, 8);
    assert_eq!(summary.stage_stats["IF"].max_duration, 4);
    let latency = summary.retired_latency.unwrap();
    assert_eq!(
        (latency.count, latency.min, latency.max, latency.average),
        (2, 11, 13, 12.0)
    );
    let tui = summarize_for_tui(
        &trace,
        SummaryOptions {
            experimental_bottlenecks: true,
        },
    );
    assert_eq!(tui.bottlenecks, summary.bottlenecks);
    assert_eq!(tui.status_counts, summary.status_counts);
    assert_eq!(tui.ipc, summary.ipc);
}

#[test]
fn konata_stage_matching_lane_replacement_and_retirement() {
    let trace = parse_konata_reader(Cursor::new(concat!(
        "Kanata\t0004\nC=\t0\nI\t1\t1\t0\nI\t2\t2\t0\n",
        "S\t1\t0\tIF\nS\t1\t1\tWAIT\nS\t2\t0\tIF\n",
        "C\t2\nE\t1\t0\tWRONG\nS\t1\t0\tEX\n",
        "C\t3\nE\t1\t0\tIF\nR\t1\t1\t0\nE\t2\t0\tIF\n",
    )))
    .unwrap();
    let mut spans: Vec<_> = trace
        .spans
        .iter()
        .map(|s| {
            (
                s.inst_id,
                s.lane.as_str(),
                s.stage.as_str(),
                s.cycle,
                s.duration,
            )
        })
        .collect();
    spans.sort_unstable();
    assert_eq!(
        spans,
        vec![
            (1, "lane1", "WAIT", 0, 5),
            (1, "main", "EX", 2, 3),
            (1, "main", "IF", 0, 2),
            (2, "main", "IF", 0, 5)
        ]
    );
    assert!(std::ptr::eq(
        trace.spans[0].stage.as_str(),
        trace.spans.last().unwrap().stage.as_str()
    ));
}

#[test]
fn konata_preview_and_eof_close_remaining_lanes() {
    let text = "Kanata\t0004\nC=\t-2\nI\t10\t10\t0\nS\t10\t0\tIF\nS\t10\t2\tWAIT\nC\t6\nI\t20\t20\t0\nS\t20\t0\tIF\n";
    let preview = parse_konata_preview_reader(Cursor::new(text), 2).unwrap();
    assert_eq!(preview.spans.len(), 2);
    assert!(
        preview
            .spans
            .iter()
            .all(|s| s.inst_id == 10 && s.cycle == 0 && s.duration == 4)
    );
    let trace = parse_konata_reader(Cursor::new(text)).unwrap();
    assert_eq!(trace.spans.len(), 3);
    let last = trace.spans.iter().find(|s| s.inst_id == 20).unwrap();
    assert_eq!((last.cycle, last.duration), (4, 1));
}
