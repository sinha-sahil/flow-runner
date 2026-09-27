//! The rules a flow must follow once every node has compiled: entries and
//! events are well formed, asks continue at nodes that can take an answer,
//! jumps can't loop forever, and every node can be reached.

use super::Compiler;
use crate::common::diagnostic::Diagnostic;
use crate::common::error::{names_listed, twice, TICK_DECLARED};
use crate::flow::{Node, NodeId, Outcome};
use crate::syntax::ast::TICK;
use indexmap::IndexSet;
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use temple_dsl::{ExprUnit, InputRead, Span};

impl Compiler<'_> {
    /// The entry nodes, resolved. An entry must exist, be listed once, and
    /// take no parameters.
    pub(super) fn check_entries(&mut self, node_ids: &HashMap<SmolStr, NodeId>) -> Vec<NodeId> {
        let ast = self.ast;
        if ast.entry_names().next().is_none() {
            let span = ast.name.as_ref().map_or(Span::new(0, 0), |name| name.span);
            self.push(
                Diagnostic::error(span, "this flow has no `entry`")
                    .with_help("add `entry <node>` to say where a new run starts"),
            );
        }
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for entry in ast.entry_names() {
            let Some(&id) = node_ids.get(&entry.name) else {
                self.unknown_node(entry, node_ids);
                continue;
            };
            if !seen.insert(id) {
                self.error(entry.span, twice("entry", &entry.name));
                continue;
            }
            if !ast.nodes[id.index()].params.is_empty() {
                self.push(
                    Diagnostic::error(entry.span, format!("entry node `{}` takes parameters", entry.name))
                        .with_help("a run that starts here has nothing to fill them with; read `input.data` instead"),
                );
            }
            entries.push(id);
        }
        entries
    }

    /// The declared events, each once. `tick` is built in.
    pub(super) fn check_events(&mut self) -> IndexSet<SmolStr> {
        let mut events = IndexSet::new();
        for event in self.ast.event_names() {
            if event.name == TICK {
                self.push(Diagnostic::warning(event.span, TICK_DECLARED));
            } else if !events.insert(event.name.clone()) {
                self.error(event.span, twice("event", &event.name));
            }
        }
        events
    }

    /// The node an ask continues at is entered only by resuming that ask,
    /// and must move on from there. Only such a node can read the answer;
    /// `shared`, the flow lets and helpers, can't. Entries must already be
    /// marked.
    pub(super) fn check_asks<'u>(
        &mut self,
        nodes: &mut [Node],
        shared: impl Iterator<Item = &'u ExprUnit>,
    ) {
        let mut continued_at = vec![false; nodes.len()];
        let mut jumped_to = vec![false; nodes.len()];
        for branch in nodes.iter().flat_map(|node| &node.branches) {
            match &branch.outcome {
                Outcome::Ask { node: target, .. } => continued_at[target.index()] = true,
                Outcome::Jump { node: target, .. } => jumped_to[target.index()] = true,
                _ => {}
            }
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            node.resumes_ask = continued_at[index];
            if !node.resumes_ask {
                for read in node.reads().into_iter().filter(reads_answer) {
                    self.push(
                        Diagnostic::error(read.span, "`input.result` and `input.failure` only exist in the node an ask continues at")
                            .with_help(format!("node `{}` isn't the target of any `ask ... ->`", node.name)),
                    );
                }
                continue;
            }
            if node.is_entry {
                self.push(
                    Diagnostic::error(
                        node.span,
                        format!(
                            "node `{}` continues an ask, so it can't be an entry",
                            node.name
                        ),
                    )
                    .with_help("it only runs when the host resumes the ask with an answer"),
                );
            }
            if jumped_to[index] {
                self.push(
                    Diagnostic::error(
                        node.span,
                        format!(
                            "node `{}` continues an ask, so nothing can jump to it",
                            node.name
                        ),
                    )
                    .with_help("a jump has no answer to give it; jump to a node that asks instead"),
                );
            }
            for branch in &node.branches {
                if !matches!(branch.outcome, Outcome::Jump { .. } | Outcome::Ask { .. }) {
                    self.push(
                        Diagnostic::error(branch.span, format!("node `{}` continues an ask, so every branch must jump or ask again", node.name))
                            .with_help("the answer is gone on the next run; jump to a node that returns the value"),
                    );
                }
            }
        }
        for read in shared.flat_map(ExprUnit::reads).filter(reads_answer) {
            self.error(read.span, "flow-level lets and helpers can't read `input.result` or `input.failure`; read them in the node an ask continues at");
        }
    }

    /// A loop made only of jumps that need no event could run forever in one
    /// call. Events are used up by the first jump, so any loop that includes
    /// an event-guarded jump goes round at most once per run.
    pub(super) fn check_jump_loops(&mut self, nodes: &[Node]) {
        let mut jumps = vec![Vec::new(); nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            for branch in &node.branches {
                if let Outcome::Jump { node: target, .. } = &branch.outcome {
                    if !branch.needs_event {
                        jumps[index].push(Jump {
                            to: target.index(),
                            span: branch.span,
                        });
                    }
                }
            }
        }
        let components = strongly_connected(&jumps);
        let mut component_of = vec![0; nodes.len()];
        for (number, component) in components.iter().enumerate() {
            for &index in component {
                component_of[index] = number;
            }
        }
        for (number, mut component) in components.into_iter().enumerate() {
            let loops = component.len() > 1
                || jumps[component[0]]
                    .iter()
                    .any(|jump| jump.to == component[0]);
            if !loops {
                continue;
            }
            component.sort_by(|&a, &b| nodes[a].name.cmp(&nodes[b].name));
            let names: Vec<&str> = component
                .iter()
                .map(|&index| nodes[index].name.as_str())
                .collect();
            let listed = names_listed(&names);
            let first = component[0];
            let span = jumps[first]
                .iter()
                .find(|jump| component_of[jump.to] == number)
                .map_or(nodes[first].span, |jump| jump.span);
            self.push(
                Diagnostic::error(span, format!("these nodes can jump in a loop forever within one run: {listed}"))
                    .with_help("make one of the jumps depend on an event (`input.event == '...'`), or end the run with a value"),
            );
        }
    }

    pub(super) fn check_reachability(&mut self, nodes: &[Node], entries: &[NodeId]) {
        let mut seen = vec![false; nodes.len()];
        let mut pending: Vec<usize> = entries.iter().map(|entry| entry.index()).collect();
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut seen[index], true) {
                continue;
            }
            for branch in &nodes[index].branches {
                match &branch.outcome {
                    Outcome::Jump { node: target, .. } | Outcome::Ask { node: target, .. } => {
                        pending.push(target.index())
                    }
                    _ => {}
                }
            }
        }
        for (index, node) in nodes.iter().enumerate() {
            if !seen[index] {
                self.push(Diagnostic::warning(
                    node.span,
                    format!("node `{}` can never be reached", node.name),
                ));
            }
        }
    }
}

