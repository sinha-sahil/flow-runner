//! Where expressions test `input.event`: a test against a name the flow
//! doesn't declare is a typo, and a jump whose guard needs an event can't
//! repeat within one run, because the first jump uses the event up.
//!
//! Both read a unit's lexemes, not temple's tree, so both lean the safe way:
//! a guard needs an event only when it plainly does, and only tests plainly
//! against a literal are checked.

use super::literal;
use super::Compiler;
use crate::common::diagnostic::Diagnostic;
use crate::common::error::kind_phrase;
use crate::syntax::ast::TICK;
use crate::syntax::lexer::{lex, Lexeme, LexemeKind, Mode};
use indexmap::IndexSet;
use smol_str::SmolStr;
use temple_dsl::{Span, Value};

/// Whether the guard at `span` can only hold while an event is delivered:
/// it is `input.event == '<name>'`, or an `&&` chain with that as one link.
pub(super) fn guard_needs_event(src: &str, span: Span) -> bool {
    mentions_event(src, span) && requires_event(&lex(src, span, Mode::Expression).lexemes)
}

/// A unit that never spells `event` can't test `input.event`, and needn't be
/// lexed.
fn mentions_event(src: &str, span: Span) -> bool {
    src[span.range()].contains("event")
}

fn requires_event(guard: &[Lexeme]) -> bool {
    let guard = without_parens(guard);
    // with `||`, `??` or `? :` on top no one link has to hold, and a `let`
    // on top binds everything after it, links included
    let loose = top_level(guard)
        .any(|(_, lexeme)| matches!(lexeme.text, "||" | "??" | "?") || is_let(lexeme));
    if loose {
        return false;
    }
    let links = split_top_level(guard, "&&");
    if links.len() > 1 {
        return links.into_iter().any(requires_event);
    }
    event_test(guard)
        .is_some_and(|test| test.operator == "==" && test.value.kind == LexemeKind::Text)
}

fn is_let(lexeme: &Lexeme) -> bool {
    lexeme.kind == LexemeKind::Word && lexeme.is("let")
}

/// Lexemes in an event test: `input`, `.`, `event`, the operator and the
/// value.
const TEST_LEN: usize = 5;

/// `input.event == <literal>` or `!=`, either way round.
struct EventTest<'l, 's> {
    operator: &'s str,
    value: &'l Lexeme<'s>,
}

/// The test, when `lexemes` are exactly one.
fn event_test<'l, 's>(lexemes: &'l [Lexeme<'s>]) -> Option<EventTest<'l, 's>> {
    let (operator, value) = match lexemes {
        [input, dot, event, operator, value] if is_event_read(input, dot, event) => {
            (operator, value)
        }
        [value, operator, input, dot, event] if is_event_read(input, dot, event) => {
            (operator, value)
        }
        _ => return None,
    };
    let is_test = matches!(operator.text, "==" | "!=") && value.is_literal();
    is_test.then_some(EventTest {
        operator: operator.text,
        value,
    })
}

fn is_event_read(input: &Lexeme, dot: &Lexeme, event: &Lexeme) -> bool {
    input.is("input") && (dot.is(".") || dot.is("?.")) && event.is("event")
}

/// Every literal `input.event` is tested against in `lexemes`, as long as
/// nothing around the test binds tighter: in `input.event == 'a' + 'b'` the
/// test is against `'a' + 'b'`.
fn tested_values<'l, 's>(lexemes: &'l [Lexeme<'s>]) -> impl Iterator<Item = &'l Lexeme<'s>> {
    lexemes
        .windows(TEST_LEN)
        .enumerate()
        .filter_map(move |(at, window)| {
            let test = event_test(window)?;
            let before = at.checked_sub(1).map(|index| &lexemes[index]);
            let after = lexemes.get(at + TEST_LEN);
            let bound_tighter =
                before.is_some_and(takes_what_follows) || after.is_some_and(takes_what_precedes);
            (!bound_tighter).then_some(test.value)
        })
}

