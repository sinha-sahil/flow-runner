//! The parser: `.flow` text in, a [`FlowAst`] and diagnostics out.
//!
//! Parsing never gives up: after a bad declaration it skips to the next line
//! that starts one, so a file with several mistakes reports all of them. The
//! same lines bound what each declaration reads: nothing in it, a temple unit
//! included, reaches past the next declaration, so a broken value costs its
//! own lines, not the rest of the file.
//!
//! The parser is one type, [`Parser`], whose methods are split by concern:
//! this file reads the file as a whole, [`declarations`] reads each
//! top-level declaration but nodes, [`nodes`] reads nodes with their
//! branches and outcomes, and [`scan`] reads bytes and words.

mod declarations;
mod nodes;
mod scan;

use crate::common::diagnostic::{Diagnostic, Diagnostics};
use crate::common::error::listed;
use crate::syntax::ast::{FlowAst, DECLARATIONS, LATEST_EDITION};
use crate::syntax::lexer::{lex, Lexed, Mode};
use declarations::LetPlace;
use temple_dsl::Span;

/// Parse a whole file. Always returns a tree; problems are diagnostics.
pub(crate) fn parse(src: &str) -> (FlowAst, Vec<Diagnostic>) {
    let mut parser = Parser::new(src);
    let ast = parser.file();
    (ast, parser.diagnostics)
}

/// Parse `src` as exactly one guard, as an edit gives it.
pub(crate) fn parse_guard(src: &str) -> Result<(), Diagnostics> {
    let mut parser = Parser::refusing_comments(src);
    let parsed = parser.declaration(Parser::expression).is_some();
    parser.finish(parsed, "guard")
}

/// Parse `src` as exactly one outcome, as an edit gives it: a jump, a value,
/// an ask or `ignore`.
pub(crate) fn parse_outcome(src: &str) -> Result<(), Diagnostics> {
    let mut parser = Parser::refusing_comments(src);
    let parsed = parser.declaration(Parser::outcome).is_some();
    parser.finish(parsed, "outcome")
}

/// Parse `src` as exactly one `node` declaration.
pub(crate) fn parse_node(src: &str) -> Result<(), Diagnostics> {
    let mut parser = Parser::new(src);
    parser.skip_trivia();
    let parsed = parser
        .declaration(|parser| parser.node(Vec::new()))
        .is_some();
    parser.finish(parsed, "node")
}

/// A production that failed. Its diagnostic is already recorded, so the
/// caller only has to decide where to pick up again.
struct Fail;

type ParseResult<T> = Result<T, Fail>;

