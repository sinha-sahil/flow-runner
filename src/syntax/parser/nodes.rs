//! Nodes: their parameters and lets, `when` blocks and their branches, and
//! the outcomes a branch leads to. Every expression and value inside is
//! handed to temple's parser where it sits.

use super::declarations::LetPlace;
use super::{Fail, ParseResult, Parser};
use crate::common::diagnostic::{from_temple, Diagnostic};
use crate::common::error::too_large;
use crate::common::limits::MAX_UNIT_BYTES;
use crate::syntax::ast::{
    Annotation, Body, Branch, Guard, Identifier, NodeDeclaration, Outcome, KEYWORDS, LITERAL_WORDS,
};
use crate::syntax::lexer::{LexemeKind, Mode};
use temple_dsl::{CompileError, ExprUnit, ParsedExpr, Span, TemplateUnit};

/// What an outcome can be, for messages.
const OUTCOME_KINDS: &str = "an outcome is a jump `node(...)`, a value, `ask ...` or `ignore`";

impl Parser<'_> {
    pub(super) fn node(&mut self, annotations: Vec<Annotation>) -> ParseResult<NodeDeclaration> {
        let start = self.keyword("node")?.start as usize;
        self.skip_spaces();
        let name = self.word("a node name")?;
        self.skip_spaces();
        let params = if self.peek() == Some(b'(') {
            self.param_list()?
        } else {
            Vec::new()
        };
        self.skip_spaces();
        self.expect(b'=', "`=` after the node name, as in `node review = ...`")?;
        let mut lets = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek_word() == Some("let") {
                lets.push(self.let_declaration(LetPlace::Node)?);
            } else {
                break;
            }
        }
        let body = if self.peek_word() == Some("when") {
            self.when_block()?
        } else if self.at >= self.bytes.len() {
            return self.fail(name.span, format!("node `{}` has no body", name.name));
        } else {
            Body::Single(self.outcome()?)
        };
        let end = match &body {
            Body::When { span, .. } => span.end as usize,
            Body::Single(outcome) => outcome.span().end as usize,
        };
        Ok(NodeDeclaration {
            name,
            params,
            annotations,
            lets,
            body,
            span: Span::new(start, end),
        })
    }

    fn when_block(&mut self) -> ParseResult<Body> {
        let keyword = self.keyword("when")?;
        self.skip_spaces();
        self.expect(b'{', "`{` after `when`")?;
        let mut branches = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek().is_none() {
                return self.fail(keyword, "this `when` block is never closed");
            }
            if self.peek() == Some(b'}') {
                self.at += 1;
                break;
            }
            branches.push(self.branch()?);
            self.after_branch()?;
        }
        Ok(Body::When {
            branches,
            span: Span::new(keyword.start as usize, self.at),
        })
    }

    fn branch(&mut self) -> ParseResult<Branch> {
        let start = self.at;
        let guard = if self.else_follows() {
            Guard::Else(self.keyword("else")?)
        } else {
            Guard::Expression(self.expression()?)
        };
        self.skip_trivia();
        self.expect(b':', "`:` between the guard and its outcome")?;
        let outcome = self.outcome()?;
        let end = outcome.span().end as usize;
        Ok(Branch {
            guard,
            outcome,
            span: Span::new(start, end),
        })
    }

    /// After a branch: a comma, a line break, a comment, or the closing brace.
    fn after_branch(&mut self) -> ParseResult<()> {
        self.skip_spaces();
        match self.peek() {
            Some(b',') => {
                self.at += 1;
                Ok(())
            }
            Some(b'\n' | b'\r' | b'#' | b'}') | None => Ok(()),
            _ => self.fail(
                Span::new(self.at, self.line_end(self.at)),
                "expected a new line, `,` or `}` after this outcome",
            ),
        }
    }

    pub(super) fn outcome(&mut self) -> ParseResult<Outcome> {
        self.skip_trivia();
        let start = self.at;
        match self.peek_word() {
            Some("ignore") => {
                let span = self.keyword("ignore")?;
                Ok(Outcome::Ignore { span })
            }
            Some("ask") => self.ask(start),
            Some(word) if !LITERAL_WORDS.contains(&word) => {
                let word_end = self.at + word.len();
                let after = self.skip_spaces_from(word_end);
                if self.bytes.get(after) == Some(&b'(') {
                    let target = self.word("a node name")?;
                    let args = self.arg_list()?;
                    Ok(Outcome::Jump {
                        target,
                        args,
                        span: Span::new(start, self.at),
                    })
                } else {
                    let span = Span::new(start, word_end);
                    let help = if KEYWORDS.contains(&word) {
                        OUTCOME_KINDS.to_string()
                    } else {
                        format!("to jump to node `{word}`, write `{word}()`")
                    };
                    self.diagnostics.push(
                        Diagnostic::error(
                            span,
                            format!("unexpected `{word}` where an outcome was expected"),
                        )
                        .with_help(help),
                    );
                    Err(Fail)
                }
            }
            _ => self.value(start),
        }
    }

    fn ask(&mut self, start: usize) -> ParseResult<Outcome> {
        self.keyword("ask")?;
        self.skip_spaces();
        let name =
            self.dotted_name("the name of what to ask for, as in `ask warehouse.inspect(...)`")?;
        self.skip_spaces();
        self.expect(b'(', "`(` and a payload after the ask name")?;
        self.skip_trivia();
        let payload = if self.peek() == Some(b')') {
            None
        } else {
            Some(self.expression()?)
        };
        self.skip_trivia();
        self.expect(b')', "`)` to close the payload")?;
        self.skip_trivia();
        if !self.src[self.at..].starts_with("->") {
            return self.fail(
                self.char_at(self.at),
                "expected `->` and the node to continue at, as in `-> reviewed()`",
            );
        }
        self.at += "->".len();
        self.skip_trivia();
        let target = self.word("the node to continue at")?;
        let args = self.arg_list()?;
        Ok(Outcome::Ask {
            name,
            payload,
            target,
            args,
            span: Span::new(start, self.at),
        })
    }

    fn value(&mut self, start: usize) -> ParseResult<Outcome> {
        if start >= self.bytes.len() {
            self.diagnostics.push(
                Diagnostic::error(Span::new(start, start), "expected an outcome")
                    .with_help(OUTCOME_KINDS),
            );
            return Err(Fail);
        }
        let template = match TemplateUnit::parse_at(self.src, start) {
            Ok(template) => template,
            Err(errors) => return self.temple_fail(&errors, start, Mode::Output),
        };
        self.at = template.end();
        self.unit_parsed(template.span(), Mode::Output, "the value");
        let after_template = self.at;
        self.skip_trivia();
        let wake = if self.peek_word() == Some("wake") {
            self.keyword("wake")?;
            Some(self.expression()?)
        } else {
            // a line break after the value may be what ends its branch
            self.at = after_template;
            None
        };
        let end = wake.as_ref().map_or(template.end(), ParsedExpr::end);
        Ok(Outcome::Value {
            template,
            wake,
            span: Span::new(start, end),
        })
    }

    /// One temple expression at the cursor.
    pub(super) fn expression(&mut self) -> ParseResult<ParsedExpr> {
        self.skip_trivia();
        let start = self.at;
        if start >= self.bytes.len() {
            return self.fail(Span::new(start, start), "expected an expression");
        }
        match ExprUnit::parse_at(self.src, start) {
            Ok(expression) => {
                self.at = expression.end();
                self.unit_parsed(expression.span(), Mode::Expression, "the expression");
                Ok(expression)
            }
            Err(errors) => self.temple_fail(&errors, start, Mode::Expression),
        }
    }

    /// A temple unit parsed at `span`, which reads in `mode`; `what` names it
    /// in messages. One too large for temple to format is an error, since
    /// the flow hash formats it.
    fn unit_parsed(&mut self, span: Span, mode: Mode, what: &str) {
        let len = span.range().len();
        if len > MAX_UNIT_BYTES {
            self.diagnostics.push(Diagnostic::error(
                span,
                too_large(what, len, MAX_UNIT_BYTES),
            ));
        }
        if self.refuse_comments {
            self.lex_unit(span, mode);
        }
    }

    /// Temple's errors for the unit at `at`, which reads in `mode`. For an
    /// error where the text ran out, the lexer finds where the unit ends.
    fn temple_fail<T>(&mut self, errors: &[CompileError], at: usize, mode: Mode) -> ParseResult<T> {
        let first_diagnostic = self.diagnostics.len();
        let fallback = Span::new(at, self.line_end(at));
        for error in errors {
            self.diagnostics
                .push(from_temple(error, fallback, self.src));
        }
        let end = self.bytes.len();
        let ran_out = self.diagnostics[first_diagnostic..]
            .iter()
            .any(|diagnostic| diagnostic.span.start as usize >= end);
        if ran_out || self.refuse_comments {
            let lexed = self.lex_unit(Span::new(at, end), mode);
            if let Some(last) = lexed.lexemes.last().filter(|_| ran_out) {
                let last_end = last.start + last.text.len();
                // a string left open runs to the end: point where it opens
                let open_string = last.kind == LexemeKind::Text && last_end == end;
                self.token_end = if open_string { last.start } else { last_end };
            }
        }
        Err(Fail)
    }

    /// `(a, b)` of names.
    pub(super) fn param_list(&mut self) -> ParseResult<Vec<Identifier>> {
        self.parenthesized(
            "`(`",
            |parser| parser.word("a parameter name"),
            "expected `,` or `)`",
        )
    }

    /// `(expression, expression)` for jumps and ask continuations.
    fn arg_list(&mut self) -> ParseResult<Vec<ParsedExpr>> {
        self.skip_spaces();
        self.parenthesized(
            "`(` and the arguments",
            Self::expression,
            "expected `,` or `)` in the argument list",
        )
    }

    /// `(item, item)`, each item read by `item`. `open` names the `(` in the
    /// error when it is missing, and `after_item` is the error when an item
    /// is followed by neither `,` nor `)`.
    fn parenthesized<T>(
        &mut self,
        open: &str,
        item: impl Fn(&mut Self) -> ParseResult<T>,
        after_item: &str,
    ) -> ParseResult<Vec<T>> {
        self.expect(b'(', open)?;
        let mut items = Vec::new();
        self.skip_trivia();
        if self.peek() == Some(b')') {
            self.at += 1;
            return Ok(items);
        }
        loop {
            self.skip_trivia();
            items.push(item(self)?);
            self.skip_trivia();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b')') => {
                    self.at += 1;
                    return Ok(items);
                }
                _ => return self.fail(self.char_at(self.at), after_item),
            }
        }
    }
}
