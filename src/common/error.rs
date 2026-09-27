//! Every error a host can get back, and the wording they share. Code in a
//! message is in backticks.

use crate::common::diagnostic::{Diagnostic, Diagnostics};
use crate::common::limits::{MAX_LISTED_NAMES, MAX_TOKEN_STATE, MAX_VALUE_DEPTH};
use std::fmt;
use temple_dsl::{EvalError, Span, Value};

/// Why a stored flow couldn't be loaded.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum LoadError {
    /// The bytes are damaged, or aren't a flow blob.
    Corrupt(String),
    /// Saved with another blob layout. Compile the flow's source again.
    IncompatibleVersion {
        /// The blob's version.
        found: u32,
        /// [`BLOB_VERSION`](crate::BLOB_VERSION).
        expected: u32,
    },
    /// The source compiles, but to another hash than the one saved with it:
    /// this flow-runner reads the flow differently from the one that saved
    /// it. Tokens made before won't run until
    /// [rebound](crate::Flow::rebind); compile the source again to accept
    /// the new hash.
    HashChanged,
    /// The source no longer compiles, e.g. it was saved by a newer
    /// flow-runner that knows an edition this one doesn't.
    Invalid(Diagnostics),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Corrupt(why) => write!(f, "corrupt flow blob: {why}"),
            LoadError::IncompatibleVersion { found, expected } => write!(
                f,
                "flow blob version {found} is not {expected}; compile the source again"
            ),
            LoadError::HashChanged => write!(
                f,
                "the flow now has another hash; tokens made before need rebinding"
            ),
            LoadError::Invalid(errors) => write!(f, "stored flow no longer compiles: {errors}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl LoadError {
    /// Damaged bytes, and why they can't be read.
    pub(crate) fn corrupt(why: impl Into<String>) -> Self {
        LoadError::Corrupt(why.into())
    }
}

/// Why a run failed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RunError {
    /// An expression failed or ran out of budget.
    Expression {
        /// The node it is in; `None` for a flow-level let.
        node: Option<String>,
        /// What temple reported.
        error: ExpressionError,
    },
    /// A guard produced something other than true or false.
    GuardNotBool {
        /// The node the guard is in.
        node: String,
        /// What it produced.
        got: String,
        /// Where the guard is in the source.
        span: Span,
    },
    /// `wake` produced something other than a whole number of epoch seconds.
    WakeNotTime {
        /// The node the `wake` is in.
        node: String,
        /// What it produced.
        got: String,
        /// Where the `wake` expression is in the source.
        span: Span,
    },
    /// More jumps in one run than [`Limits::jumps`](crate::Limits::jumps).
    TooManyJumps {
        /// The limit.
        limit: u64,
        /// The node that would have jumped once too often.
        node: String,
    },
    /// More asks in one instance than [`Limits::asks`](crate::Limits::asks).
    TooManyAsks {
        /// The limit.
        limit: u64,
        /// The node that would have asked once too often.
        node: String,
    },
    /// A value too deeply nested to store in a token.
    ValueTooDeep {
        /// The node that would have stored it.
        node: String,
    },
    /// The node arguments and params a token would carry are over `limit`
    /// bytes. Keep large data in the host and pass it in `input.data`.
    TokenTooLarge {
        /// The node that would have made the token.
        node: String,
        /// The limit, in bytes.
        limit: u64,
    },
    /// A run that answered an ask reached `ignore`. An answer has to lead to
    /// a value or another ask; otherwise it would be lost.
    AnswerIgnored {
        /// The node with the `ignore`.
        node: String,
    },
    /// A run jumped, then reached `ignore`. A jump has to lead to a value or
    /// an ask; otherwise the event that caused it would be lost.
    JumpIgnored {
        /// The node with the `ignore`.
        node: String,
    },
}

/// A temple evaluation error, flattened so [`RunError`] can compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionError {
    /// What went wrong, as temple words it.
    pub message: String,
    /// Where in the flow's source, when temple knows.
    pub span: Option<Span>,
    /// The run used up [`Limits::steps`](crate::Limits::steps).
    pub budget_exceeded: bool,
}

impl From<EvalError> for ExpressionError {
    fn from(error: EvalError) -> Self {
        ExpressionError {
            message: error.to_string(),
            span: error.span(),
            budget_exceeded: error.is_budget_exceeded(),
        }
    }
}

impl RunError {
    /// Where in the flow's source the run failed, when that is one place.
    pub fn span(&self) -> Option<Span> {
        match self {
            RunError::Expression { error, .. } => error.span,
            RunError::GuardNotBool { span, .. } | RunError::WakeNotTime { span, .. } => Some(*span),
            _ => None,
        }
    }

    /// Whether the run used up [`Limits::steps`](crate::Limits::steps).
    pub fn is_budget_exceeded(&self) -> bool {
        matches!(self, RunError::Expression { error, .. } if error.budget_exceeded)
    }

