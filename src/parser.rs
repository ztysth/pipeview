use std::io::BufRead;

use crate::error::{ParseError, ValidationError};
use crate::model::{
    AttrMap, Counter, Event, Instruction, KeyValue, Lane, RetireEvent, Span, SpanNames, Stage,
    Trace,
};
use crate::validate::validate_trace;

mod parallel;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawRecord<'a> {
    Header(u32),
    Meta(&'a str, &'a str),
    Stage {
        id: &'a str,
        label: &'a str,
        attrs: AttrMap,
    },
    Lane {
        id: &'a str,
        label: &'a str,
        attrs: AttrMap,
    },
    Instruction {
        inst_id: u64,
        attrs: AttrMap,
    },
    Span {
        cycle: u64,
        duration: u64,
        inst_id: u64,
        lane: &'a str,
        stage: &'a str,
        attrs: AttrMap,
    },
    Event {
        cycle: u64,
        inst_id: u64,
        event: &'a str,
        attrs: AttrMap,
    },
    Counter {
        cycle: u64,
        resource: &'a str,
        attrs: AttrMap,
    },
    Retire {
        cycle: u64,
        inst_id: u64,
        status: &'a str,
        attrs: AttrMap,
    },
}

/// Tab-separated field iterator. Fields are short, so a plain byte scan is
/// cheaper than `str::split`'s generic searcher.
pub(crate) struct Fields<'a> {
    rest: Option<&'a str>,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(line: &'a str) -> Self {
        Self { rest: Some(line) }
    }

    /// Everything after the fields consumed so far, tabs included.
    pub(crate) fn rest(self) -> &'a str {
        self.rest.unwrap_or_default()
    }

    fn remaining(&self) -> usize {
        self.rest.map_or(0, |rest| {
            rest.bytes().filter(|&byte| byte == b'\t').count() + 1
        })
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let rest = self.rest?;
        match rest.bytes().position(|byte| byte == b'\t') {
            Some(tab) => {
                self.rest = Some(&rest[tab + 1..]);
                Some(&rest[..tab])
            }
            None => {
                self.rest = None;
                Some(rest)
            }
        }
    }
}

pub fn parse_plog(input: &str) -> Result<Trace, ParseError> {
    if input.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    let mut builder = TraceBuilder::default();
    let mut saw_line = false;

    for (index, line) in input.lines().enumerate() {
        saw_line = true;
        builder.push_line(index + 1, line.strip_suffix('\r').unwrap_or(line))?;
    }

    if !saw_line {
        return Err(ParseError::EmptyInput);
    }

    builder.finish()
}

pub fn parse_plog_reader<R: BufRead>(reader: R) -> Result<Trace, ParseError> {
    match parallel::worker_count() {
        0 | 1 => parse_plog_reader_with_limit(reader, None),
        workers => parallel::parse_parallel(reader, workers),
    }
}

pub fn parse_plog_preview_reader<R: BufRead>(
    reader: R,
    span_limit: usize,
) -> Result<Trace, ParseError> {
    parse_plog_reader_with_limit(reader, Some(span_limit))
}

fn parse_plog_reader_with_limit<R: BufRead>(
    mut reader: R,
    span_limit: Option<usize>,
) -> Result<Trace, ParseError> {
    let mut builder = TraceBuilder::default();
    let mut buffer = Vec::new();
    let mut line_number = 0;
    let mut saw_line = false;

    loop {
        buffer.clear();
        line_number += 1;
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| ParseError::line(line_number, error.to_string()))?;
        if read == 0 {
            break;
        }

        saw_line = true;
        let line = std::str::from_utf8(&buffer)
            .map_err(|_| ParseError::line(line_number, "stream did not contain valid UTF-8"))?;
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        builder.push_line(line_number, line)?;

        if span_limit.is_some_and(|limit| builder.spans.len() >= limit) {
            break;
        }
    }

    if !saw_line {
        return Err(ParseError::EmptyInput);
    }

    if span_limit.is_some() {
        builder.finish_preview()
    } else {
        builder.finish()
    }
}

