//! A file's outline: its declarations by name, and how many branches each
//! node has. An edit must change the outline exactly as it says, which
//! catches one that spills over, such as a `#` hiding the branches after it.

use super::edits::{Declaration, Edit};
use crate::syntax::ast::{Annotation, FlowAst};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outline {
    flow: Option<SmolStr>,
    flow_annotations: Vec<SmolStr>,
    params: Vec<Declared>,
    lets: Vec<SmolStr>,
    nodes: Vec<Declared>,
    events: Vec<SmolStr>,
    entries: Vec<SmolStr>,
}

/// A param or node: its name, its annotations' names, and for a node how
/// many branches it has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Declared {
    name: SmolStr,
    annotations: Vec<SmolStr>,
    branches: usize,
}

impl Outline {
    pub(super) fn of(ast: &FlowAst) -> Outline {
        Outline {
            flow: ast.name.as_ref().map(|name| name.name.clone()),
            flow_annotations: names_of(&ast.annotations),
            params: ast
                .params
                .iter()
                .map(|param| Declared {
                    name: param.name.name.clone(),
                    annotations: names_of(&param.annotations),
                    branches: 0,
                })
                .collect(),
            lets: ast
                .lets
                .iter()
                .map(|binding| binding.name.name.clone())
                .collect(),
            nodes: ast
                .nodes
                .iter()
                .map(|node| Declared {
                    name: node.name.name.clone(),
                    annotations: names_of(&node.annotations),
                    branches: node.branch_count(),
                })
                .collect(),
            events: ast.event_names().map(|event| event.name.clone()).collect(),
            entries: ast.entry_names().map(|entry| entry.name.clone()).collect(),
        }
    }

    /// This outline changed as `edit` says. `edited` is the file the edit
    /// made, for what the edit leaves to the text: where a new node lands,
    /// after its anchor's line, and its branches. `None` when something the
    /// edit names is missing.
    pub(super) fn after(mut self, edit: &Edit, edited: &FlowAst) -> Option<Outline> {
        match edit {
            Edit::AddNode { name, .. } => {
                let at = edited
                    .nodes
                    .iter()
                    .position(|node| node.name.name == name.as_str())?;
                if at > self.nodes.len() {
                    return None;
                }
                self.nodes.insert(
                    at,
                    Declared {
                        name: name.into(),
                        annotations: Vec::new(),
                        branches: edited.nodes[at].branch_count(),
                    },
                );
            }
            Edit::RemoveNode { name } => {
                let at = position(&self.nodes, name)?;
                self.nodes.remove(at);
            }
            Edit::RenameNode { from, to } => {
                let at = position(&self.nodes, from)?;
                self.nodes[at].name = to.into();
                for entry in &mut self.entries {
                    if entry == from {
                        *entry = to.into();
                    }
                }
            }
            Edit::AddBranch { node, .. } => {
                let at = position(&self.nodes, node)?;
                self.nodes[at].branches += 1;
            }
            Edit::RemoveBranch { node, .. } => {
                let at = position(&self.nodes, node)?;
                self.nodes[at].branches = self.nodes[at].branches.checked_sub(1)?;
            }
            Edit::MoveBranch { .. } | Edit::SetGuard { .. } | Edit::SetOutcome { .. } => {}
            Edit::SetAnnotation { on, name, .. } => {
                let annotations = self.annotations(on)?;
                if !annotations.iter().any(|annotation| annotation == name) {
                    annotations.push(name.into());
                }
            }
            Edit::RemoveAnnotation { on, name } => {
                let annotations = self.annotations(on)?;
                remove_first(annotations, name)?;
            }
            Edit::AddParam { name, .. } => self.params.push(Declared {
                name: name.into(),
                annotations: Vec::new(),
                branches: 0,
            }),
            Edit::RemoveParam { name } => {
                let at = position(&self.params, name)?;
                self.params.remove(at);
            }
            Edit::AddEvent { name } => self.events.push(name.into()),
            Edit::RemoveEvent { name } => remove_first(&mut self.events, name)?,
            Edit::AddEntry { node } => self.entries.push(node.into()),
            Edit::RemoveEntry { node } => remove_first(&mut self.entries, node)?,
        }
        Some(self)
    }

    fn annotations(&mut self, on: &Declaration) -> Option<&mut Vec<SmolStr>> {
        match on {
            Declaration::Flow => Some(&mut self.flow_annotations),
            Declaration::Param(name) => {
                let at = position(&self.params, name)?;
                Some(&mut self.params[at].annotations)
            }
            Declaration::Node(name) => {
                let at = position(&self.nodes, name)?;
                Some(&mut self.nodes[at].annotations)
            }
        }
    }
}

fn names_of(annotations: &[Annotation]) -> Vec<SmolStr> {
    annotations
        .iter()
        .map(|annotation| annotation.name.name.clone())
        .collect()
}

fn position(declared: &[Declared], name: &str) -> Option<usize> {
    declared.iter().position(|declared| declared.name == name)
}

fn remove_first(names: &mut Vec<SmolStr>, name: &str) -> Option<()> {
    let at = names.iter().position(|listed| listed == name)?;
    names.remove(at);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax;

    fn parsed(src: &str) -> FlowAst {
        syntax::parse(src).0
    }

    #[test]
    fn an_edit_that_changes_more_than_it_says_shows() {
        let before = parsed(
            "edition 2026\nflow f\nentry a\nnode a = when {\n  x: b(), y: c()\n  else: ignore\n}\n",
        );
        // as if the new outcome had hidden the branch after it
        let after =
            parsed("edition 2026\nflow f\nentry a\nnode a = when {\n  x: d()\n  else: ignore\n}\n");
        let edit = Edit::SetOutcome {
            node: "a".into(),
            index: 0,
            outcome: "d()".into(),
        };
        assert_ne!(
            Outline::of(&before).after(&edit, &after),
            Some(Outline::of(&after))
        );
        // the edit as it should land
        let landed = parsed(
            "edition 2026\nflow f\nentry a\nnode a = when {\n  x: d(), y: c()\n  else: ignore\n}\n",
        );
        assert_eq!(
            Outline::of(&before).after(&edit, &landed),
            Some(Outline::of(&landed))
        );
    }
}
