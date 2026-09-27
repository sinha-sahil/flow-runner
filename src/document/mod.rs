//! Editing a flow as text. A [`Document`] gives a visual builder a [`Graph`]
//! to draw and applies checked [`Edit`]s that rewrite only the text they
//! touch, so comments and layout elsewhere survive a trip through the builder.

mod edits;
mod graph;
mod outline;
mod text;

pub use edits::{Declaration, Edit};
pub use graph::{
    AnnotationView, BranchView, Edge, EdgeKind, Graph, LetView, NodeView, OutcomeView, ParamView,
};

use self::outline::Outline;
use crate::common::diagnostic::{Diagnostic, Diagnostics};
use crate::common::error::{too_large, EditError};
use crate::common::limits::MAX_SOURCE_BYTES;
use crate::compile;
use crate::syntax;
use crate::syntax::ast::FlowAst;
use std::collections::HashMap;

/// A `.flow` file open for editing. The text is the source of truth; the
/// graph is a view of it.
///
/// ```
/// use flow_runner::{Document, Edit};
///
/// let mut doc = Document::parse("edition 2026\nflow f\nentry a\n\nnode a = { \"ok\": true }\n");
/// doc.apply(Edit::RenameNode { from: "a".into(), to: "start".into() }).unwrap();
/// assert_eq!(doc.text(), "edition 2026\nflow f\nentry start\n\nnode start = { \"ok\": true }\n");
/// assert!(doc.diagnostics().is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct Document {
    text: String,
    ast: FlowAst,
    /// The syntax errors in the text as it stands. An edit may not add one.
    syntax_errors: Vec<String>,
    diagnostics: Diagnostics,
}

impl Document {
    /// Open a file. Never fails: problems are in
    /// [`diagnostics`](Self::diagnostics), and the graph shows every
    /// declaration that parsed. A file over [`MAX_SOURCE_BYTES`] isn't read
    /// at all.
    pub fn parse(text: impl Into<String>) -> Document {
        let text = text.into();
        if text.len() > MAX_SOURCE_BYTES {
            let error = compile::too_large_source(&text);
            return Document {
                text,
                ast: FlowAst::default(),
                syntax_errors: vec![error.message.clone()],
                diagnostics: Diagnostics::new(vec![error]),
            };
        }
        let (ast, parse_diagnostics) = syntax::parse(&text);
        Document::parsed(text, ast, parse_diagnostics)
    }

    fn parsed(text: String, ast: FlowAst, parse_diagnostics: Vec<Diagnostic>) -> Document {
        let syntax_errors = parse_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        let diagnostics = compile::diagnostics(compile::parsed(&text, &ast, parse_diagnostics));
        Document {
            text,
            ast,
            syntax_errors,
            diagnostics,
        }
    }

    /// The file as it stands, to save.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Every error and warning, as [`Flow::check`](crate::Flow::check)
    /// reports them.
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Apply one edit. On error the document is unchanged. An edit that
    /// parses but leaves the flow wrong, such as removing a node others jump
    /// to, is applied, and the problem shows in the diagnostics. An edit that
    /// would change more of the file than it asks for is refused.
    pub fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        let mut changes = self.plan(&edit)?;
        let line_break = self.line_break();
        if line_break != "\n" {
            // text copied from the file already has `\r\n`
            for change in &mut changes {
                change.text = change.text.replace("\r\n", "\n").replace('\n', line_break);
            }
        }
        let text = text::splice(&self.text, changes);
        if text.len() > MAX_SOURCE_BYTES {
            return Err(EditError::Invalid(too_large(
                "the flow it would make",
                text.len(),
                MAX_SOURCE_BYTES,
            )));
        }
        let (ast, parse_diagnostics) = syntax::parse(&text);
        if let Some(message) = self.new_syntax_error(&parse_diagnostics) {
            return Err(EditError::Invalid(format!(
                "the edit doesn't parse where it goes: {message}"
            )));
        }
        if Outline::of(&self.ast).after(&edit, &ast) != Some(Outline::of(&ast)) {
            return Err(EditError::Invalid(
                "the edit would change more of the file than it asks for".into(),
            ));
        }
        *self = Document::parsed(text, ast, parse_diagnostics);
        Ok(())
    }

    /// A syntax error in `after` that the text as it stands doesn't have.
    fn new_syntax_error<'d>(&self, after: &'d [Diagnostic]) -> Option<&'d str> {
        let mut before: HashMap<&str, usize> = HashMap::new();
        for message in &self.syntax_errors {
            *before.entry(message).or_default() += 1;
        }
        after
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message.as_str())
            .find(|message| match before.get_mut(message) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    false
                }
                _ => true,
            })
    }
}