/// Whether a lexeme before a test would take its first operand as its own,
/// as `+` does in `x + input.event == 'a'`: an operator that binds at least
/// as tightly as `==`, which groups left, or a unary one.
fn takes_what_follows(lexeme: &Lexeme) -> bool {
    matches!(
        lexeme.text,
        "==" | "!=" | "<" | "<=" | ">" | ">=" | "+" | "-" | "*" | "/" | "%" | "!" | "." | "?."
    )
}

/// Whether a lexeme after a test would take its last operand as its own, as
/// `+` does in `input.event == 'a' + 'b'`: an operator that binds more
/// tightly than `==`, or a postfix one.
fn takes_what_precedes(lexeme: &Lexeme) -> bool {
    matches!(
        lexeme.text,
        "<" | "<=" | ">" | ">=" | "+" | "-" | "*" | "/" | "%" | "." | "?." | "[" | "("
    )
}

/// The lexemes outside any bracket, each with its position in `lexemes`.
fn top_level<'l, 's>(lexemes: &'l [Lexeme<'s>]) -> impl Iterator<Item = (usize, &'l Lexeme<'s>)> {
    let mut depth = 0usize;
    lexemes.iter().enumerate().filter(move |(_, lexeme)| {
        let outside = depth == 0;
        match lexeme.text {
            "(" | "[" | "{" | "{{" => depth += 1,
            ")" | "]" | "}" | "}}" => depth = depth.saturating_sub(1),
            _ => return outside,
        }
        false
    })
}

/// `lexemes` without brackets that wrap all of them: `((a))` is `a`.
fn without_parens<'l, 's>(mut lexemes: &'l [Lexeme<'s>]) -> &'l [Lexeme<'s>] {
    while lexemes.len() >= 2
        && lexemes[0].is("(")
        && closing_bracket(lexemes) == Some(lexemes.len() - 1)
    {
        lexemes = &lexemes[1..lexemes.len() - 1];
    }
    lexemes
}

/// Where the bracket at the start of `lexemes` is closed.
fn closing_bracket(lexemes: &[Lexeme]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, lexeme) in lexemes.iter().enumerate() {
        match lexeme.text {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// `lexemes` cut at each top-level `separator`.
fn split_top_level<'l, 's>(lexemes: &'l [Lexeme<'s>], separator: &str) -> Vec<&'l [Lexeme<'s>]> {
    let mut parts = Vec::new();
    let mut from = 0;
    for (index, lexeme) in top_level(lexemes) {
        if lexeme.is(separator) {
            parts.push(&lexemes[from..index]);
            from = index + 1;
        }
    }
    parts.push(&lexemes[from..]);
    parts
}

impl Compiler<'_> {
    /// `input.event` may only be tested against declared events. `units` are
    /// the spans of the flow's expressions and values, each with how it reads.
    pub(super) fn check_event_tests(
        &mut self,
        units: impl IntoIterator<Item = (Span, Mode)>,
        events: &IndexSet<SmolStr>,
    ) {
        let src = self.src;
        for (unit, mode) in units
            .into_iter()
            .filter(|&(unit, _)| mentions_event(src, unit))
        {
            let lexed = lex(src, unit, mode);
            for value in tested_values(&lexed.lexemes) {
                match &literal::of_lexeme(value) {
                    Some(Value::Str(name)) if name == TICK || events.contains(name.as_str()) => {}
                    Some(Value::Str(name)) => self.undeclared_event(value, name, events),
                    // `input.event == null` holds exactly when no event came
                    Some(Value::Null) | None => {}
                    Some(other) => self.error(
                        value.span(),
                        format!(
                            "events are names, so comparing `input.event` with {} never matches",
                            kind_phrase(other)
                        ),
                    ),
                }
            }
        }
    }

    fn undeclared_event(&mut self, value: &Lexeme, name: &str, events: &IndexSet<SmolStr>) {
        let diagnostic =
            Diagnostic::error(value.span(), format!("`{name}` is not a declared event"));
        let candidates = events.iter().map(SmolStr::as_str).chain([TICK]);
        let diagnostic = match self.hint(name, candidates) {
            Some(suggestion) => diagnostic.with_help(format!(
                "did you mean `{suggestion}`? events are declared with `events ...`"
            )),
            None => diagnostic.with_help(format!("declare it: `events {name}`")),
        };
        self.push(diagnostic);
    }
}
