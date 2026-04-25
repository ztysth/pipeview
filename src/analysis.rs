use std::collections::BTreeMap;

use crate::model::{KeyValue, Trace};

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub cycle_start: Option<u64>,
    pub cycle_end: Option<u64>,
    pub cycle_count: u64,
    pub instruction_count: usize,
    pub retired_count: usize,
    pub span_count: usize,
    pub stage_count: usize,
    pub lane_count: usize,
    pub ipc: Option<f64>,
    pub stall_reasons: BTreeMap<String, u64>,
    pub bottlenecks: BTreeMap<String, u64>,
    pub status_counts: BTreeMap<String, u64>,
    pub stage_occupancy: BTreeMap<String, u64>,
    pub retired_latency: Option<LatencyStats>,
    pub flush_reasons: BTreeMap<String, u64>,
    pub replay_reasons: BTreeMap<String, u64>,
    pub top_bottlenecks: Vec<CountEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LatencyStats {
    pub count: usize,
    pub min: u64,
    pub max: u64,
    pub average: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountEntry {
    pub key: String,
    pub count: u64,
}

pub fn summarize(trace: &Trace) -> Summary {
    let mut cycle_start = None;
    let mut cycle_end = None;

    for span in &trace.spans {
        include_cycle(&mut cycle_start, &mut cycle_end, span.cycle);
        if let Some(last_cycle) = span.cycle.checked_add(span.duration - 1) {
            include_cycle(&mut cycle_start, &mut cycle_end, last_cycle);
        }
    }

    for event in &trace.events {
        include_cycle(&mut cycle_start, &mut cycle_end, event.cycle);
    }

    for counter in &trace.counters {
        include_cycle(&mut cycle_start, &mut cycle_end, counter.cycle);
    }

    for retire in &trace.retires {
        include_cycle(&mut cycle_start, &mut cycle_end, retire.cycle);
    }

    let cycle_count = match (cycle_start, cycle_end) {
        (Some(start), Some(end)) => end - start + 1,
        _ => 0,
    };
    let retired_count = trace
        .retires
        .iter()
        .filter(|retire| retire.status == "retire")
        .count();
    let ipc = (cycle_count > 0).then_some(retired_count as f64 / cycle_count as f64);

    let bottlenecks = bottlenecks(trace);

    Summary {
        cycle_start,
        cycle_end,
        cycle_count,
        instruction_count: trace.instructions.len(),
        retired_count,
        span_count: trace.spans.len(),
        stage_count: trace.stages.len(),
        lane_count: trace.lanes.len(),
        ipc,
        stall_reasons: stall_reasons(trace),
        top_bottlenecks: top_counts(&bottlenecks, 8),
        bottlenecks,
        status_counts: status_counts(trace),
        stage_occupancy: stage_occupancy(trace),
        retired_latency: retired_latency(trace),
        flush_reasons: lane_and_event_reasons(trace, "flush"),
        replay_reasons: lane_and_event_reasons(trace, "replay"),
    }
}

fn include_cycle(cycle_start: &mut Option<u64>, cycle_end: &mut Option<u64>, cycle: u64) {
    *cycle_start = Some(cycle_start.map_or(cycle, |start| start.min(cycle)));
    *cycle_end = Some(cycle_end.map_or(cycle, |end| end.max(cycle)));
}

fn stall_reasons(trace: &Trace) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();

    for span in &trace.spans {
        if span.lane == "stall" {
            let reason = attr_value(&span.attrs, "reason").unwrap_or("unknown");
            *counts.entry(reason.to_owned()).or_insert(0) += span.duration;
        }
    }

    for event in &trace.events {
        if event.event == "stall" {
            let reason = attr_value(&event.attrs, "reason").unwrap_or("unknown");
            *counts.entry(reason.to_owned()).or_insert(0) += 1;
        }
    }

    counts
}

fn bottlenecks(trace: &Trace) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();

    for span in &trace.spans {
        if span.lane != "main" {
            let reason = attr_value(&span.attrs, "reason").unwrap_or("unknown");
            let key = format!("{}:{reason}", span.lane);
            *counts.entry(key).or_insert(0) += span.duration;
        }
    }

    for event in &trace.events {
        let reason = attr_value(&event.attrs, "reason").unwrap_or("unknown");
        let key = format!("event:{}:{reason}", event.event);
        *counts.entry(key).or_insert(0) += 1;
    }

    counts
}