fn parse_line(line: &str) -> Result<RawRecord<'_>, String> {
    if line.contains('\n') || line.contains('\r') {
        return Err("malformed tab-separated record".to_string());
    }

    let mut fields = Fields::new(line);
    let Some(kind) = fields.next() else {
        return Err("empty record".to_string());
    };

    match kind {
        "PLOG" => parse_header_fields(line, &mut fields),
        "META" => parse_meta_fields(line, &mut fields),
        "STAGE" => parse_stage_fields(line, &mut fields),
        "LANE" => parse_lane_fields(line, &mut fields),
        "I" => parse_instruction_fields(line, &mut fields),
        "B" => parse_span_fields(line, &mut fields),
        "E" => parse_event_fields(line, &mut fields),
        "C" => parse_counter_fields(line, &mut fields),
        "R" => parse_retire_fields(line, &mut fields),
        other => Err(format!("unknown record kind `{other}`")),
    }
}

fn parse_header_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let version = take_exact(line, fields, 2, "PLOG")?;
    require_no_extra_fields(line, fields, 2, "PLOG")?;
    Ok(RawRecord::Header(parse_u32(version, "version")?))
}

fn parse_meta_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let key = take_exact(line, fields, 3, "META")?;
    let value = take_exact(line, fields, 3, "META")?;
    require_no_extra_fields(line, fields, 3, "META")?;
    require_non_empty(key, "metadata key")?;
    Ok(RawRecord::Meta(key, value))
}

