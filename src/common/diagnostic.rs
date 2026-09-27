//! Diagnostics: errors and warnings with a source span, rendered as
//! underlined snippets.

use crate::common::error::{too_deep, too_large};
use crate::common::limits::{MAX_REPORT_LINE, REPORT_AFTER, REPORT_BEFORE};
use std::fmt;
use std::ops::{Deref, Range};
use temple_dsl::{CompileError as TempleCompileError, Span};

/// How serious a [`Diagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Severity {
    /// The flow doesn't compile.
    Error,
    /// The flow compiles, but something in it looks wrong.
    Warning,
}

impl Severity {
    /// `error` or `warning`.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One problem found in a `.flow` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Whether it stops the flow from compiling.
    pub severity: Severity,
    /// What is wrong, in one sentence.
    pub message: String,
    /// Where, as byte offsets into the source.
    pub span: Span,
    /// A short suggestion, shown under the snippet.
    pub help: Option<String>,
}

impl Diagnostic {
    pub(crate) fn error(span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            help: None,
        }
    }

    pub(crate) fn warning(span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            span,
            help: None,
        }
    }

    /// The same diagnostic with a suggestion.
    pub(crate) fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Whether this stops the flow from compiling.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// 1-based line and column of the span's start.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        self.span.line_col(src)
    }

    /// Render against the source as an underlined snippet:
    ///
    /// ```text
    /// error: unknown node `aproved`
    ///    --> 12:29
    ///    |
    /// 12 |   input.event == 'approve': aproved()
    ///    |                             ^^^^^^^
    ///    = help: did you mean `approved`?
    /// ```
    pub fn report(&self, src: &str) -> String {
        report_all(src, std::slice::from_ref(self))
    }

    /// The snippet for this diagnostic, whose span starts at `at`.
    fn render(&self, lines: &Lines, at: Placement) -> String {
        let src = lines.src;
        let span = self.span.range();
        let line = lines.line_range(at.line);
        let span_start = floor_boundary(src, span.start).clamp(line.start, line.end);
        let window = lines.window(&line, span_start);
        let lead = if window.start > line.start { "…" } else { "" };
        let tail = if window.end < line.end { "…" } else { "" };
        let text = &src[window.clone()];
        let indent = lead.chars().count() + src[window.start..span_start].chars().count();
        let span_end = floor_boundary(src, span.end).clamp(span_start, window.end);
        let width = src[span_start..span_end].chars().count().max(1);
        let line_number = (at.line + 1).to_string();
        let gutter = " ".repeat(line_number.len());
        let mut out = format!(
            "{label}: {message}\n{gutter} --> {line_number}:{column}\n{gutter} |\n{line_number} | {lead}{text}{tail}\n{gutter} | {spaces}{carets}",
            label = self.severity.as_str(),
            message = self.message,
            column = at.column,
            spaces = " ".repeat(indent),
            carets = "^".repeat(width),
        );
        if let Some(help) = &self.help {
            out.push_str(&format!("\n{gutter} = help: {help}"));
        }
        out
    }
}

/// Where a diagnostic's span starts: a 0-based line and a 1-based column,
/// counted in characters.
#[derive(Clone, Copy)]
struct Placement {
    line: usize,
    column: usize,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity.as_str(), self.message)
    }
}

/// Render several diagnostics, separated by blank lines.
fn report_all(src: &str, diagnostics: &[Diagnostic]) -> String {
    let lines = Lines::new(src);
    diagnostics
        .iter()
        .zip(placements(&lines, diagnostics))
        .map(|(diagnostic, at)| diagnostic.render(&lines, at))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Where each diagnostic's span starts. Columns count characters from the
/// line's start; taking the spans in order counts each line once, not once
/// per diagnostic.
fn placements(lines: &Lines, diagnostics: &[Diagnostic]) -> Vec<Placement> {
    let src = lines.src;
    let mut starts: Vec<SpanStart> = diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| SpanStart {
            offset: floor_boundary(src, diagnostic.span.range().start),
            index,
        })
        .collect();
    starts.sort_unstable_by_key(|start| (start.offset, start.index));
    let mut placements = vec![Placement { line: 0, column: 1 }; diagnostics.len()];
    let mut current_line = usize::MAX;
    let mut counted_to = 0;
    let mut chars = 0;
    for start in starts {
        let line = lines.line_of(start.offset);
        if line != current_line {
            current_line = line;
            counted_to = lines.starts[line];
            chars = 0;
        }
        chars += src[counted_to..start.offset].chars().count();
        counted_to = start.offset;
        placements[start.index] = Placement {
            line,
            column: chars + 1,
        };
    }
    placements
}

