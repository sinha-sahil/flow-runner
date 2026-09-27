//! The edits a builder can make, and how each becomes text changes.

use super::text::{literal_text, Change};
use super::Document;
use crate::common::diagnostic::Diagnostics;
use crate::common::error::{already_declared, twice, EditError, ELSE_LAST};
use crate::syntax;
use crate::syntax::ast::{
    Annotation, Body, Branch, Guard, Identifier, NameList, NodeDeclaration, Outcome,
    ParamDeclaration,
};
use crate::syntax::names::{annotation_problem, event_problem, name_problem};
use std::collections::HashSet;
use std::ops::Range;
use temple_dsl::{Span, Value};

/// How far a branch is indented in a `when` block an edit writes.
const BRANCH_INDENT: &str = "  ";

/// A change a builder can make. Text fields hold flow syntax exactly as it
/// would be typed: a guard is a temple expression, an outcome is a jump, a
/// value, an ask or `ignore`, and a body is everything after `node name =`.
/// A guard or outcome can't hold a `#` comment, which would hide the rest of
/// the line it lands on.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Edit {
    /// Add `node name(params) = body` after node `after`, or at the end.
    AddNode {
        /// The new node's name.
        name: String,
        /// Its parameter names.
        params: Vec<String>,
        /// Everything after `=`.
        body: String,
        /// The node to add it after; `None` for the end of the file.
        after: Option<String>,
    },
    /// Remove a node and its annotations. Jumps to it become errors to fix.
    RemoveNode {
        /// The node.
        name: String,
    },
    /// Rename a node and every jump, ask continuation and entry naming it.
    RenameNode {
        /// The node.
        from: String,
        /// Its new name.
        to: String,
    },
    /// Add `guard: outcome` before branch `before`, or before `else` when
    /// `None`. A node with a single outcome gains a `when` block, with its
    /// old outcome as `else`.
    AddBranch {
        /// The node.
        node: String,
        /// The new branch's guard.
        guard: String,
        /// The new branch's outcome.
        outcome: String,
        /// The branch to add it before.
        before: Option<usize>,
    },
    /// Remove branch `index` of a node's `when`.
    RemoveBranch {
        /// The node.
        node: String,
        /// The branch, counting from 0.
        index: usize,
    },
    /// Move a branch; `else` has to stay last.
    MoveBranch {
        /// The node.
        node: String,
        /// Where the branch is, counting from 0.
        from: usize,
        /// Where it goes.
        to: usize,
    },
    /// Replace a branch's guard. `"else"` makes the last branch the `else`.
    SetGuard {
        /// The node.
        node: String,
        /// The branch, counting from 0.
        index: usize,
        /// The new guard.
        guard: String,
    },
    /// Replace a branch's outcome. A single-outcome node's outcome is index 0.
    SetOutcome {
        /// The node.
        node: String,
        /// The branch, counting from 0.
        index: usize,
        /// The new outcome.
        outcome: String,
    },
    /// Set `@name = value` on the flow, a param or a node.
    SetAnnotation {
        /// The declaration it sits on.
        on: Declaration,
        /// The name after `@`.
        name: String,
        /// Its value.
        value: Value,
    },
    /// Remove `@name` from the flow, a param or a node.
    RemoveAnnotation {
        /// The declaration it sits on.
        on: Declaration,
        /// The name after `@`.
        name: String,
    },
    /// Add `param name = default` after the other params.
    AddParam {
        /// The param's name.
        name: String,
        /// Its default.
        default: Value,
    },
    /// Remove a param and its annotations.
    RemoveParam {
        /// The param.
        name: String,
    },
    /// Declare an event.
    AddEvent {
        /// The event.
        name: String,
    },
    /// Remove an event from `events`.
    RemoveEvent {
        /// The event.
        name: String,
    },
    /// List a node under `entry`.
    AddEntry {
        /// The node.
        node: String,
    },
    /// Remove a node from `entry`.
    RemoveEntry {
        /// The node.
        node: String,
    },
}

/// The declaration an annotation sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Declaration {
    /// The `flow` declaration.
    Flow,
    /// The param with this name.
    Param(String),
    /// The node with this name.
    Node(String),
}

