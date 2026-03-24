use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("input is empty")]
    EmptyInput,

    #[error("line {line}: {message}")]
    Line { line: usize, message: String },

    #[error("{0}")]
    Validation(#[from] ValidationError),
}

impl ParseError {
    pub(crate) fn line(line: usize, message: impl Into<String>) -> Self {
        Self::Line {
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("missing PLOG header")]
    MissingHeader,

    #[error("duplicate PLOG header")]
    DuplicateHeader,

    #[error("unsupported PLog version {0}")]
    UnsupportedVersion(u32),

    #[error("duplicate stage id `{0}`")]
    DuplicateStage(String),

    #[error("duplicate lane id `{0}`")]
    DuplicateLane(String),

    #[error("duplicate instruction id {0}")]
    DuplicateInstruction(u64),

    #[error("span for instruction {inst_id} has zero duration at cycle {cycle}")]
    ZeroDuration { cycle: u64, inst_id: u64 },

    #[error("span for instruction {inst_id} overflows cycle range at cycle {cycle}")]
    SpanCycleOverflow { cycle: u64, inst_id: u64 },

    #[error("span references unknown instruction {0}")]
    UnknownInstruction(u64),

    #[error("span references unknown lane `{0}`")]
    UnknownLane(String),

    #[error("span references unknown stage `{0}`")]
    UnknownStage(String),
}
