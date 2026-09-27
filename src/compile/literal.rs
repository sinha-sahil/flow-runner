//! Literals: param defaults and annotation values are written out in full,
//! so tools can show and edit them.

use crate::syntax::lexer::{lex, Lexeme, LexemeKind, Mode};
use temple_dsl::{Budget, Env, ExprUnit, Names, ParsedExpr, Value};

/// The value of `parsed` when it is a literal: a number, string, boolean,
/// null, or a list or object of literals.
pub(crate) fn value(src: &str, parsed: &ParsedExpr) -> Option<Value> {
    if !is_literal(&lex(src, parsed.span(), Mode::Expression).lexemes) {
        return None;
    }
    evaluate(parsed.clone().check(&Names::new()).ok()?)
}

/// The value of one literal lexeme, read by temple so escapes mean what they
/// mean everywhere else.
pub(super) fn of_lexeme(lexeme: &Lexeme) -> Option<Value> {
    evaluate(ExprUnit::compile(lexeme.text, &Names::new()).ok()?)
}

/// A literal's value. It costs no more to evaluate than it is long, so it
/// needs no budget.
fn evaluate(unit: ExprUnit) -> Option<Value> {
    unit.eval(&Value::Null, &Env::new(), &Budget::unlimited())
        .ok()
}

/// Whether the lexemes of a parsed expression spell a literal. Temple has
/// already accepted the expression, so it is enough that every lexeme is one
/// a literal can hold.
fn is_literal(lexemes: &[Lexeme]) -> bool {
    let mut after_value = false;
    for (index, lexeme) in lexemes.iter().enumerate() {
        let next = lexemes.get(index + 1);
        let allowed = match lexeme.text {
            _ if lexeme.is_literal() => true,
            // after a value, `-` subtracts and `[` indexes
            "-" => !after_value && next.is_some_and(|next| next.kind == LexemeKind::Number),
            "[" => !after_value,
            "]" | "{" | "}" | "," | ":" => true,
            // an optional key: `{ "note"?: null }`
            "?" => next.is_some_and(|next| next.is(":")),
            _ => false,
        };
        if !allowed {
            return false;
        }
        after_value = lexeme.is_literal() || lexeme.is("]") || lexeme.is("}");
    }
    true
}