impl Document {
    /// The text changes `edit` makes.
    pub(super) fn plan(&self, edit: &Edit) -> Result<Vec<Change>, EditError> {
        match edit {
            Edit::AddNode {
                name,
                params,
                body,
                after,
            } => self.add_node(name, params, body, after.as_deref()),
            Edit::RemoveNode { name } => {
                let node = self.node(name)?;
                Ok(vec![self.remove_declaration(&node.annotations, node.span)])
            }
            Edit::RenameNode { from, to } => self.rename_node(from, to),
            Edit::AddBranch {
                node,
                guard,
                outcome,
                before,
            } => self.add_branch(node, guard, outcome, *before),
            Edit::RemoveBranch { node, index } => self.remove_branch(node, *index),
            Edit::MoveBranch { node, from, to } => self.move_branch(node, *from, *to),
            Edit::SetGuard { node, index, guard } => self.set_guard(node, *index, guard),
            Edit::SetOutcome {
                node,
                index,
                outcome,
            } => self.set_outcome(node, *index, outcome),
            Edit::SetAnnotation { on, name, value } => self.set_annotation(on, name, value),
            Edit::RemoveAnnotation { on, name } => {
                let (annotations, _) = self.declaration(on)?;
                let annotation = annotations
                    .iter()
                    .find(|annotation| annotation.name.name == name.as_str())
                    .ok_or_else(|| EditError::NotFound(format!("annotation `@{name}`")))?;
                Ok(vec![self.remove_span(annotation.span)])
            }
            Edit::AddParam { name, default } => self.add_param(name, default),
            Edit::RemoveParam { name } => {
                let param = self.param(name)?;
                Ok(vec![self.remove_declaration(&param.annotations, param.span)])
            }
            Edit::AddEvent { name } => self.add_event(name),
            Edit::RemoveEvent { name } => self.remove_listed(&self.ast.events, name, "event"),
            Edit::AddEntry { node } => self.add_entry(node),
            Edit::RemoveEntry { node } => self.remove_listed(&self.ast.entries, node, "entry"),
        }
    }

    fn node(&self, name: &str) -> Result<&NodeDeclaration, EditError> {
        self.ast
            .nodes
            .iter()
            .find(|node| node.name.name == name)
            .ok_or_else(|| EditError::NotFound(format!("node `{name}`")))
    }

    fn param(&self, name: &str) -> Result<&ParamDeclaration, EditError> {
        self.ast
            .params
            .iter()
            .find(|param| param.name.name == name)
            .ok_or_else(|| EditError::NotFound(format!("param `{name}`")))
    }

