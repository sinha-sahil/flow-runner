//! Text helpers for edits: lines and indents, removing spans, values as
//! literals, and making the changes.

use super::Document;
use crate::common::error::{too_deep, EditError};
use crate::common::limits::MAX_LITERAL_DEPTH;
use crate::syntax::ast::Annotation;
use std::ops::Range;
use temple_dsl::{Span, Value};

/// One text replacement: `start..end` becomes `text`.
#[derive(Debug)]
pub(super) struct Change {
    start: usize,
    end: usize,
    pub(super) text: String,
}

impl Change {
    pub(super) fn insert(at: usize, text: String) -> Change {
        Change {
            start: at,
            end: at,
            text,
        }
    }

    pub(super) fn replace(span: Span, text: impl Into<String>) -> Change {
        let range = span.range();
        Change {
            start: range.start,
            end: range.end,
            text: text.into(),
        }
    }

    pub(super) fn remove(start: usize, end: usize) -> Change {
        Change {
            start,
            end,
            text: String::new(),
        }
    }
}

impl Document {
    pub(super) fn slice(&self, span: Span) -> &str {
        &self.text[span.range()]
    }

    /// Insert whole lines at `at`: a line start, or the end of a file whose
    /// last line has no line break, which then gets one first.
    pub(super) fn insert_lines(&self, at: usize, lines: String) -> Change {
        if at > 0 && !self.text[..at].ends_with('\n') {
            return Change::insert(at, format!("\n{lines}"));
        }
        Change::insert(at, lines)
    }

    /// Where the line holding `at` starts.
    pub(super) fn line_start(&self, at: usize) -> usize {
        self.text[..at]
            .rfind('\n')
            .map_or(0, |line_break| line_break + 1)
    }

    /// Just past the line break that ends the line holding `at`.
    pub(super) fn line_after(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |line_break| at + line_break + 1)
    }

    /// Whether only spaces come before `at` on its line.
    pub(super) fn starts_line(&self, at: usize) -> bool {
        self.text[self.line_start(at)..at]
            .bytes()
            .all(|byte| byte == b' ' || byte == b'\t')
    }

    /// The leading spaces of the line holding `at`.
    pub(super) fn indent_at(&self, at: usize) -> &str {
        let line = &self.text[self.line_start(at)..];
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        &line[..indent]
    }

    /// Where the text of the line holding `at` ends, before its line break.
    pub(super) fn line_end(&self, at: usize) -> usize {
        let end = self.line_after(at);
        let text = &self.text[..end];
        let text = text.strip_suffix('\n').unwrap_or(text);
        text.strip_suffix('\r').unwrap_or(text).len().max(at)
    }

    /// Whether `a` and `b` are on the same line.
    pub(super) fn same_line(&self, a: usize, b: usize) -> bool {
        !self.text[a.min(b)..a.max(b)].contains('\n')
    }

    /// Just past the line holding `at`, and past every declaration that
    /// starts on it: where whole lines can go in after it.
    pub(super) fn next_free_line(&self, mut at: usize) -> usize {
        // in file order, so a chain of declarations sharing lines is walked
        // once, not rescanned per line
        let mut spans: Vec<Span> = self
            .declaration_spans()
            .filter(|span| span.end as usize > at)
            .collect();
        spans.sort_unstable_by_key(|span| span.start);
        let mut spans = spans.into_iter().peekable();
        loop {
            let (start, end) = (self.line_start(at), self.line_after(at));
            let mut beyond = None;
            while let Some(span) = spans.next_if(|span| (span.start as usize) < end) {
                if span.start as usize >= start && span.end as usize >= end {
                    beyond = beyond.max(Some(span.end as usize));
                }
            }
            match beyond {
                Some(span_end) if span_end > at => at = span_end,
                _ => return end,
            }
        }
    }

    /// The span of every top-level declaration.
    fn declaration_spans(&self) -> impl Iterator<Item = Span> + '_ {
        let ast = &self.ast;
        ast.flow_span
            .into_iter()
            .chain(ast.entries.iter().chain(&ast.events).map(|list| list.span))
            .chain(ast.params.iter().map(|param| param.span))
            .chain(ast.lets.iter().map(|binding| binding.span))
            .chain(ast.nodes.iter().map(|node| node.span))
    }

    /// The line break the file uses: `\r\n` when it has any, else `\n`.
    pub(super) fn line_break(&self) -> &'static str {
        if self.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// Whether nothing but spaces, a comma and a comment follow `at` on its
    /// line.
    pub(super) fn ends_line(&self, at: usize) -> bool {
        let rest = &self.text[at..self.line_after(at)];
        let rest = rest.trim_start_matches([' ', '\t']);
        let rest = rest
            .strip_prefix(',')
            .unwrap_or(rest)
            .trim_start_matches([' ', '\t']);
        rest.is_empty()
            || rest.starts_with('#')
            || rest.starts_with('\n')
            || rest.starts_with("\r\n")
    }

    /// Remove a span: its whole lines when it has them to itself, otherwise
    /// just the span and a separating comma.
    pub(super) fn remove_span(&self, span: Span) -> Change {
        let Range { start, end } = span.range();
        if self.starts_line(start) && self.ends_line(end) {
            return Change::remove(self.line_start(start), self.line_after(end));
        }
        let after = &self.text[end..];
        if let Some(rest) = after.trim_start_matches([' ', '\t']).strip_prefix(',') {
            let gap = after.len() - rest.trim_start_matches([' ', '\t']).len();
            return Change::remove(start, end + gap);
        }
        let before = self.text[..start].trim_end_matches([' ', '\t']);
        let start = before.strip_suffix(',').map_or(start, str::len);
        Change::remove(start, end)
    }

    /// Remove a declaration with its annotations, and one blank line so the
    /// spacing around it stays even.
    pub(super) fn remove_declaration(&self, annotations: &[Annotation], span: Span) -> Change {
        let start = annotations.first().map_or(span.start, |annotation| {
            annotation.span.start.min(span.start)
        });
        let mut change = self.remove_span(Span { start, ..span });
        let before = &self.text[..change.start];
        let blank_line = ["\r\n\r\n", "\n\r\n", "\n\n"]
            .into_iter()
            .find(|blank| before.ends_with(blank));
        let after = &self.text[change.end..];
        let blank_after = after.starts_with('\n') || after.starts_with("\r\n") || after.is_empty();
        if let Some(blank) = blank_line {
            if blank_after && self.starts_line(change.start) {
                // the line break that ends the blank line
                change.start -= if blank.ends_with("\r\n") { 2 } else { 1 };
            }
        }
        change
    }
}

