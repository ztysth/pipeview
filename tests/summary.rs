use pipeview::analysis::summarize;
use pipeview::parser::parse_plog;

#[test]
fn summary_counts_cycles_ipc_statuses_and_bottlenecks() {
    let trace = parse_plog(concat!(
        "PLOG\t1\n",
        "STAGE\tFE\tFetch\n",
        "STAGE\tEX\tExecute\n",
        "STAGE\tMEM\tMemory\n",
        "STAGE\tCOM\tCommit\n",
        "LANE\tmain\tMain\n",
        "LANE\tstall\tStall\n",
        "LANE\treplay\tReplay\n",
        "LANE\tflush\tFlush\n",
        "I\t1\tpc=0x1000\tasm=ld\n",
        "I\t2\tpc=0x1004\tasm=add\n",
        "I\t3\tpc=0x1008\tasm=br\n",
        "B\t10\t1\t1\tmain\tFE\n",
        "B\t11\t3\t1\tstall\tMEM\treason=dcache_miss\n",
        "B\t14\t1\t1\tmain\tCOM\n",
        "B\t11\t1\t2\tmain\tFE\n",
        "B\t12\t2\t2\treplay\tEX\treason=memory_order_violation\n",
        "B\t14\t1\t2\tmain\tCOM\n",
        "B\t12\t1\t3\tmain\tFE\n",
        "B\t13\t2\t3\tflush\tEX\treason=branch_mispredict\n",
        "E\t11\t1\tstall\treason=dcache_miss\n",
        "E\t12\t2\treplay\treason=memory_order_violation\n",
        "E\t13\t3\tflush\treason=branch_mispredict\n",
        "C\t12\trob\tused=32\n",
        "R\t15\t1\tretire\n",
        "R\t16\t2\tretire\n",
        "R\t17\t3\tflush\n",
    ))
    .expect("valid PLog parses");

    let summary = summarize(&trace);

    assert_eq!(summary.cycle_start, Some(10));
    assert_eq!(summary.cycle_end, Some(17));
    assert_eq!(summary.cycle_count, 8);
    assert_eq!(summary.instruction_count, 3);
    assert_eq!(summary.retired_count, 2);
    assert_eq!(summary.span_count, 8);
    assert_eq!(summary.stage_count, 4);
    assert_eq!(summary.lane_count, 4);
    assert_eq!(summary.ipc, Some(0.25));
    assert_eq!(summary.stall_reasons["dcache_miss"], 4);
    assert_eq!(summary.bottlenecks["stall:dcache_miss"], 3);
    assert_eq!(summary.bottlenecks["replay:memory_order_violation"], 2);
    assert_eq!(summary.bottlenecks["flush:branch_mispredict"], 2);
    assert_eq!(summary.bottlenecks["event:stall:dcache_miss"], 1);
    assert_eq!(
        summary.bottlenecks["event:replay:memory_order_violation"],
        1
    );
    assert_eq!(summary.bottlenecks["event:flush:branch_mispredict"], 1);
    assert_eq!(summary.top_bottlenecks[0].key, "stall:dcache_miss");
    assert_eq!(summary.top_bottlenecks[0].count, 3);
    assert_eq!(summary.flush_reasons["branch_mispredict"], 3);
    assert_eq!(summary.replay_reasons["memory_order_violation"], 3);
    assert_eq!(summary.status_counts["retire"], 2);
    assert_eq!(summary.status_counts["flush"], 1);
    assert_eq!(summary.stage_occupancy["FE"], 3);
    assert_eq!(summary.stage_occupancy["EX"], 4);
    assert_eq!(summary.stage_occupancy["MEM"], 3);
    assert_eq!(summary.stage_occupancy["COM"], 2);
    assert_eq!(summary.stage_stats["FE"].span_count, 3);
    assert_eq!(summary.stage_stats["FE"].total_cycles, 3);
    assert_eq!(summary.stage_stats["FE"].max_duration, 1);
    assert_eq!(summary.stage_stats["EX"].span_count, 2);
    assert_eq!(summary.stage_stats["EX"].total_cycles, 4);
    assert_eq!(summary.stage_stats["EX"].average_duration, 2.0);
    assert_eq!(summary.stage_stats["EX"].max_duration, 2);
    assert_eq!(summary.lane_stats["main"].span_count, 5);
    assert_eq!(summary.lane_stats["main"].total_cycles, 5);
    assert_eq!(summary.lane_stats["stall"].span_count, 1);
    assert_eq!(summary.lane_stats["stall"].total_cycles, 3);
    assert_eq!(summary.lane_stats["stall"].average_duration, 3.0);
    assert_eq!(summary.lane_stats["replay"].max_duration, 2);

    let latency = summary.retired_latency.expect("retired latency");
    assert_eq!(latency.count, 2);
    assert_eq!(latency.min, 6);
    assert_eq!(latency.max, 6);
    assert_eq!(latency.average, 6.0);
}
