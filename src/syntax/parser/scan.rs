//! Reading bytes and words: names, keywords, whitespace and comments, and
//! where declarations start, to bound units and to pick up after an error.

use super::{Fail, ParseResult, Parser};
use crate::common::diagnostic::Diagnostic;
use crate::syntax::ast::{Identifier, DECLARATIONS};
use crate::syntax::names::{dotted_len, length_problem, word_len};
use smol_str::SmolStr;
use temple_dsl::Span;

impl<'a> Parser<'a> {
    /// A word: node, param, let and annotation names.
    pub(super) fn word(&mut self, what: &str) -> ParseResult<Identifier> {
        let len = word_len(&self.bytes[self.at..]);
        self.name_of_len(len, what)
    }

    /// Words joined by `.` or `-`: flow, event and ask names.
    pub(super) fn dotted_name(&mut self, what: &str) -> ParseResult<Identifier> {
        let len = dotted_len(&self.bytes[self.at..]);
        self.name_of_len(len, what)
    }

    fn name_of_len(&mut self, len: usize, what: &str) -> ParseResult<Identifier> {
        if len == 0 {
            return self.fail(self.char_at(self.at), format!("expected {what}"));
        }
        let start = self.at;
        self.at += len;
        let identifier = Identifier {
            name: SmolStr::new(&self.src[start..self.at]),
            span: Span::new(start, self.at),
        };
        if let Some(problem) = length_problem(&identifier.name) {
            self.diagnostics
                .push(Diagnostic::error(identifier.span, problem));
        }
        Ok(identifier)
    }

    pub(super) fn keyword(&mut self, keyword: &str) -> ParseResult<Span> {
        if self.peek_word() == Some(keyword) {
            let start = self.at;
            self.at += keyword.len();
            Ok(Span::new(start, self.at))
        } else {
            let at = self.at.min(self.bytes.len());
            self.fail(Span::new(at, at), format!("expected `{keyword}`"))
        }
    }

    pub(super) fn expect(&mut self, byte: u8, what: &str) -> ParseResult<()> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            self.fail(self.char_at(self.at), format!("expected {what}"))
        }
    }

    /// The character at `at`, or an empty span at the end of the file: where
    /// the parser found something other than what it expected.
    pub(super) fn char_at(&self, at: usize) -> Span {
        let at = at.min(self.bytes.len());
        let width = self.src[at..].chars().next().map_or(0, char::len_utf8);
        Span::new(at, at + width)
    }

    pub(super) fn fail<T>(&mut self, span: Span, message: impl Into<String>) -> ParseResult<T> {
        self.diagnostics.push(Diagnostic::error(span, message));
        Err(Fail)
    }

    pub(super) fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// The word at the cursor, if any, without consuming it.
    pub(super) fn peek_word(&self) -> Option<&'a str> {
        self.word_at(self.at)
    }

    fn word_at(&self, at: usize) -> Option<&'a str> {
        let len = word_len(self.bytes.get(at..)?);
        (len > 0).then(|| &self.src[at..at + len])
    }

    /// Whether `else` and a `:` come next. A guard that only starts with
    /// the word `else` is an expression.
    pub(super) fn else_follows(&self) -> bool {
        let after = self.skip_spaces_from(self.at + "else".len());
        self.peek_word() == Some("else") && self.bytes.get(after) == Some(&b':')
    }

    /// Skip whitespace, line breaks and `#` comments.
    pub(super) fn skip_trivia(&mut self) {
        self.note_token_end();
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.at += 1;
            } else if byte == b'#' {
                self.comment.get_or_insert(self.at);
                self.at = self.line_end(self.at);
            } else {
                break;
            }
        }
        self.trivia_end = self.at;
    }

    /// Skip spaces and tabs only, staying on the line.
    pub(super) fn skip_spaces(&mut self) {
        self.note_token_end();
        self.at = self.skip_spaces_from(self.at);
        self.trivia_end = self.at;
    }

    /// The cursor moved on since trivia was last skipped, so past a token.
    fn note_token_end(&mut self) {
        if self.at > self.trivia_end {
            self.token_end = self.at;
        }
    }

    pub(super) fn skip_spaces_from(&self, from: usize) -> usize {
        let mut at = from;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        at
    }

    /// Where the line holding `from` ends, before its line break.
    pub(super) fn line_end(&self, from: usize) -> usize {
        let from = from.min(self.bytes.len());
        self.bytes[from..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(self.bytes.len(), |offset| from + offset)
    }

    /// Whether the line starting at `at` starts a top-level declaration: an
    /// annotation, or a declaration keyword in column 0. An indented `let`
    /// belongs to a node.
    pub(super) fn starts_declaration(&self, at: usize) -> bool {
        self.bytes.get(at) == Some(&b'@')
            || self
                .word_at(at)
                .is_some_and(|word| DECLARATIONS.contains(&word))
    }

    /// Where the declaration at `start` ends: at the next line that starts
    /// one, or at the end of the source.
    pub(super) fn declaration_end(&self, start: usize) -> usize {
        let next = self
            .declaration_lines
            .partition_point(|&line| line <= start);
        self.declaration_lines
            .get(next)
            .copied()
            .unwrap_or(self.whole.len())
    }
}
