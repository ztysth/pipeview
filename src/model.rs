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

    pub(crate) fn as_ptr(&self) -> *const String {
        Arc::as_ptr(&self.0)
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

const SMALL_NAME_SET: usize = 32;

/// Traces use a handful of stage and lane names, so a linear scan beats
/// hashing every span; the set only falls back to hashing past that.
#[derive(Default)]
pub(crate) struct SpanNames {
    small: Vec<SpanName>,
    large: HashSet<SpanName>,
}

impl SpanNames {
    pub(crate) fn intern(&mut self, name: &str) -> SpanName {
        if let Some(existing) = self.small.iter().find(|existing| existing.as_str() == name) {
            return existing.clone();
        }
        if let Some(existing) = self.large.get(name) {
            return existing.clone();
        }
        let interned = SpanName::from(name);
        if self.small.len() < SMALL_NAME_SET {
            self.small.push(interned.clone());
        } else {
            self.large.insert(interned.clone());
        }
        interned
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SpanName> {
        self.small.iter().chain(&self.large)
    }
}

/// Maps instruction ids to their rank in ascending id order. Traces usually
/// number instructions densely, which makes lookups a subtraction; anything
/// else falls back to a sorted table and binary search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionOrder(Order);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Order {
    Dense { base: u64, len: usize },
    Sorted(Vec<(u64, usize)>),
}

impl InstructionOrder {
    pub fn new(instructions: &[Instruction]) -> Self {
        let base = instructions
            .first()
            .map_or(0, |instruction| instruction.inst_id);
        let dense = instructions
            .iter()
            .enumerate()
            .all(|(index, instruction)| instruction.inst_id.wrapping_sub(base) == index as u64);
        if dense {
            return Self(Order::Dense {
                base,
                len: instructions.len(),
            });
        }

        let mut sorted = instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| (instruction.inst_id, index))
            .collect::<Vec<_>>();
        sorted.sort_by_key(|&(inst_id, _)| inst_id);
        Self(Order::Sorted(sorted))
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Order::Dense { len, .. } => *len,
            Order::Sorted(sorted) => sorted.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The rank of `inst_id` in ascending id order.
    #[inline]
    pub fn rank(&self, inst_id: u64) -> Option<usize> {
        match &self.0 {
            Order::Dense { base, len } => {
                let rank = inst_id.checked_sub(*base)?;
                (rank < *len as u64).then_some(rank as usize)
            }
            Order::Sorted(sorted) => sorted
                .binary_search_by_key(&inst_id, |&(inst_id, _)| inst_id)
                .ok(),
        }
    }

    /// The index into `Trace::instructions` of the instruction at `rank`.
    #[inline]
    pub fn instruction_index(&self, rank: usize) -> usize {
        match &self.0 {
            Order::Dense { .. } => rank,
            Order::Sorted(sorted) => sorted[rank].1,
        }
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
