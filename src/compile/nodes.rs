//! Compiling one node: its parameters and lets join the scope, then every
//! branch's guard and outcome is checked against those names.

use super::events::guard_needs_event;
use super::scope::Scope;
use super::{Compiler, Declarations, Kind};
use crate::common::diagnostic::Diagnostic;
use crate::common::error::{already_declared, AFTER_ELSE, ELSE_LAST};
use crate::flow::{Branch, Let, Node, NodeId, Outcome};
use crate::syntax::ast::{self, Body, Guard, Identifier, NodeDeclaration};
use indexmap::IndexSet;
use smol_str::SmolStr;
use std::collections::HashMap;
use temple_dsl::{ExprUnit, ParsedExpr};

impl Compiler<'_> {
    /// Compile a node. Its parameters and lets are in scope while its own
    /// expressions are checked, and leave it after.
    pub(super) fn node(
        &mut self,
        declaration: &NodeDeclaration,
        globals: &Declarations,
        scope: &mut Scope,
        node_ids: &HashMap<SmolStr, NodeId>,
    ) -> Option<Node> {
        let first_diagnostic = self.diagnostics.len();
        let annotations = self.annotations(&declaration.annotations);
        let mut locals = Declarations::new();
        for param in &declaration.params {
            self.check_parameter(param, globals, &mut locals);
            scope.add_local(&param.name);
        }
        let mut lets = Vec::new();
        for binding in &declaration.lets {
            self.check_name(&binding.name);
            let name = &binding.name.name;
            if let Some(existing) = globals.get(name).or_else(|| locals.get(name)) {
                self.error(binding.name.span, already_declared(name, existing.noun()));
            } else {
                locals.insert(name.clone(), Kind::NodeLet);
            }
            if let Some(expression) = self.expression(&binding.expression, scope) {
                lets.push(Let {
                    name: name.clone(),
                    expression,
                });
            }
            scope.add_local(name);
        }
        let branches = self.branches(declaration, scope, node_ids);
        scope.clear_locals();

        if self.errors_since(first_diagnostic) {
            return None;
        }
        Some(Node {
            name: declaration.name.name.clone(),
            params: declaration
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            annotations,
            lets,
            branches,
            resumes_ask: false,
            is_entry: false,
            span: declaration.span,
        })
    }

    /// A node's branches: its single outcome, or each branch of its `when`.
    fn branches(
        &mut self,
        declaration: &NodeDeclaration,
        scope: &Scope,
        node_ids: &HashMap<SmolStr, NodeId>,
    ) -> Vec<Branch> {
        let (source_branches, when_span) = match &declaration.body {
            Body::Single(outcome) => {
                let Some(compiled) = self.outcome(outcome, scope, node_ids) else {
                    return Vec::new();
                };
                return vec![Branch {
                    guard: None,
                    outcome: compiled,
                    needs_event: false,
                    span: outcome.span(),
                }];
            }
            Body::When { branches, span } => (branches, *span),
        };
        if source_branches.is_empty() {
            self.push(
                Diagnostic::error(when_span, "this `when` has no branches")
                    .with_help("add at least `else: ignore`"),
            );
        }
        let mut branches = Vec::new();
        let mut seen_else = false;
        for branch in source_branches {
            if seen_else {
                self.push(Diagnostic::error(branch.span, AFTER_ELSE).with_help(ELSE_LAST));
            }
            let guard = match &branch.guard {
                Guard::Else(_) => {
                    seen_else = true;
                    None
                }
                Guard::Expression(parsed) => match self.expression(parsed, scope) {
                    Some(guard) => Some(guard),
                    None => continue,
                },
            };
            let needs_event = guard
                .as_ref()
                .is_some_and(|guard| guard_needs_event(self.src, guard.span()));
            if let Some(outcome) = self.outcome(&branch.outcome, scope, node_ids) {
                branches.push(Branch {
                    guard,
                    outcome,
                    needs_event,
                    span: branch.span,
                });
            }
        }
        if !seen_else && !source_branches.is_empty() {
            self.push(
                Diagnostic::error(
                    when_span,
                    format!(
                        "the `when` in node `{}` has no `else`",
                        declaration.name.name
                    ),
                )
                .with_help(
                    "every `when` ends with `else: <outcome>`; use `else: ignore` to do nothing",
                ),
            );
        }
        branches
    }

    fn outcome(
        &mut self,
        outcome: &ast::Outcome,
        scope: &Scope,
        node_ids: &HashMap<SmolStr, NodeId>,
    ) -> Option<Outcome> {
        match outcome {
            ast::Outcome::Ignore { .. } => Some(Outcome::Ignore),
            ast::Outcome::Jump { target, args, .. } => {
                let node = self.resolve_target(target, args.len(), node_ids)?;
                let args = self.args(args, scope)?;
                Some(Outcome::Jump { node, args })
            }
            ast::Outcome::Ask {
                name,
                payload,
                target,
                args,
                ..
            } => {
                let payload = match payload {
                    Some(parsed) => Some(self.expression(parsed, scope)?),
                    None => None,
                };
                let node = self.resolve_target(target, args.len(), node_ids)?;
                let args = self.args(args, scope)?;
                Some(Outcome::Ask {
                    name: name.name.clone(),
                    payload,
                    node,
                    args,
                })
            }
            ast::Outcome::Value { template, wake, .. } => {
                let template = self.template(template, scope)?;
                let wake = match wake {
                    Some(parsed) => Some(self.expression(parsed, scope)?),
                    None => None,
                };
                Some(Outcome::Value { template, wake })
            }
        }
    }

    /// The node a jump or ask continuation names, which must exist and take
    /// `arity` arguments.
    fn resolve_target(
        &mut self,
        target: &Identifier,
        arity: usize,
        node_ids: &HashMap<SmolStr, NodeId>,
    ) -> Option<NodeId> {
        let Some(&id) = node_ids.get(&target.name) else {
            self.unknown_node(target, node_ids);
            return None;
        };
        let expected = self.ast.nodes[id.index()].params.len();
        if expected != arity {
            self.error(
                target.span,
                format!(
                    "node `{}` takes {expected} argument{}, got {arity}",
                    target.name,
                    if expected == 1 { "" } else { "s" }
                ),
            );
            return None;
        }
        Some(id)
    }

    pub(super) fn unknown_node(&mut self, name: &Identifier, node_ids: &HashMap<SmolStr, NodeId>) {
        let mut diagnostic = Diagnostic::error(name.span, format!("unknown node `{}`", name.name));
        if let Some(suggestion) = self.hint(&name.name, node_ids.keys().map(SmolStr::as_str)) {
            diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
        }
        self.push(diagnostic);
    }

    fn args(&mut self, args: &[ParsedExpr], scope: &Scope) -> Option<Vec<ExprUnit>> {
        let mut compiled = Vec::with_capacity(args.len());
        let mut all_compiled = true;
        for arg in args {
            match self.expression(arg, scope) {
                Some(unit) => compiled.push(unit),
                None => all_compiled = false,
            }
        }
        all_compiled.then_some(compiled)
    }
}

/// Every ask name in the flow, first use first.
pub(super) fn ask_names(nodes: &[Node]) -> IndexSet<SmolStr> {
    nodes
        .iter()
        .flat_map(|node| &node.branches)
        .filter_map(|branch| match &branch.outcome {
            Outcome::Ask { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}
