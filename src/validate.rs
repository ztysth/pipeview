use std::collections::HashSet;

use crate::error::ValidationError;
use crate::model::{InstructionOrder, SpanName, Trace};

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

    let instructions = KnownInstructions::new(trace)?;
    let mut stages = KnownNames::new(stage_ids);
    let mut lanes = KnownNames::new(lane_ids);

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

        if !instructions.contains(span.inst_id) {
            return Err(ValidationError::UnknownInstruction(span.inst_id));
        }

        if !lanes.contains(&span.lane) {
            return Err(ValidationError::UnknownLane(span.lane.to_string()));
        }

        if !stages.contains(&span.stage) {
            return Err(ValidationError::UnknownStage(span.stage.to_string()));
        }
    }

    Ok(())
}

enum KnownInstructions {
    Ordered(InstructionOrder),
    Hashed(HashSet<u64>),
}

impl KnownInstructions {
    fn new(trace: &Trace) -> Result<Self, ValidationError> {
        // Ascending ids cannot repeat, so the common case skips hashing.
        let ascending = trace
            .instructions
            .windows(2)
            .all(|pair| pair[0].inst_id < pair[1].inst_id);
        if ascending {
            return Ok(Self::Ordered(InstructionOrder::new(&trace.instructions)));
        }

        let mut ids = HashSet::with_capacity(trace.instructions.len());
        for instruction in &trace.instructions {
            if !ids.insert(instruction.inst_id) {
                return Err(ValidationError::DuplicateInstruction(instruction.inst_id));
            }
        }
        Ok(Self::Hashed(ids))
    }

    fn contains(&self, inst_id: u64) -> bool {
        match self {
            Self::Ordered(order) => order.rank(inst_id).is_some(),
            Self::Hashed(ids) => ids.contains(&inst_id),
        }
    }
}

const MAX_REMEMBERED_NAMES: usize = 64;

/// Span names are interned, so each distinct name only needs one hash lookup;
/// later spans match it by pointer.
struct KnownNames<'a> {
    ids: HashSet<&'a str>,
    checked: Vec<*const String>,
}

impl<'a> KnownNames<'a> {
    fn new(ids: HashSet<&'a str>) -> Self {
        Self {
            ids,
            checked: Vec::new(),
        }
    }

    fn contains(&mut self, name: &SpanName) -> bool {
        let pointer = name.as_ptr();
        if self.checked.contains(&pointer) {
            return true;
        }
        if !self.ids.contains(name.as_str()) {
            return false;
        }
        if self.checked.len() < MAX_REMEMBERED_NAMES {
            self.checked.push(pointer);
        }
        true
    }
}
