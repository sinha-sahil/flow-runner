//! A compiled flow: its node table, and what a host can read from it.

use crate::common::diagnostic::Diagnostic;
use crate::syntax::lexer::Mode;
use indexmap::IndexSet;
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use temple_dsl::{ExprUnit, Functions, InputRead, Span, TemplateUnit, Value};

/// Bytes in a flow hash.
pub(crate) const HASH_LEN: usize = 32;

/// A compiled flow. Compile it once with [`Flow::compile`], store it with
/// [`Flow::to_bytes`] and load it with [`Flow::from_bytes`]. It is immutable
/// and can be shared between threads.
#[derive(Debug, Clone)]
pub struct Flow {
    pub(crate) name: SmolStr,
    pub(crate) edition: u32,
    pub(crate) annotations: Vec<Annotation>,
    pub(crate) entries: Vec<NodeId>,
    pub(crate) events: IndexSet<SmolStr>,
    pub(crate) params: Vec<Param>,
    pub(crate) lets: Vec<Let>,
    pub(crate) helpers: Vec<Helper>,
    pub(crate) functions: Functions,
    pub(crate) nodes: Vec<Node>,
    pub(crate) node_ids: HashMap<SmolStr, NodeId>,
    /// Where each param sits in `params`.
    pub(crate) param_positions: HashMap<SmolStr, usize>,
    /// Every ask name, first use first.
    pub(crate) asks: IndexSet<SmolStr>,
    pub(crate) hash: [u8; HASH_LEN],
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) source: Arc<str>,
}

/// Where a node sits in the flow's node table. Jumps and asks refer to
/// nodes by id, so a run never looks a name up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NodeId(pub(crate) u32);

impl NodeId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// `@name = value` on a flow, a param or a node, with its value evaluated.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// The name after `@`.
    pub name: String,
    /// The value; `true` for a bare `@name`.
    pub value: Value,
}

/// A flow parameter: a constant the host may set when an instance starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The param's name.
    pub name: String,
    /// The value an instance gets when the host doesn't set one.
    pub default: Value,
    /// The annotations on the `param` declaration.
    pub annotations: Vec<Annotation>,
}

/// A `let name = expression`, at flow or node level, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Let {
    pub name: SmolStr,
    pub expression: ExprUnit,
}

/// A helper, `let name(a, b) = expression` at flow level, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Helper {
    pub name: SmolStr,
    pub params: Vec<SmolStr>,
    pub body: ExprUnit,
}

/// A node, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub name: SmolStr,
    pub params: Vec<SmolStr>,
    pub annotations: Vec<Annotation>,
    pub lets: Vec<Let>,
    pub branches: Vec<Branch>,
    /// True when some `ask` continues here.
    pub resumes_ask: bool,
    pub is_entry: bool,
    pub span: Span,
}

/// One branch of a node, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Branch {
    /// `None` is `else`, or a node's only outcome.
    pub guard: Option<ExprUnit>,
    pub outcome: Outcome,
    /// The guard holds only while an event is delivered, so the branch
    /// can't be taken again after a jump.
    pub needs_event: bool,
    pub span: Span,
}

/// What a branch does, compiled: nodes by id, units ready to run.
#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    Jump {
        node: NodeId,
        args: Vec<ExprUnit>,
    },
    Value {
        template: TemplateUnit,
        wake: Option<ExprUnit>,
    },
    Ask {
        name: SmolStr,
        payload: Option<ExprUnit>,
        node: NodeId,
        args: Vec<ExprUnit>,
    },
    Ignore,
}

impl Outcome {
    /// The outcome's expressions and values, in source order.
    fn units(&self) -> impl Iterator<Item = UnitRef<'_>> {
        let (payload, template, args, wake) = match self {
            Outcome::Jump { args, .. } => (None, None, args.as_slice(), None),
            Outcome::Value { template, wake } => {
                (None, Some(template), [].as_slice(), wake.as_ref())
            }
            Outcome::Ask { payload, args, .. } => (payload.as_ref(), None, args.as_slice(), None),
            Outcome::Ignore => (None, None, [].as_slice(), None),
        };
        payload
            .map(UnitRef::Expression)
            .into_iter()
            .chain(template.map(UnitRef::Template))
            .chain(args.iter().map(UnitRef::Expression))
            .chain(wake.map(UnitRef::Expression))
    }
}

