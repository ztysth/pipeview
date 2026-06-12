use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::BufRead;

use crate::error::{ParseError, ValidationError};
use crate::model::{Instruction, KeyValue, Lane, RetireEvent, Span, Stage, Trace};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveKey {
    inst_id: u64,
    lane_id: i64,
    stage: String,
}

impl Hash for ActiveKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inst_id.hash(state);
        self.lane_id.hash(state);
        self.stage.hash(state);
    }
}

#[derive(Debug, Clone)]
struct ActiveStage {
    cycle: i64,
}

#[derive(Default)]
struct KonataBuilder {
    cycle: i64,
    saw_header: bool,
    instructions: HashMap<u64, Instruction>,
    labels: HashMap<u64, Vec<KeyValue>>,
    active: HashMap<ActiveKey, ActiveStage>,
    stages: HashSet<String>,
    lanes: HashSet<i64>,
    spans: Vec<Span>,
    retires: Vec<RetireEvent>,
    events: Vec<crate::model::Event>,
}

pub fn parse_konata_reader<R: BufRead>(reader: R) -> Result<Trace, ParseError> {
    parse_konata_reader_with_limit(reader, None)
}

pub fn parse_konata_preview_reader<R: BufRead>(
    reader: R,
    instruction_limit: usize,
) -> Result<Trace, ParseError> {
    parse_konata_reader_with_limit(reader, Some(instruction_limit))
}

fn parse_konata_reader_with_limit<R: BufRead>(
    reader: R,
    instruction_limit: Option<usize>,
) -> Result<Trace, ParseError> {
    let mut builder = KonataBuilder::default();
    let mut saw_line = false;
    let mut reader = reader;
    let mut line = String::new();
    let mut line_number = 0;

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|error| ParseError::line(line_number + 1, error.to_string()))?;
        if bytes == 0 {
            break;
        }
        line_number += 1;
        let line = strip_comment(&line);
        if line.trim().is_empty() {
            continue;
        }
        saw_line = true;
        builder
            .push_line(line)
            .map_err(|message| ParseError::line(line_number, message))?;
        if instruction_limit.is_some_and(|limit| builder.instructions.len() >= limit) {
            break;
        }
    }

    if !saw_line {
        return Err(ParseError::EmptyInput);
    }

    builder.finish()
}

fn strip_comment(line: &str) -> &str {
    line.split_once("//")
        .map_or(line, |(before_comment, _)| before_comment)
        .trim_end_matches('\r')
        .trim_end()
}

impl KonataBuilder {
    fn push_line(&mut self, line: &str) -> Result<(), String> {
        let mut fields = line.split('\t');
        let command = fields.next().unwrap_or_default();

        if !self.saw_header {
            let version = next_field(&mut fields, "Kanata", "version")?;
            ensure_no_more(fields, "Kanata")?;
            if command != "Kanata" {
                return Err("missing Kanata header".to_owned());
            }
            if version != "0004" {
                return Err(format!("unsupported Konata version `{version}`"));
            }
            self.saw_header = true;
            return Ok(());
        }

        match command {
            "C=" => {
                self.cycle = parse_i64(next_field(&mut fields, "C=", "cycle")?, "cycle")?;
                ensure_no_more(fields, "C=")?;
            }
            "C" => {
                let delta = parse_i64(next_field(&mut fields, "C", "cycle delta")?, "cycle delta")?;
                ensure_no_more(fields, "C")?;
                self.cycle = self
                    .cycle
                    .checked_add(delta)
                    .ok_or_else(|| "cycle overflow".to_owned())?;
            }
            "I" => {
                let inst_id = parse_u64(
                    next_field(&mut fields, "I", "instruction id")?,
                    "instruction id",
                )?;
                let sim_id = next_field(&mut fields, "I", "sim id")?.to_owned();
                let thread_id = next_field(&mut fields, "I", "thread id")?.to_owned();

                if self.instructions.contains_key(&inst_id) {
                    return Err(format!("duplicate instruction id {inst_id}"));
                }

                self.instructions.insert(
                    inst_id,
                    Instruction {
                        inst_id,
                        attrs: vec![
                            KeyValue {
                                key: "sim_id".to_owned(),
                                value: sim_id,
                            },
                            KeyValue {
                                key: "thread".to_owned(),
                                value: thread_id,
                            },
                        ],
                    },
                );
            }
            "L" => {
                let inst_id = parse_u64(
                    next_field(&mut fields, "L", "instruction id")?,
                    "instruction id",
                )?;
                let text_type = next_field(&mut fields, "L", "text type")?;
                if text_type == "0" {
                    let text = fields.collect::<Vec<_>>().join("\t");
                    self.labels.entry(inst_id).or_default().push(KeyValue {
                        key: "asm".to_owned(),
                        value: text,
                    });
                }
            }
            "S" => {
                let inst_id = parse_u64(
                    next_field(&mut fields, "S", "instruction id")?,
                    "instruction id",
                )?;
                let lane_id = parse_i64(next_field(&mut fields, "S", "lane id")?, "lane id")?;
                let stage = next_field(&mut fields, "S", "stage")?.to_owned();
                ensure_no_more(fields, "S")?;

                self.close_lane(inst_id, lane_id, self.cycle);
                self.stages.insert(stage.clone());
                self.lanes.insert(lane_id);
                self.active.insert(
                    ActiveKey {
                        inst_id,
                        lane_id,
                        stage,
                    },
                    ActiveStage { cycle: self.cycle },
                );
            }
            "E" => {
                let key = ActiveKey {
                    inst_id: parse_u64(
                        next_field(&mut fields, "E", "instruction id")?,
                        "instruction id",
                    )?,
                    lane_id: parse_i64(next_field(&mut fields, "E", "lane id")?, "lane id")?,
                    stage: next_field(&mut fields, "E", "stage")?.to_owned(),
                };
                ensure_no_more(fields, "E")?;
                self.close_key(&key, self.cycle);
            }
            "R" => {
                let inst_id = parse_u64(
                    next_field(&mut fields, "R", "instruction id")?,
                    "instruction id",
                )?;
                let retire_id = next_field(&mut fields, "R", "retire id")?.to_owned();
                let status = match next_field(&mut fields, "R", "status")? {
                    "0" => "retire",
                    "1" => "flush",
                    other => other,
                };
                ensure_no_more(fields, "R")?;

                self.close_instruction(inst_id, self.cycle);
                self.retires.push(RetireEvent {
                    cycle: cycle_to_u64(self.cycle),
                    inst_id,
                    status: status.to_owned(),
                    attrs: vec![KeyValue {
                        key: "retire_id".to_owned(),
                        value: retire_id,
                    }],
                });
            }
            "W" => {
                let consumer_id = parse_u64(
                    next_field(&mut fields, "W", "consumer instruction id")?,
                    "consumer instruction id",
                )?;
                let producer_id =
                    next_field(&mut fields, "W", "producer instruction id")?.to_owned();
                let dependency_type = next_field(&mut fields, "W", "dependency type")?.to_owned();
                ensure_no_more(fields, "W")?;
                self.events.push(crate::model::Event {
                    cycle: cycle_to_u64(self.cycle),
                    inst_id: consumer_id,
                    event: "dependency".to_owned(),
                    attrs: vec![
                        KeyValue {
                            key: "producer".to_owned(),
                            value: producer_id,
                        },
                        KeyValue {
                            key: "type".to_owned(),
                            value: dependency_type,
                        },
                    ],
                });
            }
            other => return Err(format!("unknown Konata command `{other}`")),
        }

        Ok(())
    }