    /// What a top-level name is declared as, if it is: a param, a let, a
    /// helper or a node.
    fn declared_as(&self, name: &str) -> Option<&'static str> {
        let ast = &self.ast;
        if ast.params.iter().any(|param| param.name.name == name) {
            return Some("a param");
        }
        if let Some(binding) = ast.lets.iter().find(|binding| binding.name.name == name) {
            return Some(if binding.params.is_some() {
                "a helper"
            } else {
                "a let"
            });
        }
        ast.nodes
            .iter()
            .any(|node| node.name.name == name)
            .then_some("a node")
    }

    fn check_new_name(&self, name: &str) -> Result<(), EditError> {
        if let Some(problem) = name_problem(name) {
            return Err(EditError::BadName(problem));
        }
        match self.declared_as(name) {
            Some(what) => Err(EditError::BadName(already_declared(name, what))),
            None => Ok(()),
        }
    }

    fn add_node(
        &self,
        name: &str,
        params: &[String],
        body: &str,
        after: Option<&str>,
    ) -> Result<Vec<Change>, EditError> {
        self.check_new_name(name)?;
        let mut seen = HashSet::new();
        for param in params {
            if let Some(problem) = name_problem(param) {
                return Err(EditError::BadName(problem));
            }
            if !seen.insert(param) {
                return Err(EditError::BadName(twice("parameter", param)));
            }
        }
        let head = if params.is_empty() {
            format!("node {name} = ")
        } else {
            format!("node {name}({}) = ", params.join(", "))
        };
        let declaration = format!("{head}{}", body.trim());
        syntax::parse_node(&declaration)
            .map_err(|errors| syntax_error(errors, head.len(), body))?;
        let at = match after {
            Some(after) => self.next_free_line(self.node(after)?.span.end as usize),
            None => self.text.len(),
        };
        let lead = if at == 0 || self.text[..at].ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let rest = &self.text[at..];
        let trail = if rest.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n") {
            ""
        } else {
            "\n"
        };
        Ok(vec![Change::insert(
            at,
            format!("{lead}{declaration}\n{trail}"),
        )])
    }

    fn rename_node(&self, from: &str, to: &str) -> Result<Vec<Change>, EditError> {
        if !self.syntax_errors.is_empty() {
            return Err(EditError::Invalid(
                "fix the syntax errors first: a rename can't see references inside declarations that don't parse".into(),
            ));
        }
        let node = self.node(from)?;
        if from == to {
            return Ok(Vec::new());
        }
        self.check_new_name(to)?;
        let mut changes = vec![Change::replace(node.name.span, to)];
        let mut retarget = |target: &Identifier| {
            if target.name == from {
                changes.push(Change::replace(target.span, to));
            }
        };
        for outcome in self.ast.outcomes() {
            if let Outcome::Jump { target, .. } | Outcome::Ask { target, .. } = outcome {
                retarget(target);
            }
        }
        for entry in self.ast.entry_names() {
            retarget(entry);
        }
        Ok(changes)
    }

    fn add_branch(
        &self,
        node: &str,
        guard: &str,
        outcome: &str,
        before: Option<usize>,
    ) -> Result<Vec<Change>, EditError> {
        check_guard(guard)?;
        check_outcome(outcome)?;
        let guard = guard.trim();
        let branch = format!("{guard}: {}", outcome.trim());
        let declaration = self.node(node)?;
        let (branches, block) = match &declaration.body {
            Body::When { branches, span } => (branches, *span),
            Body::Single(old) => {
                if before.is_some_and(|before| before > 0) {
                    return Err(EditError::Invalid(
                        "this node has one outcome; a new branch goes before it".into(),
                    ));
                }
                if guard == "else" {
                    return Err(EditError::Invalid(ELSE_LAST.into()));
                }
                return Ok(vec![self.when_block(declaration, old, &branch)]);
            }
        };
        let else_at = branches
            .iter()
            .position(|branch| matches!(branch.guard, Guard::Else(_)));
        let at = before.or(else_at).unwrap_or(branches.len());
        if at > branches.len() {
            return Err(missing_branch(node, at));
        }
        let after_else = else_at.is_some_and(|else_at| at > else_at);
        if after_else || (guard == "else" && at < branches.len()) {
            return Err(EditError::Invalid(ELSE_LAST.into()));
        }
        Ok(vec![self.insert_branch(branches, block, at, &branch)])
    }

    /// A single outcome made into a `when` block: `branch`, then the old
    /// outcome as `else`.
    fn when_block(&self, declaration: &NodeDeclaration, old: &Outcome, branch: &str) -> Change {
        let span = old.span();
        let start = span.start as usize;
        let base = if self.starts_line(start) {
            self.indent_at(start)
        } else {
            self.indent_at(declaration.span.start as usize)
        };
        let inner = format!("{base}{BRANCH_INDENT}");
        let text = format!(
            "when {{\n{inner}{branch}\n{inner}else: {}\n{base}}}",
            self.slice(span)
        );
        Change::replace(span, text)
    }

    /// `branch` put in at position `at` of a `when` block.
    fn insert_branch(&self, branches: &[Branch], block: Span, at: usize, branch: &str) -> Change {
        if let Some(next) = branches.get(at) {
            let start = next.span.start as usize;
            if self.starts_line(start) {
                let indent = self.indent_at(start);
                return Change::insert(self.line_start(start), format!("{indent}{branch}\n"));
            }
            return Change::insert(start, format!("{branch}, "));
        }
        match branches.last() {
            Some(last)
                if self.starts_line(last.span.start as usize)
                    && self.ends_line(last.span.end as usize) =>
            {
                let indent = self.indent_at(last.span.start as usize);
                Change::insert(
                    self.line_after(last.span.end as usize),
                    format!("{indent}{branch}\n"),
                )
            }
            Some(last) => Change::insert(last.span.end as usize, format!(", {branch}")),
            None => {
                let close = block.end as usize - 1;
                Change::insert(close, format!("\n{BRANCH_INDENT}{branch}\n"))
            }
        }
    }

    /// The branches of node `name`'s `when`, for an edit that would `action`
    /// one.
    fn when_branches(&self, name: &str, action: &str) -> Result<&[Branch], EditError> {
        match &self.node(name)?.body {
            Body::When { branches, .. } => Ok(branches),
            Body::Single(_) => Err(EditError::Invalid(format!(
                "node `{name}` has a single outcome, so it has no branches to {action}"
            ))),
        }
    }

    fn remove_branch(&self, node: &str, index: usize) -> Result<Vec<Change>, EditError> {
        let branches = self.when_branches(node, "remove")?;
        let branch = branches
            .get(index)
            .ok_or_else(|| missing_branch(node, index))?;
        Ok(vec![self.remove_span(branch.span)])
    }

    fn move_branch(&self, node: &str, from: usize, to: usize) -> Result<Vec<Change>, EditError> {
        let branches = self.when_branches(node, "move")?;
        if from >= branches.len() || to >= branches.len() {
            return Err(missing_branch(node, from.max(to)));
        }
        let mut order: Vec<usize> = (0..branches.len()).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        let else_at = order
            .iter()
            .position(|&index| matches!(branches[index].guard, Guard::Else(_)));
        if else_at.is_some_and(|else_at| else_at + 1 != order.len()) {
            return Err(EditError::Invalid(ELSE_LAST.into()));
        }
        // with one branch to a line, each takes its line's comment along, so
        // comments stay with the branch they describe
        let whole_lines = branches
            .iter()
            .all(|branch| self.ends_line(branch.span.end as usize));
        let extent = |branch: &Branch| {
            if whole_lines {
                let end = self.line_end(branch.span.end as usize);
                Span::new(branch.span.start as usize, end)
            } else {
                branch.span
            }
        };
        Ok(order
            .iter()
            .enumerate()
            .filter(|(position, index)| position != *index)
            .map(|(position, &index)| {
                Change::replace(
                    extent(&branches[position]),
                    self.slice(extent(&branches[index])),
                )
            })
            .collect())
    }

    fn set_guard(&self, node: &str, index: usize, guard: &str) -> Result<Vec<Change>, EditError> {
        check_guard(guard)?;
        let guard = guard.trim();
        let branches = self.when_branches(node, "change")?;
        let branch = branches
            .get(index)
            .ok_or_else(|| missing_branch(node, index))?;
        if guard == "else" && index + 1 != branches.len() {
            return Err(EditError::Invalid(ELSE_LAST.into()));
        }
        let span = match &branch.guard {
            Guard::Else(span) => *span,
            Guard::Expression(expression) => expression.span(),
        };
        Ok(vec![Change::replace(span, guard)])
    }

    fn set_outcome(
        &self,
        node: &str,
        index: usize,
        outcome: &str,
    ) -> Result<Vec<Change>, EditError> {
        check_outcome(outcome)?;
        let declaration = self.node(node)?;
        let span = match &declaration.body {
            Body::Single(old) if index == 0 => old.span(),
            Body::When { branches, .. } if index < branches.len() => branches[index].outcome.span(),
            _ => return Err(missing_branch(node, index)),
        };
        Ok(vec![Change::replace(span, outcome.trim())])
    }

    /// The annotations and span of a flow, param or node declaration.
    fn declaration(&self, on: &Declaration) -> Result<(&[Annotation], Span), EditError> {
        match on {
            Declaration::Flow => self
                .ast
                .flow_span
                .map(|span| (self.ast.annotations.as_slice(), span))
                .ok_or_else(|| EditError::NotFound("`flow` declaration".into())),
            Declaration::Param(name) => self
                .param(name)
                .map(|param| (param.annotations.as_slice(), param.span)),
            Declaration::Node(name) => self
                .node(name)
                .map(|node| (node.annotations.as_slice(), node.span)),
        }
    }

    fn set_annotation(
        &self,
        on: &Declaration,
        name: &str,
        value: &Value,
    ) -> Result<Vec<Change>, EditError> {
        if let Some(problem) = annotation_problem(name) {
            return Err(EditError::BadName(problem));
        }
        let text = format!("@{name} = {}", literal_text(value)?);
        let (annotations, span) = self.declaration(on)?;
        if let Some(existing) = annotations
            .iter()
            .find(|annotation| annotation.name.name == name)
        {
            return Ok(vec![Change::replace(existing.span, text)]);
        }
        let start = span.start as usize;
        if !self.starts_line(start) {
            // another declaration comes first on this line: annotate in place
            return Ok(vec![Change::insert(start, format!("{text} "))]);
        }
        Ok(vec![Change::insert(
            self.line_start(start),
            format!("{}{text}\n", self.indent_at(start)),
        )])
    }

    fn add_param(&self, name: &str, default: &Value) -> Result<Vec<Change>, EditError> {
        self.check_new_name(name)?;
        let line = format!("param {name} = {}", literal_text(default)?);
        if let Some(last) = self.ast.params.last() {
            return Ok(vec![self.insert_lines(
                self.next_free_line(last.span.end as usize),
                format!("{line}\n"),
            )]);
        }
        let at = self.after_header(&self.ast.events);
        Ok(vec![self.insert_lines(at, format!("\n{line}\n"))])
    }

    fn add_event(&self, name: &str) -> Result<Vec<Change>, EditError> {
        if let Some(problem) = event_problem(name) {
            return Err(EditError::BadName(problem));
        }
        if self.ast.event_names().any(|event| event.name == name) {
            return Err(EditError::BadName(already_declared(name, "an event")));
        }
        Ok(self.add_listed(&self.ast.events, name, "events"))
    }

    fn add_entry(&self, node: &str) -> Result<Vec<Change>, EditError> {
        self.node(node)?;
        if self.ast.entry_names().any(|entry| entry.name == node) {
            return Err(EditError::Invalid(already_declared(node, "an entry")));
        }
        Ok(self.add_listed(&self.ast.entries, node, "entry"))
    }

    /// Add a name to the last `entry` or `events` declaration, or start one
    /// after the entries and the flow declaration.
    fn add_listed(&self, lists: &[NameList], name: &str, keyword: &str) -> Vec<Change> {
        if let Some(last) = lists.last().and_then(|list| list.names.last()) {
            return vec![Change::insert(last.span.end as usize, format!(", {name}"))];
        }
        vec![self.insert_lines(self.after_header(lists), format!("{keyword} {name}\n"))]
    }

    /// Where lines go after `lists`, the entries and the flow declaration:
    /// past the last of them, or at the end of a file without any.
    fn after_header(&self, lists: &[NameList]) -> usize {
        let ast = &self.ast;
        lists
            .iter()
            .chain(&ast.entries)
            .map(|list| list.span.end)
            .chain(ast.flow_span.map(|span| span.end))
            .max()
            .map_or(self.text.len(), |end| self.next_free_line(end as usize))
    }

    /// Remove one name from `entry` or `events`, and the declaration with it
    /// when it was the only name.
    fn remove_listed(
        &self,
        lists: &[NameList],
        name: &str,
        what: &str,
    ) -> Result<Vec<Change>, EditError> {
        let (list, position) = lists
            .iter()
            .find_map(|list| {
                let position = list.names.iter().position(|listed| listed.name == name)?;
                Some((list, position))
            })
            .ok_or_else(|| EditError::NotFound(format!("{what} `{name}`")))?;
        if list.names.len() == 1 {
            return Ok(vec![self.remove_span(list.span)]);
        }
        let Range { start, end } = list.names[position].span.range();
        let next = list
            .names
            .get(position + 1)
            .map(|next| next.span.start as usize);
        let own_line =
            self.starts_line(start) && next.map_or(true, |next| !self.same_line(end, next));
        if !own_line {
            // the next name takes its place, or, for the last, the text from
            // the name before it goes, comma and all
            return Ok(vec![match next {
                Some(next) => Change::remove(start, next),
                None => Change::remove(list.names[position - 1].span.end as usize, end),
            }]);
        }
        // on a line of its own, with its comment: the line goes, and the last
        // name also takes the comma before it
        let mut changes = vec![Change::remove(self.line_start(start), self.line_after(end))];
        if next.is_none() {
            let previous = list.names[position - 1].span.end as usize;
            if let Some(comma) = self.text[previous..].find(',') {
                changes.push(Change::remove(previous + comma, previous + comma + 1));
            }
        }
        Ok(changes)
    }
}

