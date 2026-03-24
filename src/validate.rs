use std::collections::HashSet;

use crate::error::ValidationError;
use crate::model::Trace;

pub fn validate_trace(trace: &Trace) -> Result<(), ValidationError> {
    if trace.version != 1 {
        return Err(ValidationError::UnsupportedVersion(trace.version));
    }

    let mut stage_ids = HashSet::with_capacity(trace.stages.len());
    for stage in &trace.stages {
        if !stage_ids.insert(stage.id.as_str()) {
            return Err(ValidationError::DuplicateStage(stage.id.clone()));
        }
    }

    let mut lane_ids = HashSet::with_capacity(trace.lanes.len());
    for lane in &trace.lanes {
        if !lane_ids.insert(lane.id.as_str()) {
            return Err(ValidationError::DuplicateLane(lane.id.clone()));
        }
    }

    let mut instruction_ids = HashSet::with_capacity(trace.instructions.len());
    for instruction in &trace.instructions {
        if !instruction_ids.insert(instruction.inst_id) {
            return Err(ValidationError::DuplicateInstruction(instruction.inst_id));
        }
    }

    for span in &trace.spans {
        if span.duration == 0 {
            return Err(ValidationError::ZeroDuration {
                cycle: span.cycle,
                inst_id: span.inst_id,
            });
        }

        if span.cycle.checked_add(span.duration).is_none() {
            return Err(ValidationError::SpanCycleOverflow {
                cycle: span.cycle,
                inst_id: span.inst_id,
            });
        }

        if !instruction_ids.contains(&span.inst_id) {
            return Err(ValidationError::UnknownInstruction(span.inst_id));
        }

        if !lane_ids.contains(span.lane.as_str()) {
            return Err(ValidationError::UnknownLane(span.lane.clone()));
        }

        if !stage_ids.contains(span.stage.as_str()) {
            return Err(ValidationError::UnknownStage(span.stage.clone()));
        }
    }

    Ok(())
}
