use nom::Parser;
use nom::bytes::complete::{tag, take_till, take_till1};
use nom::character::complete::{char, digit1};
use nom::combinator::{all_consuming, map_res};
use nom::multi::separated_list1;
use nom::sequence::separated_pair;
use nom::{IResult, error::ErrorKind};

use crate::error::{ParseError, ValidationError};
use crate::model::{
    AttrMap, Counter, Event, Instruction, KeyValue, Lane, RetireEvent, Span, Stage, Trace,
};
use crate::validate::validate_trace;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawRecord<'a> {
    Header(u32),
    Meta(&'a str, &'a str),
    Stage {
        id: &'a str,
        label: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Lane {
        id: &'a str,
        label: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Instruction {
        inst_id: u64,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Span {
        cycle: u64,
        duration: u64,
        inst_id: u64,
        lane: &'a str,
        stage: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Event {
        cycle: u64,
        inst_id: u64,
        event: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Counter {
        cycle: u64,
        resource: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
    Retire {
        cycle: u64,
        inst_id: u64,
        status: &'a str,
        attrs: Vec<RawKeyValue<'a>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawKeyValue<'a> {
    key: &'a str,
    value: &'a str,
}

pub fn parse_plog(input: &str) -> Result<Trace, ParseError> {
    if input.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    let mut builder = TraceBuilder::default();
    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let record = parse_line(line).map_err(|message| ParseError::line(line_number, message))?;
        builder.push(record)?;
    }

    builder.finish()
}

fn parse_line(line: &str) -> Result<RawRecord<'_>, String> {
    let (_, fields) = all_consuming(separated_list1(tag("\t"), field))
        .parse(line)
        .map_err(|_| "malformed tab-separated record".to_string())?;

    let Some(kind) = fields.first() else {
        return Err("empty record".to_string());
    };

    match *kind {
        "PLOG" => parse_header_fields(&fields),
        "META" => parse_meta_fields(&fields),
        "STAGE" => parse_stage_fields(&fields),
        "LANE" => parse_lane_fields(&fields),
        "I" => parse_instruction_fields(&fields),
        "B" => parse_span_fields(&fields),
        "E" => parse_event_fields(&fields),
        "C" => parse_counter_fields(&fields),
        "R" => parse_retire_fields(&fields),
        other => Err(format!("unknown record kind `{other}`")),
    }
}

fn field(input: &str) -> IResult<&str, &str> {
    take_till(|c| c == '\t' || c == '\n' || c == '\r')(input)
}

fn parse_header_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_field_count(fields, 2, "PLOG")?;
    Ok(RawRecord::Header(parse_u32(fields[1], "version")?))
}

fn parse_meta_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_field_count(fields, 3, "META")?;
    require_non_empty(fields[1], "metadata key")?;
    Ok(RawRecord::Meta(fields[1], fields[2]))
}

fn parse_stage_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 3, "STAGE")?;
    require_non_empty(fields[1], "stage id")?;
    require_non_empty(fields[2], "stage label")?;
    Ok(RawRecord::Stage {
        id: fields[1],
        label: fields[2],
        attrs: parse_attrs(&fields[3..])?,
    })
}

fn parse_lane_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 3, "LANE")?;
    require_non_empty(fields[1], "lane id")?;
    require_non_empty(fields[2], "lane label")?;
    Ok(RawRecord::Lane {
        id: fields[1],
        label: fields[2],
        attrs: parse_attrs(&fields[3..])?,
    })
}

fn parse_instruction_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 2, "I")?;
    Ok(RawRecord::Instruction {
        inst_id: parse_u64(fields[1], "instruction id")?,
        attrs: parse_attrs(&fields[2..])?,
    })
}

fn parse_span_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 6, "B")?;
    require_non_empty(fields[4], "lane id")?;
    require_non_empty(fields[5], "stage id")?;
    Ok(RawRecord::Span {
        cycle: parse_u64(fields[1], "cycle")?,
        duration: parse_u64(fields[2], "duration")?,
        inst_id: parse_u64(fields[3], "instruction id")?,
        lane: fields[4],
        stage: fields[5],
        attrs: parse_attrs(&fields[6..])?,
    })
}

