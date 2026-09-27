//! The one rule for names. A word is `[A-Za-z_][A-Za-z0-9_]*`; a dotted name,
//! for flows, events and asks, is a word followed by runs of letters, digits
//! and `_`, each after a single `.` or `-`. A name a flow declares for its
//! expressions to read must also start with a letter, as temple's own names
//! do, and be neither a keyword nor a builtin. The parser reads any word, so
//! that [`name_problem`] can say what is wrong.

use crate::common::error::{too_large, TICK_DECLARED};
use crate::common::limits::MAX_NAME_BYTES;
use crate::syntax::ast::{KEYWORDS, TICK};
use temple_dsl::{is_builtin_function, is_reserved_name};

pub(super) fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The runs of word characters in `text`: every name it could spell.
pub(crate) fn word_runs(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !u8::try_from(c).is_ok_and(is_word_char))
        .filter(|run| !run.is_empty())
}

/// Bytes of the word at the start of `bytes`; 0 when there is none.
pub(crate) fn word_len(bytes: &[u8]) -> usize {
    match bytes.first() {
        Some(&first) if is_word_start(first) => {
            1 + bytes[1..]
                .iter()
                .take_while(|&&byte| is_word_char(byte))
                .count()
        }
        _ => 0,
    }
}

/// Bytes of the dotted name at the start of `bytes`; 0 when there is none.
pub(super) fn dotted_len(bytes: &[u8]) -> usize {
    if word_len(bytes) == 0 {
        return 0;
    }
    let mut len = 1;
    while let Some(&byte) = bytes.get(len) {
        let joins_a_word = matches!(byte, b'.' | b'-')
            && bytes.get(len + 1).is_some_and(|&next| is_word_char(next));
        if !is_word_char(byte) && !joins_a_word {
            break;
        }
        len += 1;
    }
    len
}

/// Why a name is too long, if it is.
pub(super) fn length_problem(name: &str) -> Option<String> {
    (name.len() > MAX_NAME_BYTES).then(|| too_large("the name", name.len(), MAX_NAME_BYTES))
}

/// Why `name` can't name a node, param, let, helper or parameter, if it can't.
pub(crate) fn name_problem(name: &str) -> Option<String> {
    if let Some(problem) = length_problem(name) {
        return Some(problem);
    }
    let starts_with_letter = name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    if !starts_with_letter || word_len(name.as_bytes()) != name.len() {
        Some(format!(
            "`{name}` is not a valid name; use letters, digits and `_`, starting with a letter"
        ))
    } else if is_reserved_name(name) || KEYWORDS.contains(&name) {
        Some(format!("`{name}` is a reserved name"))
    } else if is_builtin_function(name) {
        Some(format!("`{name}` is a built-in function"))
    } else {
        None
    }
}

/// Why `name` can't name an event, if it can't.
pub(crate) fn event_problem(name: &str) -> Option<String> {
    if let Some(problem) = length_problem(name) {
        Some(problem)
    } else if dotted_len(name.as_bytes()) != name.len() {
        Some(format!(
            "`{name}` is not a valid event name; use letters, digits and `_`, with single `.` or `-` between them, starting with a letter or `_`"
        ))
    } else if name == TICK {
        Some(TICK_DECLARED.to_string())
    } else {
        None
    }
}

/// Why `name` can't name an annotation, if it can't. Any word will do,
/// keywords included.
pub(crate) fn annotation_problem(name: &str) -> Option<String> {
    if let Some(problem) = length_problem(name) {
        Some(problem)
    } else if word_len(name.as_bytes()) != name.len() {
        Some(format!(
            "`{name}` is not a valid annotation name; use letters, digits and `_`"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_dotted_names() {
        assert_eq!(word_len(b"total_1 + x"), 7);
        assert_eq!(word_len(b"_x"), 2);
        assert_eq!(word_len(b"1x"), 0);
        assert_eq!(dotted_len(b"order.v2-created: x"), 16);
        assert_eq!(dotted_len(b"a..b"), 1);
        assert_eq!(dotted_len(b"a.-b"), 1);
        assert_eq!(dotted_len(b"a. b"), 1);
        let runs: Vec<&str> = word_runs("a.b1 + _c(é)").collect();
        assert_eq!(runs, ["a", "b1", "_c"]);
    }

    #[test]
    fn declared_names_start_with_a_letter() {
        assert!(name_problem("total").is_none());
        assert!(name_problem("_total").is_some());
        assert!(name_problem("when").is_some());
        assert!(name_problem("input").is_some());
        assert!(name_problem(&"a".repeat(MAX_NAME_BYTES)).is_none());
        assert!(name_problem(&"a".repeat(MAX_NAME_BYTES + 1)).is_some());
        assert!(event_problem("order.v2-created").is_none());
        assert!(event_problem("tick").is_some());
        assert!(event_problem("a.").is_some());
        assert!(annotation_problem("when").is_none());
        assert!(annotation_problem("a-b").is_some());
    }
}
