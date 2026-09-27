//! # flow-runner
//!
//! A small decision engine. You write rules in a `.flow` file; the host passes
//! in data and gets back a decision. The engine never does I/O, never reads the
//! clock, and never performs actions: it decides, and the host acts.
//!
//! ```
//! use flow_runner::{Decision, Flow, Input, Limits, Value};
//!
//! let flow = Flow::compile(r#"
//! edition 2026
//! flow return-window
//! entry evaluate
//!
//! param windowDays = 30
//!
//! node evaluate =
//!   let age = input.data.evaluatedAt - input.data.deliveredAt
//!   when {
//!     age > windowDays * 86400: { "eligible": false, "reason": "WindowExpired" }
//!     else:                     { "eligible": true }
//!   }
//! "#).unwrap();
//!
//! let data = Value::obj([
//!     ("evaluatedAt", Value::Int(1_774_166_400)),
//!     ("deliveredAt", Value::Int(1_772_870_400)),
//! ]);
//! let decision = flow.run(Input::start("evaluate", data), &Limits::default()).unwrap();
//! let Decision::Value { value, .. } = decision else { panic!() };
//! assert_eq!(value, Value::obj([("eligible", Value::Bool(true))]));
//! ```
//!
//! Expressions inside a flow are [temple-dsl](https://docs.rs/temple-dsl).
//!
//! # How the pieces fit
//!
//! - [`Flow::compile`] parses and checks the text and produces a [`Flow`].
//!   Problems are [`Diagnostics`], each with a span into the source.
//! - [`Flow::run`] walks the flow for one [`Input`] and returns a
//!   [`Decision`], with a [`Token`] to continue from later.
//! - [`Flow::to_bytes`] and [`Flow::from_bytes`] store a flow.
//! - [`Document`] edits a flow as text, for a visual builder.
//!
//! # Features
//!
//! - `json`: conversions to and from `serde_json`, and [`Graph::to_json`].
//! - `cli`: the `flow` command-line tool. Implies `json`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod blob;
mod common;
mod compile;
mod document;
mod flow;
mod run;
mod syntax;

pub use common::diagnostic::{Diagnostic, Diagnostics, Severity};
pub use common::error::{EditError, ExpressionError, InputError, LoadError, RebindError, RunError};
pub use common::limits::{Limits, MAX_SOURCE_BYTES};
pub use common::versions::BLOB_VERSION;
pub use document::{
    AnnotationView, BranchView, Declaration, Document, Edge, EdgeKind, Edit, Graph, LetView,
    NodeView, OutcomeView, ParamView,
};
pub use flow::{Annotation, AskInfo, Flow, NodeInfo, Param};
pub use run::{BranchKind, BranchTaken, Decision, Input, Token, TokenInfo, Trace};
pub use temple_dsl::{Span, Value};