    fn close_lane(&mut self, inst_id: u64, lane_id: i64, end_cycle: i64) {
        let keys = self
            .active
            .keys()
            .filter(|key| key.inst_id == inst_id && key.lane_id == lane_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.close_key(&key, end_cycle);
        }
    }

    fn close_instruction(&mut self, inst_id: u64, end_cycle: i64) {
        let keys = self
            .active
            .keys()
            .filter(|key| key.inst_id == inst_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.close_key(&key, end_cycle);
        }
    }

    fn close_key(&mut self, key: &ActiveKey, end_cycle: i64) {
        let Some(active) = self.active.remove(key) else {
            return;
        };
        let start_cycle = cycle_to_u64(active.cycle);
        let end_cycle = cycle_to_u64(end_cycle);
        let duration = end_cycle.saturating_sub(start_cycle).max(1);
        self.spans.push(Span {
            cycle: start_cycle,
            duration,
            inst_id: key.inst_id,
            lane: lane_name(key.lane_id),
            stage: key.stage.clone(),
            attrs: Vec::new(),
        });
    }

    fn finish(mut self) -> Result<Trace, ParseError> {
        if !self.saw_header {
            return Err(ParseError::Validation(ValidationError::MissingHeader));
        }

        let open_keys = self.active.keys().cloned().collect::<Vec<_>>();
        for key in open_keys {
            self.close_key(&key, self.cycle);
        }

        let mut instructions = self.instructions.into_values().collect::<Vec<_>>();
        instructions.sort_by_key(|instruction| instruction.inst_id);
        for instruction in &mut instructions {
            if let Some(labels) = self.labels.remove(&instruction.inst_id) {
                instruction.attrs.extend(labels);
            }
        }

        let mut stage_ids = self.stages.into_iter().collect::<Vec<_>>();
        stage_ids.sort();
        let stages = stage_ids
            .into_iter()
            .map(|id| Stage {
                label: id.clone(),
                id,
                attrs: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut lane_ids = self.lanes.into_iter().collect::<Vec<_>>();
        lane_ids.sort();
        let lanes = lane_ids
            .into_iter()
            .map(|id| Lane {
                id: lane_name(id),
                label: if id == 0 {
                    "Main".to_owned()
                } else {
                    format!("Lane {id}")
                },
                attrs: vec![KeyValue {
                    key: "konata_lane".to_owned(),
                    value: id.to_string(),
                }],
            })
            .collect::<Vec<_>>();

        let trace = Trace {
            version: 1,
            meta: vec![KeyValue {
                key: "source_format".to_owned(),
                value: "konata".to_owned(),
            }],
            stages,
            lanes,
            instructions,
            spans: self.spans,
            events: self.events,
            counters: Vec::new(),
            retires: self.retires,
        };

        Ok(trace)
    }
}

fn lane_name(id: i64) -> String {
    if id == 0 {
        "main".to_owned()
    } else {
        format!("lane{id}")
    }
}

fn next_field<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    kind: &str,
    label: &str,
) -> Result<&'a str, String> {
    fields
        .next()
        .ok_or_else(|| format!("{kind} command is missing {label}"))
}

fn ensure_no_more<'a>(mut fields: impl Iterator<Item = &'a str>, kind: &str) -> Result<(), String> {
    if fields.next().is_some() {
        Err(format!("{kind} command has too many fields"))
    } else {
        Ok(())
    }
}

fn parse_u64(input: &str, label: &str) -> Result<u64, String> {
    input
        .parse()
        .map_err(|_| format!("invalid {label} `{input}`"))
}

fn parse_i64(input: &str, label: &str) -> Result<i64, String> {
    input
        .parse()
        .map_err(|_| format!("invalid {label} `{input}`"))
}

fn cycle_to_u64(cycle: i64) -> u64 {
    cycle.max(0) as u64
}
