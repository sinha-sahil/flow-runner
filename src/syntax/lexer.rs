//! Temple source split into lexemes, for what a flow asks of a unit that
//! temple doesn't answer: is a default a literal, which event does a guard
//! test, does an edit hide a comment. Units are lexed only once temple has
//! parsed them, so this follows temple's rules for valid source, and on
//! anything else merely stays in bounds.

use crate::syntax::ast::LITERAL_WORDS;
use crate::syntax::names::{is_word_start, word_len};
use temple_dsl::Span;

/// One lexeme. Whitespace and `#` comments are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Lexeme<'s> {
    pub kind: LexemeKind,
    pub text: &'s str,
    /// Where the lexeme starts in the whole source.
    pub start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexemeKind {
    /// A name or keyword: `total`, `input`, `true`.
    Word,
    /// Digits, with an optional fraction: `12`, `0.5`. A sign is a symbol.
    Number,
    /// A string with no holes, quotes included.
    Text,
    /// An operator or bracket, such as `==`, `?.` or `(`, or the `{{` and
    /// `}}` around a hole.
    Symbol,
}

impl Lexeme<'_> {
    pub(crate) fn span(&self) -> Span {
        Span::new(self.start, self.start + self.text.len())
    }

    pub(crate) fn is(&self, text: &str) -> bool {
        self.text == text
    }

    /// A number, a string, `true`, `false` or `null`.
    pub(crate) fn is_literal(&self) -> bool {
        match self.kind {
            LexemeKind::Number | LexemeKind::Text => true,
            LexemeKind::Word => LITERAL_WORDS.contains(&self.text),
            LexemeKind::Symbol => false,
        }
    }
}

/// How a unit reads. In an output value, `"` strings can hold `{{ }}`
/// holes, and only holes hold expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Expression,
    Output,
}

/// A unit's lexemes, and where its first comment is.
pub(crate) struct Lexed<'s> {
    pub lexemes: Vec<Lexeme<'s>>,
    pub comment: Option<usize>,
}

/// The lexemes of the unit at `span` in `src`.
pub(crate) fn lex(src: &str, span: Span, mode: Mode) -> Lexed<'_> {
    let end = (span.end as usize).min(src.len());
    let mut lexer = Lexer {
        src,
        at: (span.start as usize).min(end),
        end,
        lexed: Lexed {
            lexemes: Vec::new(),
            comment: None,
        },
    };
    match mode {
        Mode::Expression => lexer.expression(Until::UnitEnd),
        Mode::Output => lexer.output(),
    }
    lexer.lexed
}

/// The operators temple spells with two characters.
const TWO_CHARACTER_SYMBOLS: &[&[u8]] = &[
    b"==", b"!=", b"<=", b">=", b"&&", b"||", b"??", b"?.", b"->",
];

/// Where an expression being lexed ends: at the end of the unit, or at the
/// `}}` that closes its hole.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Until {
    UnitEnd,
    HoleEnd,
}

/// An object or list open in an output value.
enum Open {
    /// `key_next` holds while the next `"` string is a key, which has no
    /// holes.
    Object {
        key_next: bool,
    },
    List,
}

struct Lexer<'s> {
    src: &'s str,
    at: usize,
    end: usize,
    lexed: Lexed<'s>,
}

