//! Temple units: each expression and value is checked against the names in
//! scope, and literals are read to values.

use super::scope::Scope;
use super::{hints, literal, Compiler};
use crate::common::diagnostic::from_temple;
use crate::common::error::too_large;
use crate::common::limits::MAX_NAME_BYTES;
use crate::syntax::lexer::{lex, Lexeme, LexemeKind, Mode};
use crate::syntax::names::{word_len, word_runs};
use temple_dsl::{
    CompileError, ExprUnit, Names, ParsedExpr, ParsedTemplate, Span, TemplateUnit, Value,
};

impl Compiler<'_> {
    pub(super) fn expression(&mut self, parsed: &ParsedExpr, scope: &Scope) -> Option<ExprUnit> {
        self.check_unit(parsed.span(), Mode::Expression, scope, |names| {
            parsed.clone().check(names)
        })
    }

    pub(super) fn template(
        &mut self,
        parsed: &ParsedTemplate,
        scope: &Scope,
    ) -> Option<TemplateUnit> {
        self.check_unit(parsed.span(), Mode::Output, scope, |names| {
            parsed.clone().check(names)
        })
    }

    /// Check the unit at `span`, which reads in `mode`, against the names it
    /// spells out. If that fails at a misspelt name, and hints can afford
    /// it, check it again against every name in scope, so temple's
    /// did-you-mean can suggest any of them.
    fn check_unit<Unit>(
        &mut self,
        span: Span,
        mode: Mode,
        scope: &Scope,
        check: impl Fn(&Names) -> Result<Unit, Vec<CompileError>>,
    ) -> Option<Unit> {
        let src = self.src;
        // temple compares a misspelt name with others character by
        // character, so a unit's names are held to a flow's limit first
        if let Some(name) = long_name(src, span, mode) {
            let message = too_large("the name", name.text.len(), MAX_NAME_BYTES);
            self.error(name.span(), message);
            return None;
        }
        let errors = match check(&scope.names_in(&src[span.range()])) {
            Ok(unit) => return Some(unit),
            Err(errors) => errors,
        };
        let misspelt: Vec<usize> = errors
            .iter()
            .filter_map(|error| misspelt_len(src, error))
            .collect();
        let recheck = !misspelt.is_empty()
            && self.afford_hints(misspelt.len(), || {
                recheck_cost(src, span, mode, &misspelt, scope)
            });
        let errors = if recheck {
            check(&scope.all_names()).err().unwrap_or(errors)
        } else {
            errors
        };
        self.temple_errors(&errors, span);
        None
    }

    pub(super) fn temple_errors(&mut self, errors: &[CompileError], fallback: Span) {
        for error in errors {
            self.push(from_temple(error, fallback, self.src));
        }
    }

    /// The value of `parsed`, an annotation value or a param default, which
    /// must be a literal.
    pub(super) fn literal(&mut self, parsed: &ParsedExpr, what: &str) -> Option<Value> {
        let value = literal::value(self.src, parsed);
        if value.is_none() {
            self.error(
                parsed.span(),
                format!(
                    "{what} must be a literal: a number, string, boolean, null, list or object"
                ),
            );
        }
        value
    }
}

/// The first name in the unit at `span` that is longer than a flow's names
/// may be.
fn long_name(src: &str, span: Span, mode: Mode) -> Option<Lexeme<'_>> {
    // a unit without so long a run of word characters needn't be lexed
    let has_long_run = word_runs(&src[span.range()]).any(|run| run.len() > MAX_NAME_BYTES);
    if !has_long_run {
        return None;
    }
    lex(src, span, mode)
        .lexemes
        .into_iter()
        .find(|lexeme| lexeme.kind == LexemeKind::Word && lexeme.text.len() > MAX_NAME_BYTES)
}

/// The length of the name an error points at, if it points at one: a name
/// temple didn't know, which it may suggest another for.
fn misspelt_len(src: &str, error: &CompileError) -> Option<usize> {
    let CompileError::Syntax { span, .. } = error else {
        return None;
    };
    let text = src.get(span.range())?;
    (!text.is_empty() && word_len(text.as_bytes()) == text.len()).then_some(text.len())
}

/// The most work, in table cells, temple's did-you-mean does when the unit
/// at `span` is checked against every name in scope and fails at misspelt
/// names of the given lengths. Each is compared with the names in scope,
/// the words and strings in the unit, which hold the names it binds and the
/// keys `this.` reads, and `input` and `this`; temple's two dozen short
/// builtins are too few to count.
fn recheck_cost(src: &str, span: Span, mode: Mode, misspelt: &[usize], scope: &Scope) -> u64 {
    let lexed = lex(src, span, mode);
    let in_unit = lexed
        .lexemes
        .iter()
        .filter(|lexeme| matches!(lexeme.kind, LexemeKind::Word | LexemeKind::Text))
        .map(|lexeme| lexeme.text);
    let candidates = scope.names().chain(in_unit).chain(["input", "this"]);
    misspelt
        .iter()
        .map(|&target| {
            candidates
                .clone()
                .map(|candidate| hints::cost(target, candidate.len(), hints::TEMPLE_EDITS))
                .sum::<u64>()
        })
        .sum()
}
