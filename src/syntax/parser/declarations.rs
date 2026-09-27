//! The top-level declarations other than nodes: `edition`, `flow`, `entry`,
//! `events`, `param`, `let`, and the annotations above them.

use super::{ParseResult, Parser};
use crate::common::diagnostic::Diagnostic;
use crate::common::error::twice;
use crate::syntax::ast::{
    Annotation, FlowAst, Identifier, LetDeclaration, NameList, ParamDeclaration, EDITIONS,
    LATEST_EDITION,
};
use temple_dsl::Span;

/// Where a `let` is declared. Only a top-level one can be a helper.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LetPlace {
    TopLevel,
    Node,
}

impl Parser<'_> {
    pub(super) fn edition(&mut self, ast: &mut FlowAst, first: bool) -> ParseResult<()> {
        let keyword = self.keyword("edition")?;
        self.skip_spaces();
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let span = Span::new(start, self.at);
        let Ok(year) = self.src[start..self.at].parse::<u32>() else {
            return self.fail(
                span,
                format!("expected an edition year, such as `edition {LATEST_EDITION}`"),
            );
        };
        if !first {
            self.diagnostics.push(Diagnostic::error(
                keyword,
                "`edition` must be the first line of the file",
            ));
        }
        if ast.edition.is_some() {
            self.diagnostics
                .push(Diagnostic::error(keyword, twice("declaration", "edition")));
        }
        if !EDITIONS.contains(&year) {
            self.diagnostics.push(
                Diagnostic::error(span, format!("`edition {year}` isn't supported")).with_help(
                    format!("this version understands `edition {LATEST_EDITION}`"),
                ),
            );
        }
        ast.edition = Some(year);
        Ok(())
    }

    pub(super) fn flow_declaration(
        &mut self,
        ast: &mut FlowAst,
        annotations: Vec<Annotation>,
    ) -> ParseResult<()> {
        let keyword = self.keyword("flow")?;
        self.skip_spaces();
        let name = self.dotted_name("a flow name")?;
        if ast.name.is_some() {
            self.diagnostics
                .push(Diagnostic::error(keyword, twice("declaration", "flow")));
            return Ok(());
        }
        ast.flow_span = Some(Span::new(keyword.start as usize, name.span.end as usize));
        ast.name = Some(name);
        ast.annotations = annotations;
        Ok(())
    }

    /// `entry a, b` or `events a, b.c`: `keyword`, then names that `name`
    /// reads and `what` describes.
    pub(super) fn name_list(
        &mut self,
        keyword: &str,
        what: &str,
        name: fn(&mut Self, &str) -> ParseResult<Identifier>,
    ) -> ParseResult<NameList> {
        let start = self.keyword(keyword)?.start as usize;
        let mut names = Vec::new();
        loop {
            self.skip_spaces();
            names.push(name(self, what)?);
            self.skip_spaces();
            if self.peek() != Some(b',') {
                break;
            }
            let comma = self.at;
            self.at += 1;
            self.skip_trivia();
            if self.at >= self.bytes.len() {
                return self.fail(
                    Span::new(comma, comma + 1),
                    format!("expected {what} after `,`"),
                );
            }
        }
        let end = names.last().map_or(start, |last| last.span.end as usize);
        Ok(NameList {
            names,
            span: Span::new(start, end),
        })
    }

    pub(super) fn annotation(&mut self) -> ParseResult<Annotation> {
        let start = self.at;
        self.at += "@".len();
        if self.peek_word().is_none() {
            return self.fail(self.char_at(start), "expected an annotation name after `@`");
        }
        let name = self.word("an annotation name")?;
        self.skip_spaces();
        let value = if self.peek() == Some(b'=') {
            self.at += 1;
            Some(self.expression()?)
        } else {
            None
        };
        let end = value
            .as_ref()
            .map_or(name.span.end as usize, |value| value.end());
        Ok(Annotation {
            name,
            value,
            span: Span::new(start, end),
        })
    }

    pub(super) fn param(&mut self, annotations: Vec<Annotation>) -> ParseResult<ParamDeclaration> {
        let start = self.keyword("param")?.start as usize;
        self.skip_spaces();
        let name = self.word("a param name")?;
        self.skip_spaces();
        self.expect(
            b'=',
            "`=` and a default value, as in `param windowDays = 30`",
        )?;
        let default = self.expression()?;
        let end = default.end();
        Ok(ParamDeclaration {
            name,
            default,
            annotations,
            span: Span::new(start, end),
        })
    }

    /// `let name = expression` or, at the top level, `let name(a, b) = expression`.
    pub(super) fn let_declaration(&mut self, place: LetPlace) -> ParseResult<LetDeclaration> {
        let start = self.keyword("let")?.start as usize;
        self.skip_spaces();
        let name = self.word("a name")?;
        self.skip_spaces();
        let params = if self.peek() == Some(b'(') {
            let params = self.param_list()?;
            if place == LetPlace::Node {
                self.diagnostics.push(
                    Diagnostic::error(name.span, "helpers can only be declared at the top level")
                        .with_help("move this `let` above the first node"),
                );
            }
            Some(params)
        } else {
            None
        };
        self.skip_spaces();
        self.expect(b'=', "`=`")?;
        let expression = self.expression()?;
        let end = expression.end();
        Ok(LetDeclaration {
            name,
            params,
            expression,
            span: Span::new(start, end),
        })
    }
}
