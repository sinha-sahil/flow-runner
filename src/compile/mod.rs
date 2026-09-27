//! From a parsed file to a runnable [`Flow`]: resolve names, check every
//! expression and value against the names in scope, and enforce the rules
//! that keep a run finite and predictable.
//!
//! One [`Compiler`] walks the tree; its methods are split by concern:
//! [`declarations`] reads the top-level declarations, [`nodes`] each node,
//! [`units`] each temple unit, [`events`] how units test `input.event`, and
//! [`rules`] the checks that need every node compiled. [`hash`] is the
//! flow's version.

mod declarations;
mod events;
mod hash;
mod hints;
pub(crate) mod literal;
mod nodes;
mod rules;
mod scope;
mod units;

use crate::common::diagnostic::{Diagnostic, Diagnostics};
use crate::common::error::too_large;
use crate::common::limits::{MAX_HINTS, MAX_HINT_CELLS, MAX_SOURCE_BYTES};
use crate::flow::{Flow, Node, NodeId, HASH_LEN};
use crate::syntax;
use crate::syntax::ast::{FlowAst, EDITIONS};
use crate::syntax::lexer::Mode;
use scope::Scope;
use smol_str::SmolStr;
use std::collections::HashMap;
use std::sync::Arc;
use temple_dsl::Span;

impl Flow {
    /// Compile a `.flow` source. On success the flow may still carry
    /// [`warnings`](Self::warnings).
    pub fn compile(src: &str) -> Result<Flow, Diagnostics> {
        if src.len() > MAX_SOURCE_BYTES {
            return Err(Diagnostics::new(vec![too_large_source(src)]));
        }
        let (ast, diagnostics) = syntax::parse(src);
        parsed(src, &ast, diagnostics)
    }

    /// Every error and warning in a source, without keeping the result.
    pub fn check(src: &str) -> Diagnostics {
        diagnostics(Flow::compile(src))
    }
}

/// The error for a source over [`MAX_SOURCE_BYTES`].
pub(crate) fn too_large_source(src: &str) -> Diagnostic {
    Diagnostic::error(
        Span::new(0, 0),
        too_large("the flow", src.len(), MAX_SOURCE_BYTES),
    )
}

/// Compile an already parsed file. `diagnostics` are the parser's.
pub(crate) fn parsed(
    src: &str,
    ast: &FlowAst,
    mut diagnostics: Vec<Diagnostic>,
) -> Result<Flow, Diagnostics> {
    if diagnostics.iter().any(Diagnostic::is_error) {
        return Err(Diagnostics::new(diagnostics));
    }
    let mut compiler = Compiler {
        src,
        ast,
        diagnostics: Vec::new(),
        hints_left: MAX_HINTS,
        hint_cells_left: MAX_HINT_CELLS,
    };
    let flow = compiler.flow();
    diagnostics.append(&mut compiler.diagnostics);
    match flow {
        Some(mut flow) if !diagnostics.iter().any(Diagnostic::is_error) => {
            flow.warnings = diagnostics;
            flow.source = Arc::from(src);
            Ok(flow)
        }
        _ => Err(Diagnostics::new(diagnostics)),
    }
}

/// The diagnostics a compile produced, whether it succeeded or not.
pub(crate) fn diagnostics(result: Result<Flow, Diagnostics>) -> Diagnostics {
    match result {
        Ok(flow) => Diagnostics::new(flow.warnings),
        Err(errors) => errors,
    }
}

/// What a name in a flow can be declared as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Param,
    Let,
    Helper,
    Node,
    Parameter,
    NodeLet,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Kind::Param => "a param",
            Kind::Let => "a let",
            Kind::Helper => "a helper",
            Kind::Node => "a node",
            Kind::Parameter => "a parameter",
            Kind::NodeLet => "a node let",
        }
    }
}

/// Every declared name in one namespace, and what each is.
type Declarations = HashMap<SmolStr, Kind>;