struct Parser<'a> {
    /// The whole source.
    whole: &'a str,
    /// What the declaration being parsed may read: the source up to the
    /// next line that starts a declaration.
    src: &'a str,
    bytes: &'a [u8],
    /// Where the parser is in the source.
    at: usize,
    diagnostics: Vec<Diagnostic>,
    /// Where each line that starts a declaration starts, in order.
    declaration_lines: Vec<usize>,
    /// Where the first `#` comment starts: outside temple units, and inside
    /// them too when comments are refused.
    comment: Option<usize>,
    /// Whether this is a snippet that can't hold a comment: a guard or
    /// outcome is spliced into the middle of a line, where a comment would
    /// hide the rest of it.
    refuse_comments: bool,
    /// Where the last token read ends, and where skipping whitespace and
    /// comments last stopped. An error where a declaration's text runs out
    /// points at `token_end`.
    token_end: usize,
    trivia_end: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        let mut parser = Parser {
            whole: src,
            src,
            bytes: src.as_bytes(),
            at: 0,
            diagnostics: Vec::new(),
            declaration_lines: Vec::new(),
            comment: None,
            refuse_comments: false,
            token_end: 0,
            trivia_end: 0,
        };
        let line_starts = std::iter::once(0).chain(
            src.bytes()
                .enumerate()
                .filter(|&(_, byte)| byte == b'\n')
                .map(|(at, _)| at + 1),
        );
        parser.declaration_lines = line_starts
            .filter(|&start| parser.starts_declaration(start))
            .collect();
        parser
    }

    /// A parser for a snippet that can't hold a comment.
    fn refusing_comments(src: &'a str) -> Self {
        Parser {
            refuse_comments: true,
            ..Parser::new(src)
        }
    }

    /// Parse the declaration at the cursor with `parse`, which reads only up
    /// to the next line that starts a declaration. An error where that text
    /// runs out points just past the declaration's last token instead, and
    /// after any error the cursor moves on to that next line.
    fn declaration<T>(&mut self, parse: impl FnOnce(&mut Self) -> ParseResult<T>) -> Option<T> {
        let first_diagnostic = self.diagnostics.len();
        let end = self.declaration_end(self.at);
        self.src = &self.whole[..end];
        self.bytes = self.src.as_bytes();
        let result = parse(self);
        self.src = self.whole;
        self.bytes = self.whole.as_bytes();
        let text_end = self.token_end;
        for diagnostic in &mut self.diagnostics[first_diagnostic..] {
            if diagnostic.span.start as usize >= end {
                diagnostic.span = Span::new(text_end, text_end);
            }
        }
        if result.is_err() {
            self.at = end;
        }
        result.ok()
    }

    /// The lexemes of the temple unit at `span`, which reads in `mode`. In a
    /// snippet that refuses comments, a comment among them is noted.
    fn lex_unit(&mut self, span: Span, mode: Mode) -> Lexed<'a> {
        let lexed = lex(self.src, span, mode);
        if let Some(at) = lexed.comment.filter(|_| self.refuse_comments) {
            self.comment = Some(self.comment.map_or(at, |first| first.min(at)));
        }
        lexed
    }

    /// End of a snippet parse: the production must have succeeded and used
    /// up the whole input, apart from whitespace and allowed comments.
    fn finish(mut self, parsed: bool, what: &str) -> Result<(), Diagnostics> {
        if parsed {
            self.skip_trivia();
            if self.at < self.bytes.len() {
                let end = self.line_end(self.at);
                self.diagnostics.push(Diagnostic::error(
                    Span::new(self.at, end),
                    format!("unexpected text after the {what}"),
                ));
            }
        }
        // first, since hiding a closing bracket may be why the parse failed
        if let Some(at) = self.comment.filter(|_| self.refuse_comments) {
            let refused = Diagnostic::error(
                Span::new(at, self.line_end(at)),
                format!("an edit's {what} can't hold a `#` comment"),
            )
            .with_help("it would hide the rest of the line it lands on");
            self.diagnostics.insert(0, refused);
        }
        let diagnostics = Diagnostics::new(self.diagnostics);
        if diagnostics.has_errors() {
            Err(diagnostics)
        } else {
            Ok(())
        }
    }

    fn file(&mut self) -> FlowAst {
        let mut ast = FlowAst::default();
        // annotations wait here until the declaration they belong to
        let mut pending = Vec::new();
        let mut first = true;
        loop {
            self.skip_trivia();
            if self.at >= self.bytes.len() {
                break;
            }
            let start = self.at;
            if self.peek() == Some(b'@') {
                if let Some(annotation) = self.declaration(Self::annotation) {
                    pending.push(annotation);
                }
                continue;
            }
            let word = self.peek_word();
            let takes_annotations = matches!(word, Some("flow" | "param" | "node"));
            if !pending.is_empty() && !takes_annotations {
                self.diagnostics.push(
                    Diagnostic::error(
                        pending[0].span,
                        "annotations can only be placed on a flow, a param or a node",
                    )
                    .with_help("move it directly above `flow`, `param` or `node`"),
                );
                pending.clear();
            }
            self.declaration(|parser| match word {
                Some("edition") => parser.edition(&mut ast, first),
                Some("flow") => parser.flow_declaration(&mut ast, std::mem::take(&mut pending)),
                Some("entry") => parser
                    .name_list("entry", "a node name", Self::word)
                    .map(|list| ast.entries.push(list)),
                Some("events") => parser
                    .name_list("events", "an event name", Self::dotted_name)
                    .map(|list| ast.events.push(list)),
                Some("param") => parser
                    .param(std::mem::take(&mut pending))
                    .map(|param| ast.params.push(param)),
                Some("let") => parser
                    .let_declaration(LetPlace::TopLevel)
                    .map(|binding| ast.lets.push(binding)),
                Some("node") => parser
                    .node(std::mem::take(&mut pending))
                    .map(|node| ast.nodes.push(node)),
                _ => {
                    let end = parser.line_end(start);
                    parser.diagnostics.push(
                        Diagnostic::error(
                            Span::new(start, end),
                            format!("expected a declaration: {}", listed(DECLARATIONS)),
                        )
                        .with_help("each declaration starts at the beginning of a line"),
                    );
                    Err(Fail)
                }
            });
            if first && word != Some("edition") {
                self.diagnostics.push(
                    Diagnostic::error(
                        Span::new(start, start),
                        format!("a flow file must start with `edition {LATEST_EDITION}`"),
                    )
                    .with_help(format!("add `edition {LATEST_EDITION}` as the first line")),
                );
            }
            first = false;
        }
        if let Some(annotation) = pending.first() {
            self.diagnostics.push(Diagnostic::error(
                annotation.span,
                "this annotation is not attached to a flow, param or node",
            ));
        }
        ast
    }
}