impl Node {
    fn units(&self) -> impl Iterator<Item = UnitRef<'_>> {
        let lets = self
            .lets
            .iter()
            .map(|binding| UnitRef::Expression(&binding.expression));
        let branches = self.branches.iter().flat_map(|branch| {
            let guard = branch.guard.iter().map(UnitRef::Expression);
            guard.chain(branch.outcome.units())
        });
        lets.chain(branches)
    }

    /// Where each of the node's expressions and values is in the source,
    /// and how it reads.
    pub(crate) fn unit_spans(&self) -> impl Iterator<Item = (Span, Mode)> + '_ {
        self.units().map(|unit| match unit {
            UnitRef::Expression(unit) => (unit.span(), Mode::Expression),
            UnitRef::Template(unit) => (unit.span(), Mode::Output),
        })
    }

    pub(crate) fn reads(&self) -> Vec<InputRead> {
        self.units().flat_map(|unit| unit.reads()).collect()
    }
}

/// Either kind of compiled unit, for questions both can answer.
enum UnitRef<'a> {
    Expression(&'a ExprUnit),
    Template(&'a TemplateUnit),
}

impl UnitRef<'_> {
    fn reads(&self) -> Vec<InputRead> {
        match self {
            UnitRef::Expression(unit) => unit.reads(),
            UnitRef::Template(unit) => unit.reads(),
        }
    }
}

/// What a builder or host can read about one node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo<'a> {
    /// The node's name.
    pub name: &'a str,
    /// The node's parameter names, in order.
    pub params: Vec<&'a str>,
    /// The annotations on the `node` declaration.
    pub annotations: &'a [Annotation],
    /// True when the flow lists the node under `entry`.
    pub is_entry: bool,
    /// True when an `ask` continues here.
    pub resumes_ask: bool,
    /// Nodes this node can jump to, in branch order, without repeats.
    pub jumps_to: Vec<&'a str>,
    /// Each ask in this node, in branch order.
    pub asks: Vec<AskInfo<'a>>,
    /// Number of branches, counting `else` or the single outcome.
    pub branch_count: usize,
}

/// One `ask name(payload) -> node(args)` in a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskInfo<'a> {
    /// The ask's name, for the host to resolve.
    pub name: &'a str,
    /// The node the run continues at with the answer.
    pub node: &'a str,
}

impl Flow {
    /// The name after `flow`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The language edition the flow is written in.
    pub fn edition(&self) -> u32 {
        self.edition
    }

    /// The flow's version as hex. It changes when what the flow does
    /// changes, and not when only its layout, comments or annotations do.
    pub fn hash(&self) -> String {
        hex(&self.hash)
    }

    /// The nodes a new run may start at.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|&id| self.node_name(id))
    }

    /// Declared events. `tick` is always accepted too.
    pub fn events(&self) -> impl Iterator<Item = &str> {
        self.events.iter().map(SmolStr::as_str)
    }

    /// Params in declaration order, with their defaults.
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Every ask name the flow can produce, e.g. to check that the host
    /// resolves each of them before deploying.
    pub fn asks(&self) -> impl Iterator<Item = &str> {
        self.asks.iter().map(SmolStr::as_str)
    }

    /// The annotations on the `flow` declaration.
    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    /// Problems that didn't stop the flow compiling.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// Every node, in source order.
    pub fn nodes(&self) -> impl Iterator<Item = NodeInfo<'_>> {
        self.nodes.iter().map(|node| self.info(node))
    }

    /// The node called `name`.
    pub fn node(&self, name: &str) -> Option<NodeInfo<'_>> {
        self.node_by_name(name)
            .map(|id| self.info(&self.nodes[id.index()]))
    }

    fn info<'a>(&'a self, node: &'a Node) -> NodeInfo<'a> {
        let mut jumps_to = Vec::new();
        let mut seen = HashSet::new();
        let mut asks = Vec::new();
        for branch in &node.branches {
            match &branch.outcome {
                Outcome::Jump { node: target, .. } => {
                    if seen.insert(*target) {
                        jumps_to.push(self.node_name(*target));
                    }
                }
                Outcome::Ask {
                    name, node: target, ..
                } => asks.push(AskInfo {
                    name: name.as_str(),
                    node: self.node_name(*target),
                }),
                _ => {}
            }
        }
        NodeInfo {
            name: &node.name,
            params: node.params.iter().map(SmolStr::as_str).collect(),
            annotations: &node.annotations,
            is_entry: node.is_entry,
            resumes_ask: node.resumes_ask,
            jumps_to,
            asks,
            branch_count: node.branches.len(),
        }
    }

    pub(crate) fn node_by_name(&self, name: &str) -> Option<NodeId> {
        self.node_ids.get(name).copied()
    }

    pub(crate) fn node_name(&self, id: NodeId) -> &str {
        &self.nodes[id.index()].name
    }

    /// The source the flow was compiled from, for reports such as
    /// [`RunError::report`](crate::RunError::report).
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Bytes as lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 * bytes.len());
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    out
}