fn status_counts(trace: &Trace) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();

    for retire in &trace.retires {
        *counts.entry(retire.status.clone()).or_insert(0) += 1;
    }

    counts
}

fn lane_and_event_reasons(trace: &Trace, kind: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();

    for span in &trace.spans {
        if span.lane == kind {
            let reason = attr_value(&span.attrs, "reason").unwrap_or("unknown");
            *counts.entry(reason.to_owned()).or_insert(0) += span.duration;
        }
    }

    for event in &trace.events {
        if event.event == kind {
            let reason = attr_value(&event.attrs, "reason").unwrap_or("unknown");
            *counts.entry(reason.to_owned()).or_insert(0) += 1;
        }
    }

    counts
}

fn top_counts(counts: &BTreeMap<String, u64>, limit: usize) -> Vec<CountEntry> {
    let mut entries = counts
        .iter()
        .map(|(key, count)| CountEntry {
            key: key.clone(),
            count: *count,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.key.cmp(&right.key))
    });
    entries.truncate(limit);
    entries
}

fn stage_occupancy(trace: &Trace) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();

    for span in &trace.spans {
        *counts.entry(span.stage.clone()).or_insert(0) += span.duration;
    }

    counts
}

fn retired_latency(trace: &Trace) -> Option<LatencyStats> {
    let mut first_cycle_by_inst = BTreeMap::new();
    for span in &trace.spans {
        first_cycle_by_inst
            .entry(span.inst_id)
            .and_modify(|cycle: &mut u64| *cycle = (*cycle).min(span.cycle))
            .or_insert(span.cycle);
    }

    let mut count = 0;
    let mut min = u64::MAX;
    let mut max = 0;
    let mut total = 0u128;

    for retire in &trace.retires {
        if retire.status != "retire" {
            continue;
        }
        let Some(first_cycle) = first_cycle_by_inst.get(&retire.inst_id) else {
            continue;
        };
        if retire.cycle < *first_cycle {
            continue;
        }
        let latency = retire.cycle - first_cycle + 1;
        count += 1;
        min = min.min(latency);
        max = max.max(latency);
        total += u128::from(latency);
    }

    (count > 0).then_some(LatencyStats {
        count,
        min,
        max,
        average: total as f64 / count as f64,
    })
}

fn attr_value<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.key == key)
        .map(|attr| attr.value.as_str())
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_plog;

    use super::summarize;

    #[test]
    fn summarizes_basic_pipeline_metrics() {
        let input = concat!(
            "PLOG\t1\n",
            "STAGE\tIF\tFetch\n",
            "STAGE\tID\tDecode\n",
            "LANE\tmain\tMain\n",
            "LANE\tstall\tStall\n",
            "I\t1\n",
            "I\t2\n",
            "B\t1\t2\t1\tmain\tIF\n",
            "B\t3\t2\t2\tstall\tID\treason=load_use\n",
            "E\t5\t2\tstall\treason=load_use\n",
            "R\t6\t1\tretire\n",
            "R\t7\t2\tflush\n",
        );
        let trace = parse_plog(input).expect("valid trace");
        let summary = summarize(&trace);

        assert_eq!(summary.cycle_start, Some(1));
        assert_eq!(summary.cycle_end, Some(7));
        assert_eq!(summary.cycle_count, 7);
        assert_eq!(summary.instruction_count, 2);
        assert_eq!(summary.retired_count, 1);
        assert_eq!(summary.span_count, 2);
        assert_eq!(summary.stage_count, 2);
        assert_eq!(summary.lane_count, 2);
        assert_eq!(summary.stall_reasons["load_use"], 3);
        assert_eq!(summary.bottlenecks["stall:load_use"], 2);
        assert_eq!(summary.bottlenecks["event:stall:load_use"], 1);
        assert_eq!(summary.top_bottlenecks[0].key, "stall:load_use");
        assert_eq!(summary.top_bottlenecks[0].count, 2);
        assert_eq!(summary.status_counts["retire"], 1);
        assert_eq!(summary.status_counts["flush"], 1);
        assert_eq!(summary.stage_occupancy["IF"], 2);
        assert_eq!(summary.stage_occupancy["ID"], 2);
        let latency = summary.retired_latency.expect("retired latency");
        assert_eq!(latency.count, 1);
        assert_eq!(latency.min, 6);
        assert_eq!(latency.max, 6);
        assert_eq!(latency.average, 6.0);
    }
}