    /// An underlined snippet against the flow's source.
    pub fn report(&self, src: &str) -> String {
        match self.span() {
            Some(span) => Diagnostic::error(span, self.to_string()).report(src),
            None => format!("error: {self}"),
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Expression {
                node: Some(node),
                error,
            } => write!(f, "in node `{node}`: {}", error.message),
            RunError::Expression { node: None, error } => {
                write!(f, "in a flow-level let: {}", error.message)
            }
            RunError::GuardNotBool { node, got, .. } => write!(
                f,
                "in node `{node}`: a guard must be true or false, got {got}"
            ),
            RunError::WakeNotTime { node, got, .. } => write!(
                f,
                "in node `{node}`: `wake` must be a whole number of epoch seconds, got {got}"
            ),
            RunError::TooManyJumps { limit, node } => {
                write!(f, "in node `{node}`: more than {limit} jumps in one run")
            }
            RunError::TooManyAsks { limit, node } => write!(
                f,
                "in node `{node}`: the instance would ask more than {limit} times"
            ),
            RunError::ValueTooDeep { node } => write!(
                f,
                "in node `{node}`: {}",
                too_deep("a value the token would store", MAX_VALUE_DEPTH)
            ),
            RunError::TokenTooLarge { node, limit } => write!(
                f,
                "in node `{node}`: the state the token would carry is over {limit} bytes; keep large data in the host and pass it in `input.data`"
            ),
            RunError::AnswerIgnored { node } => write!(
                f,
                "in node `{node}`: the answer to an ask reached `ignore`, so it would be lost; after an ask, every path must end in a value or another ask"
            ),
            RunError::JumpIgnored { node } => write!(
                f,
                "in node `{node}`: the run jumped here and reached `ignore`, so the event would be lost; after a jump, every path must end in a value or an ask"
            ),
        }
    }
}

impl std::error::Error for RunError {}

/// A mistake by the host: the run didn't start, and nothing was evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputError {
    /// No entry node has this name.
    UnknownEntry(String),
    /// The flow doesn't declare this event, and it isn't `tick`.
    UnknownEvent(String),
    /// The flow has no param with this name.
    UnknownParam(String),
    /// A param was given a different kind of value than its default.
    ParamType {
        /// The param.
        name: String,
        /// The kind of its default, such as "a number".
        expected: String,
        /// The kind given.
        got: String,
    },
    /// A param's value nests too deeply to store in a token.
    ParamTooDeep(String),
    /// A param was given to a resume. Params are set when an instance
    /// starts, and its token carries them from then on.
    ParamOnResume(String),
    /// The token is damaged, or isn't a flow token.
    CorruptToken(String),
    /// The token was made by a newer flow-runner than this one.
    NewerToken,
    /// The token belongs to another flow, named here.
    OtherFlow(String),
    /// The token was made by another version of this flow. Load the version
    /// that made it, or [`Flow::rebind`](crate::Flow::rebind) it.
    OtherVersion,
    /// The token waits on an ask; resume it with a `result` or a `failure`.
    AnswerMissing,
    /// A `result` or `failure` was given to a start, or to a token that
    /// isn't waiting on an ask.
    AnswerUnexpected,
    /// Both a `result` and a `failure` were given.
    AnswerAmbiguous,
    /// The `failure` given is `null`, which reads as no failure at all.
    NullFailure,
    /// An event was given to a token that waits on an ask.
    EventDuringAsk,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::UnknownEntry(entry) => write!(f, "`{entry}` is not an entry of this flow"),
            InputError::UnknownEvent(event) => {
                write!(f, "`{event}` is not a declared event of this flow")
            }
            InputError::UnknownParam(param) => write!(f, "`{param}` is not a param of this flow"),
            InputError::ParamType {
                name,
                expected,
                got,
            } => write!(f, "param `{name}` expects {expected}, got {got}"),
            InputError::ParamTooDeep(param) => {
                f.write_str(&too_deep(&format!("param `{param}`"), MAX_VALUE_DEPTH))
            }
            InputError::ParamOnResume(param) => write!(
                f,
                "param `{param}` was given to a resume; params are set when an instance starts"
            ),
            InputError::CorruptToken(why) => write!(f, "corrupt token: {why}"),
            InputError::NewerToken => {
                write!(f, "the token was made by a newer flow-runner than this one")
            }
            InputError::OtherFlow(flow) => write!(f, "the token belongs to flow `{flow}`"),
            InputError::OtherVersion => write!(
                f,
                "the token was made by another version of this flow; load the version that made it, or rebind it with `Flow::rebind`"
            ),
            InputError::AnswerMissing => write!(
                f,
                "the token is waiting on an ask; resume it with a `result` or a `failure`"
            ),
            InputError::AnswerUnexpected => write!(
                f,
                "a `result` or `failure` was given, but the run isn't answering an ask"
            ),
            InputError::AnswerAmbiguous => write!(f, "give a `result` or a `failure`, not both"),
            InputError::NullFailure => write!(
                f,
                "a `failure` can't be null, which reads as no failure; give a reason"
            ),
            InputError::EventDuringAsk => {
                write!(f, "the token is waiting on an ask; it can't take an event")
            }
        }
    }
}