struct Compiler<'a> {
    src: &'a str,
    ast: &'a FlowAst,
    diagnostics: Vec<Diagnostic>,
    /// Did-you-mean searches left, and the table cells they may still fill.
    hints_left: usize,
    hint_cells_left: u64,
}

impl Compiler<'_> {
    fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.push(Diagnostic::error(span, message));
    }

    /// Whether any diagnostic from index `from` on is an error.
    fn errors_since(&self, from: usize) -> bool {
        self.diagnostics[from..].iter().any(Diagnostic::is_error)
    }

    /// Whether `searches` did-you-mean searches, filling at most `cells()`
    /// table cells, still fit. The searches are used up even when the cells
    /// don't fit, so working out what they cost stays bounded too.
    fn afford_hints(&mut self, searches: usize, cells: impl FnOnce() -> u64) -> bool {
        if searches > self.hints_left {
            return false;
        }
        self.hints_left -= searches;
        let cells = cells();
        if cells > self.hint_cells_left {
            return false;
        }
        self.hint_cells_left -= cells;
        true
    }

    /// The closest name to `target`, while hints last.
    fn hint<'n>(
        &mut self,
        target: &str,
        candidates: impl Iterator<Item = &'n str> + Clone,
    ) -> Option<&'n str> {
        if self.afford_hints(1, || hints::closest_cost(target, candidates.clone())) {
            hints::closest(target, candidates)
        } else {
            None
        }
    }

    fn flow(&mut self) -> Option<Flow> {
        let ast = self.ast;
        let name = match &ast.name {
            Some(name) => name.name.clone(),
            None => {
                self.error(Span::new(0, 0), "missing `flow <name>` declaration");
                SmolStr::default()
            }
        };
        let edition = ast.edition.unwrap_or(EDITIONS[0]);
        let globals = self.declare_globals();
        let params = self.params();
        let mut scope = Scope::default();
        let (helpers, functions) = self.helpers(&globals, &mut scope);
        let lets = self.flow_lets(&mut scope);

        // a source within MAX_SOURCE_BYTES holds far fewer than u32::MAX nodes
        let node_ids: HashMap<SmolStr, NodeId> = ast
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.name.name.clone(), NodeId(index as u32)))
            .collect();
        let mut nodes = Vec::new();
        for node in &ast.nodes {
            if let Some(compiled) = self.node(node, &globals, &mut scope, &node_ids) {
                nodes.push(compiled);
            }
        }
        let entries = self.check_entries(&node_ids);
        let events = self.check_events();
        let annotations = self.annotations(&ast.annotations);
        if nodes.len() != ast.nodes.len() || node_ids.len() != nodes.len() {
            // the rules below need every node compiled, under a name of its own
            return None;
        }

        for &entry in &entries {
            nodes[entry.index()].is_entry = true;
        }
        let shared = lets
            .iter()
            .map(|binding| &binding.expression)
            .chain(helpers.iter().map(|helper| &helper.body));
        let unit_spans = nodes
            .iter()
            .flat_map(Node::unit_spans)
            .chain(shared.clone().map(|unit| (unit.span(), Mode::Expression)));
        self.check_event_tests(unit_spans, &events);
        self.check_asks(&mut nodes, shared);
        self.check_jump_loops(&nodes);
        // with an entry missing, what can be reached isn't known
        let entries_known = ast
            .entry_names()
            .all(|entry| node_ids.contains_key(&entry.name));
        if !entries.is_empty() && entries_known {
            self.check_reachability(&nodes, &entries);
        }
        if self.errors_since(0) {
            return None;
        }

        let mut flow = Flow {
            name,
            edition,
            annotations,
            entries,
            events,
            param_positions: params
                .iter()
                .enumerate()
                .map(|(position, param)| (SmolStr::new(&param.name), position))
                .collect(),
            params,
            lets,
            helpers,
            functions: functions?,
            asks: nodes::ask_names(&nodes),
            nodes,
            node_ids,
            hash: [0; HASH_LEN],
            warnings: Vec::new(),
            source: Arc::from(""),
        };
        flow.hash = hash::of(&flow, self.src);
        Some(flow)
    }
}
