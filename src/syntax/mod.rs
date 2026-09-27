//! `.flow` text to a tree: the tree is [`ast`], and [`parser`] builds it.
//! [`names`] holds the rule for names, and [`lexer`] splits the temple
//! units inside the tree into lexemes.

pub(crate) mod ast;
pub(crate) mod lexer;
pub(crate) mod names;
mod parser;

pub(crate) use parser::{parse, parse_guard, parse_node, parse_outcome};
