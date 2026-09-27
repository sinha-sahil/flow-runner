//! The names in scope as a flow compiles, and the temple [`Names`] each unit
//! is checked against.

use crate::syntax::names::{name_problem, word_runs};
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use temple_dsl::Names;

/// What a name in scope stands for.
#[derive(Debug, Clone, Copy)]
enum Binding {
    Variable,
    Function { arity: usize },
}

/// Params, flow lets and helpers stay in scope once declared. A node's or
/// helper's own parameters and lets are its locals, cleared after it.
#[derive(Default)]
pub(super) struct Scope {
    globals: HashMap<SmolStr, Binding>,
    locals: HashSet<SmolStr>,
}

impl Scope {
    pub(super) fn add_variable(&mut self, name: &SmolStr) {
        self.add_global(name, Binding::Variable);
    }

    pub(super) fn add_function(&mut self, name: &SmolStr, arity: usize) {
        self.add_global(name, Binding::Function { arity });
    }

    fn add_global(&mut self, name: &SmolStr, binding: Binding) {
        if self.admits(name) {
            self.globals.insert(name.clone(), binding);
        }
    }

    pub(super) fn add_local(&mut self, name: &SmolStr) {
        if self.admits(name) {
            self.locals.insert(name.clone());
        }
    }

    /// Whether `name` can join the scope. A name declared twice, or reserved,
    /// is reported where it is declared and left out here, so each unit that
    /// uses it doesn't report it again.
    fn admits(&self, name: &str) -> bool {
        name_problem(name).is_none() && !self.globals.contains_key(name)
    }

    pub(super) fn clear_locals(&mut self) {
        // a new set, not `clear()`, which costs the old set's capacity again
        // for every node after a large one
        self.locals = HashSet::new();
    }

    /// The names in scope that `text` spells out: all a unit written as
    /// `text` can use. Checking against these rather than the whole scope
    /// keeps each check as cheap as the unit is short.
    pub(super) fn names_in(&self, text: &str) -> Names {
        let mut names = Names::new();
        for word in word_runs(text) {
            if self.locals.contains(word) {
                names.add_var(word);
            } else if let Some(binding) = self.globals.get(word) {
                add(&mut names, word, *binding);
            }
        }
        names
    }

    /// Every name in scope, as text.
    pub(super) fn names(&self) -> impl Iterator<Item = &str> + Clone {
        self.globals.keys().chain(&self.locals).map(SmolStr::as_str)
    }

    /// Every name in scope, so a did-you-mean can suggest any of them.
    pub(super) fn all_names(&self) -> Names {
        let mut names = Names::new();
        for (name, binding) in &self.globals {
            add(&mut names, name, *binding);
        }
        for name in &self.locals {
            names.add_var(name.clone());
        }
        names
    }
}

fn add(names: &mut Names, name: &str, binding: Binding) {
    match binding {
        Binding::Variable => names.add_var(name),
        Binding::Function { arity } => names.add_function(name, arity),
    }
}
