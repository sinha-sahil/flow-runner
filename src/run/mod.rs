//! Running a flow: one call to [`Flow::run`] enters a node, follows jumps,
//! and stops at a value, an ask, or nothing to do. Everything the instance
//! needs to continue later is in the returned [`Token`].

mod token;
mod walk;

pub use token::{Token, TokenInfo};

use self::walk::Run;
use crate::common::error::{InputError, RunError};
use crate::common::limits::Limits;
use crate::flow::Flow;
use indexmap::IndexMap;
use std::fmt;
use temple_dsl::Value;

/// What the host asks a flow to do. Build one with [`Input::start`] or
/// [`Input::resume`], then add to it. A mistake, such as a param on a resume,
/// is reported by [`Flow::run`], not dropped.
#[derive(Debug, Clone)]
pub struct Input {
    begin: Begin,
    event: Option<Event>,
    data: Value,
    params: IndexMap<String, Value>,
    result: Option<Value>,
    failure: Option<Value>,
}

/// Where a run begins.
#[derive(Debug, Clone)]
enum Begin {
    Entry(String),
    Token(Token),
}

impl Input {
    /// Start a new instance at `entry`, with `data` as `input.data`.
    pub fn start(entry: impl Into<String>, data: impl Into<Value>) -> Input {
        Input::new(Begin::Entry(entry.into()), data.into())
    }

    /// Continue the instance `token` belongs to, with `data` as `input.data`.
    pub fn resume(token: Token, data: impl Into<Value>) -> Input {
        Input::new(Begin::Token(token), data.into())
    }

    fn new(begin: Begin, data: Value) -> Input {
        Input {
            begin,
            event: None,
            data,
            params: IndexMap::new(),
            result: None,
            failure: None,
        }
    }

    /// Deliver an event: a declared one, or `tick`. Expressions read its
    /// name as `input.event` and `payload` as `input.payload`.
    pub fn event(mut self, name: impl Into<String>, payload: impl Into<Value>) -> Input {
        self.event = Some(Event {
            name: name.into(),
            payload: payload.into(),
        });
        self
    }

    /// Set a param. Only a start takes params.
    pub fn param(mut self, name: impl Into<String>, value: impl Into<Value>) -> Input {
        self.params.insert(name.into(), value.into());
        self
    }

    /// Answer the ask a resumed token waits on.
    pub fn result(mut self, value: impl Into<Value>) -> Input {
        self.result = Some(value.into());
        self
    }

    /// Say why the ask a resumed token waits on couldn't be answered. The
    /// reason can't be `null`, which reads as no failure.
    pub fn failure(mut self, reason: impl Into<Value>) -> Input {
        self.failure = Some(reason.into());
        self
    }

    /// Whether it answers an ask, with a result or a failure.
    fn answers(&self) -> bool {
        self.result.is_some() || self.failure.is_some()
    }
}

/// Something that happened, delivered to the node an instance is parked at.
#[derive(Debug, Clone)]
struct Event {
    /// A declared event, or `tick`.
    name: String,
    /// What came with it, as `input.payload`.
    payload: Value,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Decision {
    /// The instance is parked at `node`, with a value for the host.
    Value {
        /// What the flow decided.
        value: Value,
        /// The node the instance is parked at.
        node: String,
        /// The token to store and resume with.
        token: Token,
        /// When to send a `tick` event, in epoch seconds.
        wake_at: Option<i64>,
    },
    /// The flow needs the host to do something: run the ask, then resume the
    /// token with a `result` or a `failure`.
    Ask {
        /// The ask's name.
        name: String,
        /// What to ask with.
        payload: Value,
        /// The node the run continues at with the answer.
        node: String,
        /// The token to store and resume with the answer.
        token: Token,
    },
    /// The run reached `ignore`; nothing changed. Keep the token you had.
    Ignored,
    /// The run failed. Nothing was performed; fix the data or the flow and
    /// retry with `token`, or start again when it is `None`.
    Error {
        /// What failed.
        error: RunError,
        /// The token the run resumed; `None` for a start.
        token: Option<Token>,
    },
}

impl Decision {
    /// The token to store, if the decision has one.
    pub fn token(&self) -> Option<&Token> {
        match self {
            Decision::Value { token, .. } | Decision::Ask { token, .. } => Some(token),
            Decision::Error { token, .. } => token.as_ref(),
            Decision::Ignored => None,
        }
    }

    /// Whether the run failed.
    pub fn is_error(&self) -> bool {
        matches!(self, Decision::Error { .. })
    }
}

/// Where a run went, for answering "why did this happen?". Holds node names
/// and branch positions only, never values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    /// The branches taken, in order.
    pub branches: Vec<BranchTaken>,
    /// Evaluation steps the run used.
    pub steps_used: u64,
}

/// One branch a run took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTaken {
    /// The node the branch is in.
    pub node: String,
    /// Index of the branch taken, counting from 0 in source order.
    pub branch: usize,
    /// What the branch did.
    pub kind: BranchKind,
}

/// What a branch did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BranchKind {
    /// Jumped to another node.
    Jump,
    /// Ended the run with a value.
    Value,
    /// Ended the run with an ask.
    Ask,
    /// Ended the run with nothing to do.
    Ignore,
    /// An expression in the branch failed.
    Error,
}

impl BranchKind {
    /// The kind as a word: `jump`, `value`, `ask`, `ignore` or `error`.
    pub fn as_str(self) -> &'static str {
        match self {
            BranchKind::Jump => "jump",
            BranchKind::Value => "value",
            BranchKind::Ask => "ask",
            BranchKind::Ignore => "ignore",
            BranchKind::Error => "error",
        }
    }
}

impl fmt::Display for BranchKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Flow {
    /// Run the flow once. `Err` means the host passed something wrong and
    /// nothing ran; every other ending is a [`Decision`].
    pub fn run(&self, input: Input, limits: &Limits) -> Result<Decision, InputError> {
        Ok(Run::begin(self, input, limits, None)?.walk())
    }

    /// Like [`run`](Self::run), also returning the path the run took.
    pub fn run_traced(
        &self,
        input: Input,
        limits: &Limits,
    ) -> Result<(Decision, Trace), InputError> {
        let mut trace = Trace::default();
        let decision = Run::begin(self, input, limits, Some(&mut trace))?.walk();
        Ok((decision, trace))
    }
}
