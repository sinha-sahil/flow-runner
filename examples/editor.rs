//! A visual builder's backend: open a flow as a `Document`, draw its graph,
//! and apply edits. Each edit rewrites only the text it touches, so the file
//! stays the source of truth, comments and layout included.
//!
//! ```text
//! cargo run --example editor --features json
//! ```

use flow_runner::{Declaration, Document, Edit, EditError, OutcomeView, Value};

const FLOW: &str = "\
edition 2026
flow refund
entry review
events approve, reject

# a person looks at every refund
node review = when {
  input.event == 'approve': paid()
  input.event == 'reject':  { \"status\": \"REJECTED\" }
  else: ignore
}

node paid = { \"status\": \"PAID\" }
";

fn main() {
    let mut document = Document::parse(FLOW);
    draw(&document);

    // small refunds skip the review: a new branch, placed first
    let edits = [
        Edit::AddBranch {
            node: "review".into(),
            guard: "input.data.amount < 100".into(),
            outcome: "paid()".into(),
            before: Some(0),
        },
        Edit::SetAnnotation {
            on: Declaration::Node("review".into()),
            name: "pos".into(),
            value: Value::from(vec![120, 40]),
        },
        Edit::RenameNode {
            from: "paid".into(),
            to: "refunded".into(),
        },
    ];
    for edit in edits {
        document.apply(edit).expect("a valid edit");
    }
    println!(
        "\nafter three edits, the comment and layout kept:\n{}",
        document.text()
    );
    draw(&document);

    // what a builder's frontend receives
    let json = document.graph().to_json();
    let count = |key: &str| json[key].as_array().map_or(0, Vec::len);
    println!(
        "as JSON: {} nodes, {} edges\n",
        count("nodes"),
        count("edges")
    );

    // an edit that doesn't parse is refused, and the document stays as it was
    let before = document.text().to_string();
    let guard = "input.data.amount <";
    let broken = Edit::SetGuard {
        node: "review".into(),
        index: 0,
        guard: guard.into(),
    };
    match document.apply(broken) {
        Err(EditError::Syntax(errors)) => println!("refused:\n{}\n", errors.report(guard)),
        other => panic!("{other:?}"),
    }
    assert_eq!(document.text(), before);

    // one that parses but breaks the flow is applied; the problem shows in the
    // diagnostics until another edit fixes it
    document
        .apply(Edit::RemoveNode {
            name: "refunded".into(),
        })
        .expect("the node exists");
    let diagnostics = document.diagnostics();
    assert!(diagnostics.has_errors());
    println!(
        "after removing `refunded`:\n{}",
        diagnostics.report(document.text())
    );
}

/// Print each node's branches, as a builder would draw them.
fn draw(document: &Document) {
    let graph = document.graph();
    for node in &graph.nodes {
        println!(
            "node {}{}",
            node.name,
            if node.is_entry { " (entry)" } else { "" }
        );
        for branch in &node.branches {
            // a node without `when` has one outcome and no guard
            let guard = match (&branch.guard, node.uses_when) {
                (Some(guard), _) => guard.as_str(),
                (None, true) => "else",
                (None, false) => "always",
            };
            let outcome = match &branch.outcome {
                OutcomeView::Jump { target, .. } => format!("go to {target}"),
                OutcomeView::Value { template, .. } => template.clone(),
                OutcomeView::Ask { name, target, .. } => format!("ask {name}, then {target}"),
                OutcomeView::Ignore => "ignore".to_string(),
                other => format!("{other:?}"),
            };
            println!("  {guard}: {outcome}");
        }
    }
}
