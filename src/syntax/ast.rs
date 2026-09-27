//! The syntax tree of a `.flow` file: what the parser produces and the
//! compiler and editor read. Temple parses each expression and value where
//! it sits, so every span is absolute into the file.

use smol_str::SmolStr;
use temple_dsl::{ParsedExpr, ParsedTemplate, Span};

/// Words with a meaning in the outer grammar.
pub(crate) const KEYWORDS: &[&str] = &[
    "edition", "flow", "entry", "events", "param", "let", "node", "when", "else", "ask", "wake",
    "ignore",
];

/// Words that start a top-level declaration.
pub(crate) const DECLARATIONS: &[&str] =
    &["edition", "flow", "entry", "events", "param", "let", "node"];

/// Editions this version understands, oldest first.
pub(crate) const EDITIONS: &[u32] = &[2026];

/// The edition new files are written in.
pub(crate) const LATEST_EDITION: u32 = EDITIONS[EDITIONS.len() - 1];

/// The event every flow takes without declaring it.
pub(crate) const TICK: &str = "tick";

/// Words that are values, not names.
pub(crate) const LITERAL_WORDS: &[&str] = &["true", "false", "null"];

/// A name and where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier {
    pub name: SmolStr,
    pub span: Span,
}

/// `@name` or `@name = value`, on a flow, a param or a node.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: Identifier,
    /// `None` for a bare `@name`, which means `true`.
    pub value: Option<ParsedExpr>,
    pub span: Span,
}

/// `param name = default`.
#[derive(Debug, Clone)]
pub struct ParamDeclaration {
    pub name: Identifier,
    pub default: ParsedExpr,
    pub annotations: Vec<Annotation>,
    pub span: Span,
}

/// `let name = expression`, or a helper `let name(a, b) = expression`.
#[derive(Debug, Clone)]
pub struct LetDeclaration {
    pub name: Identifier,
    /// `Some` for a helper, even one with no parameters.
    pub params: Option<Vec<Identifier>>,
    pub expression: ParsedExpr,
    pub span: Span,
}

/// `node name(params) = lets… body`.
#[derive(Debug, Clone)]
pub struct NodeDeclaration {
    pub name: Identifier,
    pub params: Vec<Identifier>,
    pub annotations: Vec<Annotation>,
    pub lets: Vec<LetDeclaration>,
    pub body: Body,
    pub span: Span,
}

impl NodeDeclaration {
    /// The branches of its `when`, or 1 for a single outcome.
    pub fn branch_count(&self) -> usize {
        match &self.body {
            Body::When { branches, .. } => branches.len(),
            Body::Single(_) => 1,
        }
    }
}

/// What a node does: choose between branches, or one outcome.
#[derive(Debug, Clone)]
pub enum Body {
    When { branches: Vec<Branch>, span: Span },
    Single(Outcome),
}

/// One `guard: outcome` line of a `when` block.
#[derive(Debug, Clone)]
pub struct Branch {
    pub guard: Guard,
    pub outcome: Outcome,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Guard {
    Else(Span),
    Expression(ParsedExpr),
}

/// What a branch leads to.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// `node(args)`: go to another node in the same run.
    Jump {
        target: Identifier,
        args: Vec<ParsedExpr>,
        span: Span,
    },
    /// A value, optionally with `wake <time>`.
    Value {
        template: ParsedTemplate,
        wake: Option<ParsedExpr>,
        span: Span,
    },
    /// `ask name(payload) -> node(args)`.
    Ask {
        name: Identifier,
        payload: Option<ParsedExpr>,
        target: Identifier,
        args: Vec<ParsedExpr>,
        span: Span,
    },
    Ignore {
        span: Span,
    },
}

impl Outcome {
    pub fn span(&self) -> Span {
        match self {
            Outcome::Jump { span, .. }
            | Outcome::Value { span, .. }
            | Outcome::Ask { span, .. }
            | Outcome::Ignore { span } => *span,
        }
    }
}

/// One `entry a, b` or `events a, b.c` declaration.
#[derive(Debug, Clone)]
pub struct NameList {
    pub names: Vec<Identifier>,
    /// The whole declaration, keyword to last name.
    pub span: Span,
}

/// A parsed `.flow` file, before any meaning is checked.
#[derive(Debug, Clone, Default)]
pub struct FlowAst {
    /// The year after `edition`.
    pub edition: Option<u32>,
    pub name: Option<Identifier>,
    /// The whole `flow <name>` declaration.
    pub flow_span: Option<Span>,
    pub annotations: Vec<Annotation>,
    /// Each `entry …` declaration, in file order.
    pub entries: Vec<NameList>,
    /// Each `events …` declaration, in file order.
    pub events: Vec<NameList>,
    pub params: Vec<ParamDeclaration>,
    pub lets: Vec<LetDeclaration>,
    pub nodes: Vec<NodeDeclaration>,
}

impl FlowAst {
    /// Every entry node named, across all `entry` declarations.
    pub fn entry_names(&self) -> impl Iterator<Item = &Identifier> {
        self.entries.iter().flat_map(|list| list.names.iter())
    }

    /// Every event named, across all `events` declarations.
    pub fn event_names(&self) -> impl Iterator<Item = &Identifier> {
        self.events.iter().flat_map(|list| list.names.iter())
    }

    /// Every outcome in the file, node by node.
    pub fn outcomes(&self) -> impl Iterator<Item = &Outcome> {
        self.nodes.iter().flat_map(|node| {
            let (single, branches) = match &node.body {
                Body::Single(outcome) => (Some(outcome), &[][..]),
                Body::When { branches, .. } => (None, branches.as_slice()),
            };
            single
                .into_iter()
                .chain(branches.iter().map(|branch| &branch.outcome))
        })
    }
}