fn parse_event_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 4, "E")?;
    require_non_empty(fields[3], "event")?;
    Ok(RawRecord::Event {
        cycle: parse_u64(fields[1], "cycle")?,
        inst_id: parse_u64(fields[2], "instruction id")?,
        event: fields[3],
        attrs: parse_attrs(&fields[4..])?,
    })
}

fn parse_counter_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 3, "C")?;
    require_non_empty(fields[2], "resource")?;
    Ok(RawRecord::Counter {
        cycle: parse_u64(fields[1], "cycle")?,
        resource: fields[2],
        attrs: parse_attrs(&fields[3..])?,
    })
}

fn parse_retire_fields<'a>(fields: &[&'a str]) -> Result<RawRecord<'a>, String> {
    require_min_field_count(fields, 4, "R")?;
    require_non_empty(fields[3], "status")?;
    Ok(RawRecord::Retire {
        cycle: parse_u64(fields[1], "cycle")?,
        inst_id: parse_u64(fields[2], "instruction id")?,
        status: fields[3],
        attrs: parse_attrs(&fields[4..])?,
    })
}

fn parse_attrs<'a>(fields: &[&'a str]) -> Result<Vec<RawKeyValue<'a>>, String> {
    fields
        .iter()
        .map(|field| {
            let (_, attr) = all_consuming(parse_key_value)
                .parse(field)
                .map_err(|_| format!("malformed key/value attribute `{field}`"))?;
            Ok(attr)
        })
        .collect()
}

fn parse_key_value(input: &str) -> IResult<&str, RawKeyValue<'_>> {
    let (remaining, (key, value)) = separated_pair(
        take_till1(|c| c == '=' || c == '\t' || c == '\n' || c == '\r'),
        char('='),
        take_till(|c| c == '\t' || c == '\n' || c == '\r'),
    )
    .parse(input)?;

    if value.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            ErrorKind::TakeTill1,
        )));
    }

    Ok((remaining, RawKeyValue { key, value }))
}

fn parse_u32(input: &str, label: &str) -> Result<u32, String> {
    parse_number(input).map_err(|_| format!("invalid {label} `{input}`"))
}

fn parse_u64(input: &str, label: &str) -> Result<u64, String> {
    parse_number(input).map_err(|_| format!("invalid {label} `{input}`"))
}

fn parse_number<T>(input: &str) -> Result<T, nom::Err<nom::error::Error<&str>>>
where
    T: std::str::FromStr,
{
    let (_, number) = all_consuming(map_res(digit1, str::parse)).parse(input)?;
    Ok(number)
}

fn require_field_count(fields: &[&str], expected: usize, kind: &str) -> Result<(), String> {
    if fields.len() == expected {
        Ok(())
    } else {
        Err(format!(
            "{kind} record expects {expected} fields, got {}",
            fields.len()
        ))
    }
}

fn require_min_field_count(fields: &[&str], expected: usize, kind: &str) -> Result<(), String> {
    if fields.len() >= expected {
        Ok(())
    } else {
        Err(format!(
            "{kind} record expects at least {expected} fields, got {}",
            fields.len()
        ))
    }
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
                attrs: own_attrs(attrs),
            }),
            RawRecord::Lane { id, label, attrs } => self.lanes.push(Lane {
                id: id.to_owned(),
                label: label.to_owned(),
                attrs: own_attrs(attrs),
            }),
            RawRecord::Instruction { inst_id, attrs } => self.instructions.push(Instruction {
                inst_id,
                attrs: own_attrs(attrs),
            }),
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
                lane: lane.to_owned(),
                stage: stage.to_owned(),
                attrs: own_attrs(attrs),
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
                attrs: own_attrs(attrs),
            }),
            RawRecord::Counter {
                cycle,
                resource,
                attrs,
            } => self.counters.push(Counter {
                cycle,
                resource: resource.to_owned(),
                attrs: own_attrs(attrs),
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
                attrs: own_attrs(attrs),
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
}

fn own_attrs(attrs: Vec<RawKeyValue<'_>>) -> AttrMap {
    attrs
        .into_iter()
        .map(|attr| KeyValue {
            key: attr.key.to_owned(),
            value: attr.value.to_owned(),
        })
        .collect()
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
