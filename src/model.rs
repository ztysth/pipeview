use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// An owned, shared stage or lane name. Equality and hashing use its text, so
/// names remain comparable across traces and survive dropping their parser.
/// A thin Arc<String> keeps each handle one pointer wide; Arc<str> is two.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanName(Arc<String>);

impl SpanName {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<&str> for SpanName {
    fn from(value: &str) -> Self {
        Self(Arc::new(value.to_owned()))
    }
}

impl From<String> for SpanName {
    fn from(value: String) -> Self {
        Self(Arc::new(value))
    }
}

impl Deref for SpanName {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for SpanName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<&str> for SpanName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for SpanName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

#[derive(Default)]
pub(crate) struct SpanNames(HashSet<SpanName>);

impl SpanNames {
    pub(crate) fn intern(&mut self, name: &str) -> SpanName {
        if let Some(existing) = self.0.get(name) {
            return existing.clone();
        }
        let name = SpanName::from(name);
        self.0.insert(name.clone());
        name
    }
}

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
    pub lane: SpanName,
    pub stage: SpanName,
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
