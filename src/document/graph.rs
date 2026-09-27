//! The [`Graph`] a builder draws.

use super::Document;
use crate::compile::literal;
use crate::syntax::ast::{Annotation, Body, Guard, LetDeclaration, NodeDeclaration, Outcome};
use std::collections::HashSet;
use temple_dsl::{ParsedExpr, Span, Value};

/// Everything a builder needs to draw a flow. Expression texts are exactly
/// as written in the file; spans are byte offsets into [`Document::text`].
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// The name after `flow`, when there is one.
    pub name: Option<String>,
    /// The annotations on the `flow` declaration.
    pub annotations: Vec<AnnotationView>,
    /// The nodes under `entry`, in file order.
    pub entries: Vec<String>,
    /// The declared events, in file order.
    pub events: Vec<String>,
    /// The params, in file order.
    pub params: Vec<ParamView>,
    /// Flow-level lets and helpers.
    pub lets: Vec<LetView>,
    /// The nodes, in file order.
    pub nodes: Vec<NodeView>,
    /// Jumps and ask continuations, in node and branch order.
    pub edges: Vec<Edge>,
}

/// An annotation as written.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationView {
    /// The name after `@`.
    pub name: String,
    /// The value, or `None` when it isn't a literal. A bare `@name` is `true`.
    pub value: Option<Value>,
    /// The value as written; `None` for a bare `@name`.
    pub text: Option<String>,
    /// Where the annotation is.
    pub span: Span,
}

/// A `param` declaration as written.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamView {
    /// The param's name.
    pub name: String,
    /// The default, or `None` when it isn't a literal.
    pub default: Option<Value>,
    /// The default as written.
    pub default_text: String,
    /// The annotations on it.
    pub annotations: Vec<AnnotationView>,
    /// Where the declaration is, without its annotations.
    pub span: Span,
}

/// A `let` or helper declaration as written.
#[derive(Debug, Clone, PartialEq)]
pub struct LetView {
    /// The name it binds.
    pub name: String,
    /// `Some` for a helper `let name(a, b) = ...`.
    pub params: Option<Vec<String>>,
    /// The expression as written.
    pub expression: String,
    /// Where the declaration is.
    pub span: Span,
}

/// A `node` declaration as written.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    /// The node's name.
    pub name: String,
    /// Its parameter names.
    pub params: Vec<String>,
    /// The annotations on it.
    pub annotations: Vec<AnnotationView>,
    /// Its lets, in order.
    pub lets: Vec<LetView>,
    /// Its branches, or its single outcome as one branch.
    pub branches: Vec<BranchView>,
    /// Whether it is listed under `entry`.
    pub is_entry: bool,
    /// False for a node written as a single outcome, without `when`.
    pub uses_when: bool,
    /// Where the declaration is, without its annotations.
    pub span: Span,
}

/// One branch of a node: a `when` branch, or the node's single outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchView {
    /// `None` for `else`, or for a node's single outcome.
    pub guard: Option<String>,
    /// What the branch does.
    pub outcome: OutcomeView,
    /// Where the branch is.
    pub span: Span,
}

/// What a branch does, with each expression as written.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OutcomeView {
    /// `target(args)`.
    Jump {
        /// The node jumped to.
        target: String,
        /// The arguments.
        args: Vec<String>,
    },
    /// A value, and when to wake.
    Value {
        /// The value.
        template: String,
        /// The `wake` expression.
        wake: Option<String>,
    },
    /// `ask name(payload) -> target(args)`.
    Ask {
        /// The ask's name.
        name: String,
        /// What to ask with.
        payload: Option<String>,
        /// The node the answer continues at.
        target: String,
        /// Its arguments.
        args: Vec<String>,
    },
    /// `ignore`.
    Ignore,
}

/// A way from one node to another.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// The node it leaves.
    pub from: String,
    /// The node it reaches.
    pub to: String,
    /// Index of the branch in `from` the edge leaves by.
    pub branch: usize,
    /// How it is taken.
    pub kind: EdgeKind,
}