/// `text` with `changes` made, in one pass. Changes don't overlap; two at
/// the same place are made in the order given.
pub(super) fn splice(text: &str, mut changes: Vec<Change>) -> String {
    changes.sort_by_key(|change| (change.start, change.end));
    let added: usize = changes.iter().map(|change| change.text.len()).sum();
    let mut out = String::with_capacity(text.len() + added);
    let mut at = 0;
    for change in &changes {
        let start = change.start.max(at);
        out.push_str(&text[at..start]);
        out.push_str(&change.text);
        at = change.end.max(start);
    }
    out.push_str(&text[at..]);
    out
}

/// A value as a temple literal, as it would be written in a flow.
pub(super) fn literal_text(value: &Value) -> Result<String, EditError> {
    literal_at(value, 0)
}

fn literal_at(value: &Value, depth: usize) -> Result<String, EditError> {
    if depth > MAX_LITERAL_DEPTH {
        return Err(EditError::Invalid(too_deep("the value", MAX_LITERAL_DEPTH)));
    }
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(flag) => flag.to_string(),
        Value::Int(number) => number.to_string(),
        // a decimal needs a fraction, or it reads back as an integer
        Value::Decimal(number) if number.scale() == 0 => format!("{number}.0"),
        Value::Decimal(number) => number.to_string(),
        Value::Str(text) => quoted(text, '\''),
        Value::Arr(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| literal_at(item, depth + 1))
                .collect::<Result<_, _>>()?;
            format!("[{}]", items.join(", "))
        }
        Value::Obj(object) if object.is_empty() => "{}".into(),
        Value::Obj(object) => {
            let entries: Vec<String> = object
                .iter()
                .map(|(key, item)| {
                    Ok(format!(
                        "{}: {}",
                        quoted(key, '"'),
                        literal_at(item, depth + 1)?
                    ))
                })
                .collect::<Result<_, EditError>>()?;
            format!("{{ {} }}", entries.join(", "))
        }
    })
}

/// `text` between `quote`s, escaped.
fn quoted(text: &str, quote: char) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ if character == quote => {
                out.push('\\');
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out.push(quote);
    out
}