fn parse_stage_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let id = take_at_least(line, fields, 3, "STAGE")?;
    let label = take_at_least(line, fields, 3, "STAGE")?;
    require_non_empty(id, "stage id")?;
    require_non_empty(label, "stage label")?;
    Ok(RawRecord::Stage {
        id,
        label,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_lane_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let id = take_at_least(line, fields, 3, "LANE")?;
    let label = take_at_least(line, fields, 3, "LANE")?;
    require_non_empty(id, "lane id")?;
    require_non_empty(label, "lane label")?;
    Ok(RawRecord::Lane {
        id,
        label,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_instruction_fields<'a>(
    line: &str,
    fields: &mut Fields<'a>,
) -> Result<RawRecord<'a>, String> {
    let inst_id = take_at_least(line, fields, 2, "I")?;
    Ok(RawRecord::Instruction {
        inst_id: parse_u64(inst_id, "instruction id")?,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_span_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let cycle = take_at_least(line, fields, 6, "B")?;
    let duration = take_at_least(line, fields, 6, "B")?;
    let inst_id = take_at_least(line, fields, 6, "B")?;
    let lane = take_at_least(line, fields, 6, "B")?;
    let stage = take_at_least(line, fields, 6, "B")?;
    require_non_empty(lane, "lane id")?;
    require_non_empty(stage, "stage id")?;
    Ok(RawRecord::Span {
        cycle: parse_u64(cycle, "cycle")?,
        duration: parse_u64(duration, "duration")?,
        inst_id: parse_u64(inst_id, "instruction id")?,
        lane,
        stage,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_event_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let cycle = take_at_least(line, fields, 4, "E")?;
    let inst_id = take_at_least(line, fields, 4, "E")?;
    let event = take_at_least(line, fields, 4, "E")?;
    require_non_empty(event, "event")?;
    Ok(RawRecord::Event {
        cycle: parse_u64(cycle, "cycle")?,
        inst_id: parse_u64(inst_id, "instruction id")?,
        event,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_counter_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let cycle = take_at_least(line, fields, 3, "C")?;
    let resource = take_at_least(line, fields, 3, "C")?;
    require_non_empty(resource, "resource")?;
    Ok(RawRecord::Counter {
        cycle: parse_u64(cycle, "cycle")?,
        resource,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_retire_fields<'a>(line: &str, fields: &mut Fields<'a>) -> Result<RawRecord<'a>, String> {
    let cycle = take_at_least(line, fields, 4, "R")?;
    let inst_id = take_at_least(line, fields, 4, "R")?;
    let status = take_at_least(line, fields, 4, "R")?;
    require_non_empty(status, "status")?;
    Ok(RawRecord::Retire {
        cycle: parse_u64(cycle, "cycle")?,
        inst_id: parse_u64(inst_id, "instruction id")?,
        status,
        attrs: parse_attrs(fields)?,
    })
}

fn parse_attrs(fields: &mut Fields<'_>) -> Result<AttrMap, String> {
    let mut attrs = Vec::with_capacity(fields.remaining());
    for field in fields {
        match field.split_once('=') {
            Some((key, value)) if !key.is_empty() && !value.is_empty() => attrs.push(KeyValue {
                key: key.to_owned(),
                value: value.to_owned(),
            }),
            _ => return Err(format!("malformed key/value attribute `{field}`")),
        }
    }
    Ok(attrs)
}

fn parse_u32(input: &str, label: &str) -> Result<u32, String> {
    parse_number(input).ok_or_else(|| format!("invalid {label} `{input}`"))
}

fn parse_u64(input: &str, label: &str) -> Result<u64, String> {
    parse_number(input).ok_or_else(|| format!("invalid {label} `{input}`"))
}

fn parse_number<T>(input: &str) -> Option<T>
where
    T: TryFrom<u64>,
{
    if input.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for byte in input.bytes() {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
    }
    T::try_from(value).ok()
}

fn take_exact<'a>(
    line: &str,
    fields: &mut Fields<'a>,
    expected: usize,
    kind: &str,
) -> Result<&'a str, String> {
    fields
        .next()
        .ok_or_else(|| exact_field_count_error(line, expected, kind))
}

fn require_no_extra_fields(
    line: &str,
    fields: &mut Fields<'_>,
    expected: usize,
    kind: &str,
) -> Result<(), String> {
    if fields.next().is_some() {
        return Err(exact_field_count_error(line, expected, kind));
    }

    Ok(())
}

fn take_at_least<'a>(
    line: &str,
    fields: &mut Fields<'a>,
    expected: usize,
    kind: &str,
) -> Result<&'a str, String> {
    fields
        .next()
        .ok_or_else(|| min_field_count_error(line, expected, kind))
}

fn exact_field_count_error(line: &str, expected: usize, kind: &str) -> String {
    format!(
        "{kind} record expects {expected} fields, got {}",
        field_count(line)
    )
}

fn min_field_count_error(line: &str, expected: usize, kind: &str) -> String {
    format!(
        "{kind} record expects at least {expected} fields, got {}",
        field_count(line)
    )
}

fn field_count(line: &str) -> usize {
    line.split('\t').count()
}

fn require_non_empty(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct TraceBuilder {
    span_names: SpanNames,
    version: Option<u32>,
    meta: Vec<KeyValue>,
    stages: Vec<Stage>,
    lanes: Vec<Lane>,
    instructions: Vec<Instruction>,
    spans: Vec<Span>,
    events: Vec<Event>,
    counters: Vec<Counter>,
    retires: Vec<RetireEvent>,
}

impl TraceBuilder {
    fn push_line(&mut self, line_number: usize, line: &str) -> Result<(), ParseError> {
        let record = parse_line(line).map_err(|message| ParseError::line(line_number, message))?;
        self.push(record)
    }

    fn push(&mut self, record: RawRecord<'_>) -> Result<(), ParseError> {
        match record {
            RawRecord::Header(record_version) => {
                if self.version.replace(record_version).is_some() {
                    return Err(ValidationError::DuplicateHeader.into());
                }
            }
            RawRecord::Meta(key, value) => self.meta.push(KeyValue {
                key: key.to_owned(),
                value: value.to_owned(),
            }),
            RawRecord::Stage { id, label, attrs } => self.stages.push(Stage {
                id: id.to_owned(),
                label: label.to_owned(),
                attrs,
            }),
            RawRecord::Lane { id, label, attrs } => self.lanes.push(Lane {
                id: id.to_owned(),
                label: label.to_owned(),
                attrs,
            }),
            RawRecord::Instruction { inst_id, attrs } => {
                self.instructions.push(Instruction { inst_id, attrs })
            }
            RawRecord::Span {
                cycle,
                duration,
                inst_id,
                lane,
                stage,
                attrs,
            } => self.spans.push(Span {
                cycle,
                duration,
                inst_id,
                lane: self.span_names.intern(lane),
                stage: self.span_names.intern(stage),
                attrs,
            }),
            RawRecord::Event {
                cycle,
                inst_id,
                event,
                attrs,
            } => self.events.push(Event {
                cycle,
                inst_id,
                event: event.to_owned(),
                attrs,
            }),
            RawRecord::Counter {
                cycle,
                resource,
                attrs,
            } => self.counters.push(Counter {
                cycle,
                resource: resource.to_owned(),
                attrs,
            }),
            RawRecord::Retire {
                cycle,
                inst_id,
                status,
                attrs,
            } => self.retires.push(RetireEvent {
                cycle,
                inst_id,
                status: status.to_owned(),
                attrs,
            }),
        }

        Ok(())
    }

    fn finish(self) -> Result<Trace, ParseError> {
        let trace = Trace {
            version: self.version.ok_or(ValidationError::MissingHeader)?,
            meta: self.meta,
            stages: self.stages,
            lanes: self.lanes,
            instructions: self.instructions,
            spans: self.spans,
            events: self.events,
            counters: self.counters,
            retires: self.retires,
        };

        validate_trace(&trace)?;
        Ok(trace)
    }

    fn finish_preview(self) -> Result<Trace, ParseError> {
        Ok(Trace {
            version: self.version.ok_or(ValidationError::MissingHeader)?,
            meta: self.meta,
            stages: self.stages,
            lanes: self.lanes,
            instructions: self.instructions,
            spans: self.spans,
            events: self.events,
            counters: self.counters,
            retires: self.retires,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_plog;
    use crate::error::{ParseError, ValidationError};

    #[test]
    fn parses_valid_plog_records() {
        let input = concat!(
            "PLOG\t1\n",
            "META\tname\tload-use\n",
            "STAGE\tIF\tFetch\tgroup=frontend\torder=10\tcap=1\n",
            "STAGE\tID\tDecode\tgroup=frontend\torder=20\n",
            "LANE\tmain\tMain\torder=0\n",
            "LANE\tstall\tStall\torder=1\n",
            "I\t1\tpc=0x80000000\tasm=lw_x1_0_x2\n",
            "I\t2\tpc=0x80000004\tasm=add_x3_x1_x4\n",
            "B\t1\t1\t1\tmain\tIF\n",
            "B\t2\t1\t1\tmain\tID\n",
            "B\t4\t1\t2\tstall\tID\treason=load_use\n",
            "E\t4\t2\tstall\treason=load_use\n",
            "C\t4\tload_queue\tfull=false\n",
            "R\t8\t1\tretire\n",
            "R\t9\t2\tretire\n",
        );

        let trace = parse_plog(input).expect("valid input parses");

        assert_eq!(trace.version, 1);
        assert_eq!(trace.meta[0].key, "name");
        assert_eq!(trace.stages.len(), 2);
        assert_eq!(trace.lanes.len(), 2);
        assert_eq!(trace.instructions.len(), 2);
        assert_eq!(trace.spans.len(), 3);
        assert_eq!(trace.events.len(), 1);
        assert_eq!(trace.counters.len(), 1);
        assert_eq!(trace.retires.len(), 2);
        assert_eq!(trace.spans[2].attrs[0].value, "load_use");
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(parse_plog(""), Err(ParseError::EmptyInput));
    }

    #[test]
    fn rejects_missing_header() {
        assert_eq!(
            parse_plog("STAGE\tIF\tFetch"),
            Err(ParseError::Validation(ValidationError::MissingHeader))
        );
    }

    #[test]
    fn rejects_unsupported_version() {
        assert_eq!(
            parse_plog("PLOG\t2"),
            Err(ParseError::Validation(ValidationError::UnsupportedVersion(
                2
            )))
        );
    }

    #[test]
    fn rejects_unknown_record_kind() {
        assert_eq!(
            parse_plog("PLOG\t1\nX\t1"),
            Err(ParseError::line(2, "unknown record kind `X`"))
        );
    }

    #[test]
    fn rejects_missing_required_field() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF"),
            Err(ParseError::line(
                2,
                "STAGE record expects at least 3 fields, got 2"
            ))
        );
    }

    #[test]
    fn rejects_non_numeric_cycle() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\nI\t1\nB\tx\t1\t1\tmain\tIF"),
            Err(ParseError::line(5, "invalid cycle `x`"))
        );
    }

    #[test]
    fn rejects_malformed_key_value_attribute() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\torder"),
            Err(ParseError::line(2, "malformed key/value attribute `order`"))
        );
    }

    #[test]
    fn rejects_duplicate_stage_id() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\nSTAGE\tIF\tFetch2"),
            Err(ParseError::Validation(ValidationError::DuplicateStage(
                "IF".to_owned()
            )))
        );
    }

    #[test]
    fn rejects_duplicate_lane_id() {
        assert_eq!(
            parse_plog("PLOG\t1\nLANE\tmain\tMain\nLANE\tmain\tMain2"),
            Err(ParseError::Validation(ValidationError::DuplicateLane(
                "main".to_owned()
            )))
        );
    }

    #[test]
    fn rejects_duplicate_instruction_id() {
        assert_eq!(
            parse_plog("PLOG\t1\nI\t1\nI\t1"),
            Err(ParseError::Validation(
                ValidationError::DuplicateInstruction(1)
            ))
        );
    }

    #[test]
    fn rejects_zero_duration_span() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\nI\t1\nB\t1\t0\t1\tmain\tIF"),
            Err(ParseError::Validation(ValidationError::ZeroDuration {
                cycle: 1,
                inst_id: 1,
            }))
        );
    }

    #[test]
    fn rejects_unknown_instruction_reference() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\nB\t1\t1\t99\tmain\tIF"),
            Err(ParseError::Validation(ValidationError::UnknownInstruction(
                99
            )))
        );
    }

    #[test]
    fn rejects_unknown_stage_reference() {
        assert_eq!(
            parse_plog("PLOG\t1\nLANE\tmain\tMain\nI\t1\nB\t1\t1\t1\tmain\tIF"),
            Err(ParseError::Validation(ValidationError::UnknownStage(
                "IF".to_owned()
            )))
        );
    }

    #[test]
    fn rejects_unknown_lane_reference() {
        assert_eq!(
            parse_plog("PLOG\t1\nSTAGE\tIF\tFetch\nI\t1\nB\t1\t1\t1\tmain\tIF"),
            Err(ParseError::Validation(ValidationError::UnknownLane(
                "main".to_owned()
            )))
        );
    }

    #[test]
    fn rejects_span_cycle_overflow() {
        assert_eq!(
            parse_plog(
                "PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\nI\t1\nB\t18446744073709551615\t1\t1\tmain\tIF"
            ),
            Err(ParseError::Validation(ValidationError::SpanCycleOverflow {
                cycle: u64::MAX,
                inst_id: 1,
            }))
        );
    }
}
