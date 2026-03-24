pub type AttrMap = Vec<KeyValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub version: u32,
    pub meta: Vec<KeyValue>,
    pub stages: Vec<Stage>,
    pub lanes: Vec<Lane>,
    pub instructions: Vec<Instruction>,
    pub spans: Vec<Span>,
    pub events: Vec<Event>,
    pub counters: Vec<Counter>,
    pub retires: Vec<RetireEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub id: String,
    pub label: String,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub id: String,
    pub label: String,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub inst_id: u64,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub cycle: u64,
    pub duration: u64,
    pub inst_id: u64,
    pub lane: String,
    pub stage: String,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub cycle: u64,
    pub inst_id: u64,
    pub event: String,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counter {
    pub cycle: u64,
    pub resource: String,
    pub attrs: AttrMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireEvent {
    pub cycle: u64,
    pub inst_id: u64,
    pub status: String,
    pub attrs: AttrMap,
}