/// How an [`Edge`] is taken.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EdgeKind {
    /// A jump.
    Jump,
    /// An ask continuation, with the ask's name.
    Ask(String),
}

impl Document {
    /// The flow as a [`Graph`] to draw.
    pub fn graph(&self) -> Graph {
        let ast = &self.ast;
        let entries: HashSet<&str> = ast.entry_names().map(|entry| entry.name.as_str()).collect();
        let nodes: Vec<NodeView> = ast
            .nodes
            .iter()
            .map(|node| self.node_view(node, &entries))
            .collect();
        Graph {
            name: ast.name.as_ref().map(|name| name.name.to_string()),
            annotations: self.annotation_views(&ast.annotations),
            entries: ast
                .entry_names()
                .map(|entry| entry.name.to_string())
                .collect(),
            events: ast
                .event_names()
                .map(|event| event.name.to_string())
                .collect(),
            params: ast
                .params
                .iter()
                .map(|param| ParamView {
                    name: param.name.name.to_string(),
                    default: literal::value(&self.text, &param.default),
                    default_text: self.slice(param.default.span()).to_string(),
                    annotations: self.annotation_views(&param.annotations),
                    span: param.span,
                })
                .collect(),
            lets: ast
                .lets
                .iter()
                .map(|binding| self.let_view(binding))
                .collect(),
            edges: edges(&nodes),
            nodes,
        }
    }

    fn node_view(&self, node: &NodeDeclaration, entries: &HashSet<&str>) -> NodeView {
        let branches = match &node.body {
            Body::Single(outcome) => vec![BranchView {
                guard: None,
                outcome: self.outcome_view(outcome),
                span: outcome.span(),
            }],
            Body::When { branches, .. } => branches
                .iter()
                .map(|branch| BranchView {
                    guard: match &branch.guard {
                        Guard::Else(_) => None,
                        Guard::Expression(guard) => Some(self.slice(guard.span()).to_string()),
                    },
                    outcome: self.outcome_view(&branch.outcome),
                    span: branch.span,
                })
                .collect(),
        };
        NodeView {
            name: node.name.name.to_string(),
            params: node
                .params
                .iter()
                .map(|param| param.name.to_string())
                .collect(),
            annotations: self.annotation_views(&node.annotations),
            // a helper in a node is an error; show it as a plain let
            lets: node
                .lets
                .iter()
                .map(|binding| LetView {
                    params: None,
                    ..self.let_view(binding)
                })
                .collect(),
            branches,
            is_entry: entries.contains(node.name.name.as_str()),
            uses_when: matches!(node.body, Body::When { .. }),
            span: node.span,
        }
    }

    fn let_view(&self, binding: &LetDeclaration) -> LetView {
        LetView {
            name: binding.name.name.to_string(),
            params: binding
                .params
                .as_ref()
                .map(|params| params.iter().map(|param| param.name.to_string()).collect()),
            expression: self.slice(binding.expression.span()).to_string(),
            span: binding.span,
        }
    }

    fn annotation_views(&self, annotations: &[Annotation]) -> Vec<AnnotationView> {
        annotations
            .iter()
            .map(|annotation| AnnotationView {
                name: annotation.name.name.to_string(),
                value: match &annotation.value {
                    None => Some(Value::Bool(true)),
                    Some(value) => literal::value(&self.text, value),
                },
                text: annotation
                    .value
                    .as_ref()
                    .map(|value| self.slice(value.span()).to_string()),
                span: annotation.span,
            })
            .collect()
    }