/// Check that a guard parses on its own.
fn check_guard(guard: &str) -> Result<(), EditError> {
    let trimmed = guard.trim();
    if trimmed == "else" {
        return Ok(());
    }
    syntax::parse_guard(trimmed).map_err(|errors| syntax_error(errors, 0, guard))
}

/// Check that an outcome parses on its own.
fn check_outcome(outcome: &str) -> Result<(), EditError> {
    syntax::parse_outcome(outcome.trim()).map_err(|errors| syntax_error(errors, 0, outcome))
}

fn missing_branch(node: &str, index: usize) -> EditError {
    EditError::NotFound(format!("branch {index} of node `{node}`"))
}

/// Why `given`, an edit's text, doesn't parse. `errors` point into the text
/// parsed, which holds `given` trimmed from byte `at` on; they are moved to
/// point into `given`. A span before `at`, in text the edit wrote around
/// it, points at its start.
fn syntax_error(errors: Diagnostics, at: usize, given: &str) -> EditError {
    let given_at = given.len() - given.trim_start().len();
    let place = |offset: u32| {
        (offset as usize)
            .saturating_sub(at)
            .saturating_add(given_at)
            .min(given.len())
    };
    EditError::Syntax(Diagnostics::new(
        errors
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.span =
                    Span::new(place(diagnostic.span.start), place(diagnostic.span.end));
                diagnostic
            })
            .collect(),
    ))
}