/// Whether a read is of `input.result` or `input.failure`, the answer to an ask.
fn reads_answer(read: &InputRead) -> bool {
    read.path
        .first()
        .is_some_and(|first| first == "result" || first == "failure")
}

/// A jump that needs no event, as an edge of the jump graph.
#[derive(Clone)]
struct Jump {
    /// The index of the node jumped to.
    to: usize,
    span: Span,
}

/// The strongly connected components of a graph, by Tarjan's algorithm with
/// an explicit stack, so a long chain of nodes can't overflow the thread's
/// stack.
fn strongly_connected(edges: &[Vec<Jump>]) -> Vec<Vec<usize>> {
    /// The index of a node not visited yet.
    const UNVISITED: usize = usize::MAX;
    struct Walk {
        next_index: usize,
        index: Vec<usize>,
        lowest: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
    }
    impl Walk {
        fn visit(&mut self, node: usize) {
            self.index[node] = self.next_index;
            self.lowest[node] = self.next_index;
            self.next_index += 1;
            self.stack.push(node);
            self.on_stack[node] = true;
        }
    }
    /// A node being visited, and how many of its edges are followed.
    struct Frame {
        node: usize,
        edges_followed: usize,
    }
    let count = edges.len();
    let mut walk = Walk {
        next_index: 0,
        index: vec![UNVISITED; count],
        lowest: vec![0; count],
        on_stack: vec![false; count],
        stack: Vec::new(),
    };
    let mut components = Vec::new();
    let mut frames = Vec::new();
    for root in 0..count {
        if walk.index[root] != UNVISITED {
            continue;
        }
        walk.visit(root);
        frames.push(Frame {
            node: root,
            edges_followed: 0,
        });
        while let Some(frame) = frames.last_mut() {
            let node = frame.node;
            if let Some(jump) = edges[node].get(frame.edges_followed) {
                frame.edges_followed += 1;
                if walk.index[jump.to] == UNVISITED {
                    walk.visit(jump.to);
                    frames.push(Frame {
                        node: jump.to,
                        edges_followed: 0,
                    });
                } else if walk.on_stack[jump.to] {
                    walk.lowest[node] = walk.lowest[node].min(walk.index[jump.to]);
                }
                continue;
            }
            // every edge of the node followed: finish it
            frames.pop();
            if let Some(parent) = frames.last() {
                walk.lowest[parent.node] = walk.lowest[parent.node].min(walk.lowest[node]);
            }
            if walk.lowest[node] == walk.index[node] {
                let mut component = Vec::new();
                while let Some(member) = walk.stack.pop() {
                    walk.on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}