    fn outcome_view(&self, outcome: &Outcome) -> OutcomeView {
        let texts = |args: &[ParsedExpr]| {
            args.iter()
                .map(|arg| self.slice(arg.span()).to_string())
                .collect()
        };
        match outcome {
            Outcome::Jump { target, args, .. } => OutcomeView::Jump {
                target: target.name.to_string(),
                args: texts(args),
            },
            Outcome::Value { template, wake, .. } => OutcomeView::Value {
                template: self.slice(template.span()).to_string(),
                wake: wake
                    .as_ref()
                    .map(|wake| self.slice(wake.span()).to_string()),
            },
            Outcome::Ask {
                name,
                payload,
                target,
                args,
                ..
            } => OutcomeView::Ask {
                name: name.name.to_string(),
                payload: payload
                    .as_ref()
                    .map(|payload| self.slice(payload.span()).to_string()),
                target: target.name.to_string(),
                args: texts(args),
            },
            Outcome::Ignore { .. } => OutcomeView::Ignore,
        }
    }
}

fn edges(nodes: &[NodeView]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for node in nodes {
        for (branch, view) in node.branches.iter().enumerate() {
            let (to, kind) = match &view.outcome {
                OutcomeView::Jump { target, .. } => (target, EdgeKind::Jump),
                OutcomeView::Ask { name, target, .. } => (target, EdgeKind::Ask(name.clone())),
                OutcomeView::Value { .. } | OutcomeView::Ignore => continue,
            };
            edges.push(Edge {
                from: node.name.clone(),
                to: to.clone(),
                branch,
                kind,
            });
        }
    }
    edges
}

#[cfg(feature = "json")]
impl Graph {
    /// The graph as JSON, for builders written in other languages.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{json, Value as Json};
        let span = |span: &Span| json!([span.start, span.end]);
        let value = |value: &Option<Value>| value.clone().map_or(Json::Null, Json::from);
        let annotations = |annotations: &[AnnotationView]| -> Json {
            annotations
                .iter()
                .map(|annotation| {
                    json!({
                        "name": annotation.name, "value": value(&annotation.value),
                        "text": annotation.text, "span": span(&annotation.span),
                    })
                })
                .collect()
        };
        let lets = |lets: &[LetView]| -> Json {
            lets.iter()
                .map(|binding| {
                    json!({
                        "name": binding.name, "params": binding.params,
                        "expression": binding.expression, "span": span(&binding.span),
                    })
                })
                .collect()
        };
        let outcome = |outcome: &OutcomeView| match outcome {
            OutcomeView::Jump { target, args } => {
                json!({ "kind": "jump", "target": target, "args": args })
            }
            OutcomeView::Value { template, wake } => {
                json!({ "kind": "value", "template": template, "wake": wake })
            }
            OutcomeView::Ask {
                name,
                payload,
                target,
                args,
            } => json!({
                "kind": "ask", "name": name, "payload": payload, "target": target, "args": args,
            }),
            OutcomeView::Ignore => json!({ "kind": "ignore" }),
        };
        let edge = |edge: &Edge| {
            let (kind, ask) = match &edge.kind {
                EdgeKind::Jump => ("jump", None),
                EdgeKind::Ask(name) => ("ask", Some(name)),
            };
            json!({ "from": edge.from, "to": edge.to, "branch": edge.branch, "kind": kind, "ask": ask })
        };
        json!({
            "name": self.name,
            "annotations": annotations(&self.annotations),
            "entries": self.entries,
            "events": self.events,
            "params": self.params.iter().map(|param| json!({
                "name": param.name, "default": value(&param.default), "defaultText": param.default_text,
                "annotations": annotations(&param.annotations), "span": span(&param.span),
            })).collect::<Json>(),
            "lets": lets(&self.lets),
            "nodes": self.nodes.iter().map(|node| json!({
                "name": node.name, "params": node.params, "annotations": annotations(&node.annotations), "lets": lets(&node.lets),
                "isEntry": node.is_entry, "usesWhen": node.uses_when, "span": span(&node.span),
                "branches": node.branches.iter().map(|branch| json!({
                    "guard": branch.guard, "outcome": outcome(&branch.outcome), "span": span(&branch.span),
                })).collect::<Json>(),
            })).collect::<Json>(),
            "edges": self.edges.iter().map(edge).collect::<Json>(),
        })
    }
}