/// Where one diagnostic's span starts, and which diagnostic it is.
struct SpanStart {
    offset: usize,
    index: usize,
}

/// Where each line of a source starts, so placing many diagnostics doesn't
/// scan the source once per diagnostic.
struct Lines<'a> {
    src: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(src: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            src.bytes()
                .enumerate()
                .filter(|&(_, byte)| byte == b'\n')
                .map(|(offset, _)| offset + 1),
        );
        Lines { src, starts }
    }

    /// The 0-based line holding byte `at`.
    fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|&start| start <= at) - 1
    }

    /// A line's text as a byte range, without its line break.
    fn line_range(&self, line: usize) -> Range<usize> {
        let start = self.starts[line];
        let mut end = self
            .starts
            .get(line + 1)
            .map_or(self.src.len(), |&next_start| next_start - 1);
        if self.src[start..end].ends_with('\r') {
            end -= 1;
        }
        start..end
    }

    /// The part of `line` a report shows: all of it, or a window around
    /// `span_start` when the line is long.
    fn window(&self, line: &Range<usize>, span_start: usize) -> Range<usize> {
        if line.len() <= MAX_REPORT_LINE {
            return line.clone();
        }
        let from = span_start.saturating_sub(REPORT_BEFORE).max(line.start);
        let to = (span_start + REPORT_AFTER).min(line.end);
        floor_boundary(self.src, from)..floor_boundary(self.src, to)
    }
}

/// The largest char boundary at or before `at`, so a span into the middle
/// of a character still slices cleanly.
fn floor_boundary(src: &str, at: usize) -> usize {
    if at >= src.len() {
        return src.len();
    }
    let mut boundary = at;
    while !src.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// The smallest char boundary at or after `at`, or the end of `src`.
fn ceil_boundary(src: &str, at: usize) -> usize {
    let mut boundary = at.min(src.len());
    while !src.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

/// `span` held to `src` and widened to whole characters.
fn within(src: &str, span: Span) -> Span {
    let range = span.range();
    let start = floor_boundary(src, range.start);
    Span::new(start, ceil_boundary(src, range.end).max(start))
}

/// A temple compile error as a diagnostic, at its span held to `src`, the
/// text temple read, and widened to whole characters: temple can point just
/// past the end of its text, or into a character.
pub(crate) fn from_temple(error: &TempleCompileError, fallback: Span, src: &str) -> Diagnostic {
    match error {
        TempleCompileError::Syntax { message, span } => {
            Diagnostic::error(within(src, *span), message.clone())
        }
        TempleCompileError::TooDeep { limit, span } => {
            Diagnostic::error(within(src, *span), too_deep("the expression", *limit)).with_help(
                "split it up with `let`, or use a list method such as `.sum()` or `.any(...)`",
            )
        }
        TempleCompileError::TooLarge { bytes, limit } => {
            Diagnostic::error(fallback, too_large("the expression", *bytes, *limit))
        }
    }
}

/// The errors and warnings found in one source, in the order they were
/// found. [`Flow::compile`](crate::Flow::compile) fails with these when any
/// is an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Diagnostics(diagnostics)
    }

    /// Whether any of them is an error.
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }

    /// The errors, without the warnings.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|diagnostic| diagnostic.is_error())
    }

    /// Every diagnostic as a snippet against `src`, separated by blank lines.
    pub fn report(&self, src: &str) -> String {
        report_all(src, &self.0)
    }
}

impl Deref for Diagnostics {
    type Target = [Diagnostic];
    fn deref(&self) -> &[Diagnostic] {
        &self.0
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.errors().count();
        write!(
            f,
            "{count} error{} in flow",
            if count == 1 { "" } else { "s" }
        )?;
        for diagnostic in self.errors() {
            write!(
                f,
                "\n  {} (at {}..{})",
                diagnostic.message, diagnostic.span.start, diagnostic.span.end
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}