impl<'s> Lexer<'s> {
    fn rest(&self) -> &'s [u8] {
        &self.src.as_bytes()[self.at..self.end]
    }

    fn expression(&mut self, until: Until) {
        let mut depth = 0usize;
        loop {
            self.trivia();
            let rest = self.rest();
            let Some(&first) = rest.first() else {
                return;
            };
            if until == Until::HoleEnd && depth == 0 && rest.starts_with(b"}}") {
                return;
            }
            match first {
                b'\'' | b'"' => self.string(),
                _ if starts_number(rest) => self.number(),
                _ if is_word_start(first) => self.word(),
                b'(' | b'[' | b'{' => {
                    depth += 1;
                    self.symbol(1);
                }
                b')' | b']' | b'}' => {
                    depth = depth.saturating_sub(1);
                    self.symbol(1);
                }
                _ => self.operator(),
            }
        }
    }

    fn output(&mut self) {
        let mut open = Vec::new();
        // a finished value in an object means a key comes next
        let value_done = |open: &mut Vec<Open>| {
            if let Some(Open::Object { key_next }) = open.last_mut() {
                *key_next = true;
            }
        };
        loop {
            self.trivia();
            let rest = self.rest();
            let Some(&first) = rest.first() else {
                return;
            };
            if rest.starts_with(b"{{") {
                self.hole();
                value_done(&mut open);
                continue;
            }
            match first {
                b'{' => {
                    self.symbol(1);
                    open.push(Open::Object { key_next: true });
                }
                b'[' => {
                    self.symbol(1);
                    open.push(Open::List);
                }
                b'}' | b']' => {
                    self.symbol(1);
                    open.pop();
                    value_done(&mut open);
                }
                b'"' if matches!(open.last(), Some(Open::Object { key_next: true })) => {
                    self.string();
                    if let Some(Open::Object { key_next }) = open.last_mut() {
                        *key_next = false;
                    }
                }
                b'"' => {
                    self.interpolated();
                    value_done(&mut open);
                }
                b'\'' => {
                    self.string();
                    value_done(&mut open);
                }
                _ if starts_number(rest) => {
                    self.number();
                    value_done(&mut open);
                }
                _ if is_word_start(first) => {
                    self.word();
                    value_done(&mut open);
                }
                _ => self.operator(),
            }
        }
    }

    /// `{{ expression }}`.
    fn hole(&mut self) {
        self.symbol(2);
        self.expression(Until::HoleEnd);
        if self.rest().starts_with(b"}}") {
            self.symbol(2);
        }
    }

    /// Whitespace and comments.
    fn trivia(&mut self) {
        while let Some(&byte) = self.rest().first() {
            if byte.is_ascii_whitespace() {
                self.at += 1;
            } else if byte == b'#' {
                self.lexed.comment.get_or_insert(self.at);
                let rest = self.rest();
                self.at += rest
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .unwrap_or(rest.len());
            } else {
                break;
            }
        }
    }

    /// A quoted string without holes; a backslash escapes the byte after it.
    fn string(&mut self) {
        let start = self.at;
        let quote = self.rest()[0];
        let mut at = start + 1;
        while at < self.end {
            let byte = self.src.as_bytes()[at];
            at += if byte == b'\\' { 2 } else { 1 };
            if byte == quote {
                break;
            }
        }
        self.at = at.min(self.end);
        self.push(LexemeKind::Text, start);
    }

    /// A `"` string in an output value: its holes are lexed, its text is
    /// not. A string without holes is one [`LexemeKind::Text`].
    fn interpolated(&mut self) {
        let start = self.at;
        let lexemes_before = self.lexed.lexemes.len();
        self.at += 1;
        while let Some(&byte) = self.rest().first() {
            match byte {
                b'"' => {
                    self.at += 1;
                    break;
                }
                b'\\' => self.at = (self.at + 2).min(self.end),
                b'{' if self.rest().get(1) == Some(&b'{') => self.hole(),
                _ => self.at += 1,
            }
        }
        if self.lexed.lexemes.len() == lexemes_before {
            self.push(LexemeKind::Text, start);
        }
    }

    fn number(&mut self) {
        let start = self.at;
        let rest = self.rest();
        let digits = |from: usize| {
            rest[from..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count()
        };
        let mut len = digits(0);
        if rest.get(len) == Some(&b'.') && rest.get(len + 1).is_some_and(u8::is_ascii_digit) {
            len += 1 + digits(len + 1);
        }
        self.at += len;
        self.push(LexemeKind::Number, start);
    }

    fn word(&mut self) {
        let start = self.at;
        self.at += word_len(self.rest());
        self.push(LexemeKind::Word, start);
    }

    fn operator(&mut self) {
        let rest = self.rest();
        if TWO_CHARACTER_SYMBOLS
            .iter()
            .any(|symbol| rest.starts_with(symbol))
        {
            self.symbol(2);
        } else {
            let width = self.src[self.at..].chars().next().map_or(1, char::len_utf8);
            self.symbol(width);
        }
    }

    fn symbol(&mut self, len: usize) {
        let start = self.at;
        self.at += len;
        self.push(LexemeKind::Symbol, start);
    }

    fn push(&mut self, kind: LexemeKind, start: usize) {
        self.lexed.lexemes.push(Lexeme {
            kind,
            text: &self.src[start..self.at],
            start,
        });
    }
}

/// A digit, or a `.` before one, as in `-.5`.
fn starts_number(rest: &[u8]) -> bool {
    match rest {
        [first, ..] if first.is_ascii_digit() => true,
        [b'.', second, ..] => second.is_ascii_digit(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str, mode: Mode) -> Vec<&str> {
        let span = Span::new(0, src.len());
        lex(src, span, mode)
            .lexemes
            .iter()
            .map(|lexeme| lexeme.text)
            .collect()
    }

    #[test]
    fn expressions_split_into_lexemes() {
        assert_eq!(
            texts(
                "input?.event == 'a # b' && -.5 < x[0] # done",
                Mode::Expression
            ),
            ["input", "?.", "event", "==", "'a # b'", "&&", "-", ".5", "<", "x", "[", "0", "]"]
        );
        assert_eq!(
            texts("\"it's\" + 'say \\'hi\\''", Mode::Expression),
            ["\"it's\"", "+", "'say \\'hi\\''"]
        );
    }

    #[test]
    fn output_values_lex_only_their_holes() {
        assert_eq!(
            texts(
                r##"{ "m{{": "a {{ "#" }} b", "n": {{ input.event == 'go' }} }"##,
                Mode::Output
            ),
            [
                "{", "\"m{{\"", ":", "{{", "\"#\"", "}}", ",", "\"n\"", ":", "{{", "input", ".",
                "event", "==", "'go'", "}}", "}"
            ]
        );
        assert_eq!(
            texts(r#"["plain", {{ {"k": {"j": 1}} }}]"#, Mode::Output),
            [
                "[",
                "\"plain\"",
                ",",
                "{{",
                "{",
                "\"k\"",
                ":",
                "{",
                "\"j\"",
                ":",
                "1",
                "}",
                "}",
                "}}",
                "]"
            ]
        );
        assert_eq!(
            texts(r#""{{ "input.event == 5" }}""#, Mode::Output),
            ["{{", "\"input.event == 5\"", "}}"]
        );
    }

    #[test]
    fn comments_are_found_outside_strings_only() {
        let first = |src: &str, mode| lex(src, Span::new(0, src.len()), mode).comment;
        assert_eq!(first("a + '#' + b", Mode::Expression), None);
        assert_eq!(first("a + # note\n b", Mode::Expression), Some(4));
        assert_eq!(first("\"# {{ x # y\n }}\"", Mode::Output), Some(8));
        assert_eq!(first(r#"{ "a#": "b#" }"#, Mode::Output), None);
    }

    #[test]
    fn broken_units_stay_in_bounds() {
        for src in [
            "'open",
            "\"{{ x",
            "{{ \"é\\",
            "\"\\é",
            "é",
            "{ \"k\\",
            "[{{ }} }}]",
        ] {
            for mode in [Mode::Expression, Mode::Output] {
                lex(src, Span::new(0, src.len()), mode);
            }
        }
    }
}