impl std::error::Error for InputError {}

impl InputError {
    /// A damaged or hand-made token, and why it can't be read.
    pub(crate) fn corrupt(why: impl Into<String>) -> Self {
        InputError::CorruptToken(why.into())
    }
}

/// Why [`Flow::rebind`](crate::Flow::rebind) refused a token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RebindError {
    /// The token can't be used with this flow; the inner error says why.
    Token(InputError),
    /// The node the token points at no longer exists.
    NodeGone(String),
    /// The node takes another number of arguments, or changed whether it
    /// continues an ask.
    NodeChanged(String),
    /// A param the instance was started with no longer exists.
    ParamGone(String),
    /// With the params added since it was made, the token would carry more
    /// than a token may.
    TooLarge,
}

impl fmt::Display for RebindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RebindError::Token(error) => write!(f, "{error}"),
            RebindError::NodeGone(node) => write!(f, "node `{node}` no longer exists"),
            RebindError::NodeChanged(node) => write!(
                f,
                "node `{node}` changed shape; the token can't continue there"
            ),
            RebindError::ParamGone(param) => write!(f, "param `{param}` no longer exists"),
            RebindError::TooLarge => write!(
                f,
                "with the params added since it was made, the state the token would carry is over {MAX_TOKEN_STATE} bytes"
            ),
        }
    }
}

impl std::error::Error for RebindError {}

/// Why an edit was refused. The document is unchanged.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EditError {
    /// No node, branch, param, annotation, event or entry by that name or index.
    NotFound(String),
    /// The name is invalid, reserved or already taken.
    BadName(String),
    /// The text doesn't parse. Spans point into the text given: for
    /// `AddBranch`, into its guard when that doesn't parse, and otherwise
    /// into its outcome.
    Syntax(Diagnostics),
    /// The edit makes no sense here, such as a branch after `else`.
    Invalid(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NotFound(what) => write!(f, "no {what}"),
            EditError::BadName(why) | EditError::Invalid(why) => f.write_str(why),
            EditError::Syntax(diagnostics) => {
                let messages: Vec<&str> = diagnostics
                    .errors()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect();
                write!(f, "the edit doesn't parse: {}", messages.join("; "))
            }
        }
    }
}

impl std::error::Error for EditError {}

/// A branch after `else`.
pub(crate) const AFTER_ELSE: &str = "a branch after `else` never runs";

/// The rule a branch after `else` breaks.
pub(crate) const ELSE_LAST: &str = "`else` must be the last branch";

/// `tick` in `events`.
pub(crate) const TICK_DECLARED: &str = "`tick` is built in; it doesn't need declaring";

/// A name given twice where each must be unique: "parameter `x` appears
/// twice".
pub(crate) fn twice(what: &str, name: &str) -> String {
    format!("{what} `{name}` appears twice")
}

/// A name declared again: "`x` is already declared as a param".
pub(crate) fn already_declared(name: &str, as_what: &str) -> String {
    format!("`{name}` is already declared as {as_what}")
}

/// Something nested past a depth limit.
pub(crate) fn too_deep(what: &str, limit: usize) -> String {
    format!("{what} nests deeper than {limit} levels")
}

/// Something past a size limit.
pub(crate) fn too_large(what: &str, bytes: usize, limit: usize) -> String {
    format!("{what} is too large: {bytes} bytes exceeds the limit of {limit}")
}

/// Words as a list of choices for a message: "`a`, `b` or `c`".
pub(crate) fn listed(words: &[&str]) -> String {
    let quoted: Vec<String> = words.iter().map(|word| format!("`{word}`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => quoted.concat(),
    }
}

/// Names for a message, the first [`MAX_LISTED_NAMES`] of them: "`a`, `b`
/// and 3 more".
pub(crate) fn names_listed(names: &[&str]) -> String {
    let shown: Vec<String> = names
        .iter()
        .take(MAX_LISTED_NAMES)
        .map(|name| format!("`{name}`"))
        .collect();
    let shown = shown.join(", ");
    match names.len().saturating_sub(MAX_LISTED_NAMES) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// A value's kind as a phrase for messages: "a number", "a list".
pub(crate) fn kind_phrase(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Int(_) | Value::Decimal(_) => "a number",
        Value::Str(_) => "a string",
        Value::Arr(_) => "a list",
        Value::Obj(_) => "an object",
    }
}

/// A value for a message: a scalar with its value, anything else by kind.
pub(crate) fn describe(value: &Value) -> String {
    match value {
        Value::Bool(flag) => flag.to_string(),
        Value::Int(number) => format!("the number {number}"),
        Value::Decimal(number) => format!("the number {number}"),
        Value::Str(text) => format!("the string {text:?}"),
        Value::Null | Value::Arr(_) | Value::Obj(_) => kind_phrase(value).to_string(),
    }
}
