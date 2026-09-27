//! The editor API: graph views and checked edits that keep untouched text.

mod common;
use common::{compile, load, EXAMPLES, MAX_LITERAL_DEPTH};
use flow_runner::{Declaration, Document, EdgeKind, Edit, EditError, OutcomeView, Value};

/// Assert that `$document` refuses `$edit` with an error matching `$error`.
macro_rules! assert_refused {
    ($document:expr, $edit:expr, $error:pat) => {{
        let result = $document.apply($edit);
        assert!(matches!(result, Err($error)), "{result:?}");
    }};
}

/// An example flow, opened as a document.
fn open(name: &str) -> Document {
    Document::parse(load(name).1)
}

/// Lines of `after` that aren't in `before`, and the reverse.
fn line_diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    let added = after
        .iter()
        .filter(|l| !before.contains(l))
        .map(|s| s.to_string())
        .collect();
    let removed = before
        .iter()
        .filter(|l| !after.contains(l))
        .map(|s| s.to_string())
        .collect();
    (added, removed)
}

#[track_caller]
fn apply(document: &mut Document, edit: Edit) {
    let before = document.text().to_string();
    if let Err(error) = document.apply(edit.clone()) {
        panic!("{edit:?}: {error}\n{before}");
    }
}

fn errors(document: &Document) -> Vec<String> {
    document
        .diagnostics()
        .iter()
        .filter(|x| x.is_error())
        .map(|x| x.message.clone())
        .collect()
}

fn node_names(document: &Document) -> Vec<String> {
    document
        .graph()
        .nodes
        .into_iter()
        .map(|node| node.name)
        .collect()
}

#[track_caller]
fn assert_no_errors(document: &Document) {
    let errors = errors(document);
    assert!(errors.is_empty(), "{errors:?}\n{}", document.text());
}

#[track_caller]
fn assert_error_mentions(document: &Document, needle: &str) {
    let errors = errors(document);
    assert!(
        errors.iter().any(|error| error.contains(needle)),
        "{errors:?}"
    );
}

#[track_caller]
fn assert_text_contains(document: &Document, needle: &str) {
    let text = document.text();
    assert!(text.contains(needle), "{text}");
}

#[test]
fn opening_a_file_changes_nothing() {
    for name in EXAMPLES {
        let (_, src) = load(name);
        let document = Document::parse(src.clone());
        assert_eq!(document.text(), src);
        assert!(
            document.diagnostics().is_empty(),
            "{name}: {:?}",
            document.diagnostics()
        );
    }
}

#[test]
fn graph_shows_declarations_and_edges() {
    let document = open("return-lifecycle");
    let graph = document.graph();
    assert_eq!(graph.name.as_deref(), Some("return-lifecycle"));
    assert_eq!(graph.entries, ["requested"]);
    assert_eq!(graph.events.len(), 7);
    assert_eq!(graph.params[0].name, "stallAfterDays");
    assert_eq!(graph.params[0].default, Some(Value::Int(7)));
    assert_eq!(
        graph.params[0].annotations[0].value,
        Some(Value::from("Flag a stalled parcel after (days)"))
    );
    assert_eq!(
        graph.lets[0].params.as_deref(),
        Some(&["from".to_string(), "days".to_string()][..])
    );

    let inspecting = graph.nodes.iter().find(|n| n.name == "inspecting").unwrap();
    assert!(!inspecting.uses_when);
    let OutcomeView::Ask {
        name,
        payload,
        target,
        args,
    } = &inspecting.branches[0].outcome
    else {
        panic!("{:?}", inspecting.branches[0].outcome)
    };
    assert_eq!(
        (name.as_str(), target.as_str()),
        ("warehouse.inspect", "inspected")
    );
    assert!(payload.as_deref().unwrap().contains("input.data.returnId"));
    assert!(args.is_empty());

    let requested = graph.nodes.iter().find(|n| n.name == "requested").unwrap();
    assert!(requested.is_entry && requested.uses_when);
    assert_eq!(
        requested.branches[0].guard.as_deref(),
        Some("input.event == 'approve'")
    );
    assert_eq!(requested.branches.last().unwrap().guard, None);
    assert_eq!(
        requested.branches.last().unwrap().outcome,
        OutcomeView::Ignore
    );

    let ask_edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Ask(_)))
        .collect();
    assert_eq!(ask_edges.len(), 1);
    assert_eq!(
        (ask_edges[0].from.as_str(), ask_edges[0].to.as_str()),
        ("inspecting", "inspected")
    );
    // every jump in the file is an edge
    let jumps = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Jump)
        .count();
    assert_eq!(jumps, 12);
    // spans point at the text they describe
    assert!(document.text()[requested.span.range()].starts_with("node requested"));
}

#[test]
fn a_broken_file_still_opens() {
    let src = "edition 2026\nflow f\nentry a\n\nnode a = when {\n  input.x >: b()\n  else: ignore\n}\n\nnode b = { \"ok\": true }\n";
    let document = Document::parse(src);
    assert!(!errors(&document).is_empty());
    assert!(
        document.graph().nodes.iter().any(|n| n.name == "b"),
        "nodes after the broken one still show"
    );
}

#[test]
fn rename_updates_every_reference_and_nothing_else() {
    let mut document = open("return-lifecycle");
    let before = document.text().to_string();
    apply(
        &mut document,
        Edit::RenameNode {
            from: "closed".into(),
            to: "done".into(),
        },
    );
    assert_no_errors(&document);
    assert!(!document.text().contains("closed("), "{}", document.text());
    let (added, removed) = line_diff(&before, document.text());
    assert_eq!(added.len(), removed.len());
    assert!(removed.iter().all(|l| l.contains("closed")), "{removed:?}");
    // the entry is renamed too
    apply(
        &mut document,
        Edit::RenameNode {
            from: "requested".into(),
            to: "opened".into(),
        },
    );
    assert_text_contains(&document, "entry opened");
    assert_no_errors(&document);
    compile(document.text());
}

#[test]
fn rename_refuses_bad_names() {
    let mut document = open("return-lifecycle");
    let before = document.text().to_string();
    for bad in [
        "refunding",
        "stallAfterDays",
        "dueAt",
        "when",
        "input",
        "upper",
        "9lives",
        "a-b",
        "",
    ] {
        let result = document.apply(Edit::RenameNode {
            from: "closed".into(),
            to: bad.into(),
        });
        assert!(
            matches!(result, Err(EditError::BadName(_))),
            "{bad}: {result:?}"
        );
    }
    assert_refused!(
        document,
        Edit::RenameNode {
            from: "nope".into(),
            to: "x".into()
        },
        EditError::NotFound(_)
    );
    assert_eq!(document.text(), before);
}

#[test]
fn add_and_remove_nodes() {
    let mut document = open("spend-approval");
    apply(
        &mut document,
        Edit::AddNode {
            name: "escalated".into(),
            params: vec!["level".into()],
            body: "{ \"status\": \"ESCALATED\", \"level\": {{ level }} }".into(),
            after: Some("pending".into()),
        },
    );
    let text = document.text();
    let at = text.find("node escalated(level)").unwrap();
    assert!(
        at > text.find("node pending").unwrap() && at < text.find("node approved").unwrap(),
        "{text}"
    );
    // unreachable, so only a warning
    assert_no_errors(&document);
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|x| x.message.contains("escalated")),
        "{:?}",
        document.diagnostics()
    );

    apply(
        &mut document,
        Edit::AddNode {
            name: "last".into(),
            params: vec![],
            body: "ignore".into(),
            after: None,
        },
    );
    let text = document.text();
    assert!(text.ends_with("node last = ignore\n"), "{text}");

    apply(
        &mut document,
        Edit::RemoveNode {
            name: "escalated".into(),
        },
    );
    apply(
        &mut document,
        Edit::RemoveNode {
            name: "last".into(),
        },
    );
    assert_eq!(
        document.text(),
        load("spend-approval").1,
        "adding then removing restores the file"
    );

    // removing a node others jump to leaves errors to fix, not a refusal
    apply(
        &mut document,
        Edit::RemoveNode {
            name: "approved".into(),
        },
    );
    assert_error_mentions(&document, "unknown node `approved`");
}

#[test]
fn a_new_node_must_parse_on_its_own() {
    let mut document = open("carrier-quote");
    let before = document.text().to_string();
    for body in [
        "",
        "when {",
        "{ \"a\": ",
        "x() \nnode evil = ignore",
        "ignore }",
    ] {
        let result = document.apply(Edit::AddNode {
            name: "n".into(),
            params: vec![],
            body: body.into(),
            after: None,
        });
        assert!(
            matches!(result, Err(EditError::Syntax(_))),
            "{body:?}: {result:?}"
        );
    }
    assert_refused!(
        document,
        Edit::AddNode {
            name: "n".into(),
            params: vec!["a".into(), "a".into()],
            body: "ignore".into(),
            after: None,
        },
        EditError::BadName(_)
    );
    assert_eq!(document.text(), before);
}

#[test]
fn add_branch_goes_before_else_with_matching_indent() {
    let mut document = open("return-lifecycle");
    apply(
        &mut document,
        Edit::AddBranch {
            node: "rejected".into(),
            guard: "input.event == 'reopen'".into(),
            outcome: "requested()".into(),
            before: None,
        },
    );
    let text = document.text();
    let start = text.find("node rejected").unwrap();
    let block = &text[start..text.find("@label = 'Closed'").unwrap()];
    let lines: Vec<&str> = block.lines().collect();
    assert_eq!(
        lines[3], "  input.event == 'reopen': requested()",
        "{block}"
    );
    assert_eq!(lines[4].trim(), "else: ignore");
    assert_no_errors(&document);
    let graph = document.graph();
    let rejected = graph.nodes.iter().find(|n| n.name == "rejected").unwrap();
    assert_eq!(rejected.branches.len(), 4);
}

#[test]
fn add_branch_to_a_single_outcome_node_makes_a_when() {
    let mut document = open("spend-approval");
    apply(
        &mut document,
        Edit::AddBranch {
            node: "withdrawn".into(),
            guard: "input.data.amount > 0".into(),
            outcome: "{ \"status\": \"WITHDRAWN\", \"refund\": true }".into(),
            before: None,
        },
    );
    assert_no_errors(&document);
    let graph = document.graph();
    let withdrawn = graph.nodes.iter().find(|n| n.name == "withdrawn").unwrap();
    assert!(withdrawn.uses_when);
    assert_eq!(
        withdrawn.branches[0].guard.as_deref(),
        Some("input.data.amount > 0")
    );
    assert_eq!(withdrawn.branches[1].guard, None);
    assert!(
        matches!(&withdrawn.branches[1].outcome, OutcomeView::Value { template, .. } if template == "{ \"status\": \"WITHDRAWN\" }"),
        "{:?}",
        withdrawn.branches[1].outcome
    );
}

#[test]
fn branch_edits_are_checked() {
    let mut document = open("return-lifecycle");
    let before = document.text().to_string();
    let add = |guard: &str, outcome: &str, index: Option<usize>| Edit::AddBranch {
        node: "closed".into(),
        guard: guard.into(),
        outcome: outcome.into(),
        before: index,
    };
    assert_refused!(document, add("x >", "ignore", None), EditError::Syntax(_));
    assert_refused!(
        document,
        add("true", "ignore }", None),
        EditError::Syntax(_)
    );
    // text that would smuggle in more structure is refused
    assert_refused!(
        document,
        add(
            "true: ignore\n}\nnode evil = ignore\nnode x = when {\n true",
            "ignore",
            None
        ),
        EditError::Syntax(_)
    );
    assert_refused!(
        document,
        add("true", "ignore", Some(3)),
        EditError::Invalid(_)
    );
    assert_refused!(
        document,
        add("true", "ignore", Some(9)),
        EditError::NotFound(_)
    );
    assert_refused!(
        document,
        Edit::RemoveBranch {
            node: "inspecting".into(),
            index: 0
        },
        EditError::Invalid(_)
    );
    assert_refused!(
        document,
        Edit::MoveBranch {
            node: "closed".into(),
            from: 2,
            to: 0
        },
        EditError::Invalid(_)
    );
    assert_refused!(
        document,
        Edit::MoveBranch {
            node: "closed".into(),
            from: 0,
            to: 2
        },
        EditError::Invalid(_)
    );
    assert_refused!(
        document,
        Edit::SetGuard {
            node: "closed".into(),
            index: 0,
            guard: "input.event ==".into()
        },
        EditError::Syntax(_)
    );
    assert_eq!(document.text(), before);
}

#[test]
fn move_remove_and_rewrite_branches() {
    let mut document = open("return-lifecycle");
    apply(
        &mut document,
        Edit::MoveBranch {
            node: "requested".into(),
            from: 0,
            to: 2,
        },
    );
    let guards: Vec<Option<String>> = document.graph().nodes[0]
        .branches
        .iter()
        .map(|b| b.guard.clone())
        .collect();
    assert_eq!(
        guards,
        [
            Some("input.event == 'reject'".into()),
            Some("input.event == 'cancel'".into()),
            Some("input.event == 'approve'".into()),
            Some("input.arrived".into()),
            None
        ]
    );
    apply(
        &mut document,
        Edit::MoveBranch {
            node: "requested".into(),
            from: 2,
            to: 0,
        },
    );
    assert_eq!(
        document.text(),
        load("return-lifecycle").1,
        "moving back restores the file"
    );

    apply(
        &mut document,
        Edit::RemoveBranch {
            node: "requested".into(),
            index: 2,
        },
    );
    let text = document.text();
    assert!(!text.contains("Cancelled by the customer"), "{text}");
    assert_no_errors(&document);

    apply(
        &mut document,
        Edit::SetGuard {
            node: "requested".into(),
            index: 1,
            guard: "input.event == 'reject' && input.payload != null".into(),
        },
    );
    apply(
        &mut document,
        Edit::SetOutcome {
            node: "requested".into(),
            index: 1,
            outcome: "closed('Rejected early')".into(),
        },
    );
    apply(
        &mut document,
        Edit::SetOutcome {
            node: "inspecting".into(),
            index: 0,
            outcome: "ask warehouse.check({ \"id\": 1 }) -> inspected()".into(),
        },
    );
    assert_no_errors(&document);
    let flow = compile(document.text());
    let asks: Vec<&str> = flow.asks().collect();
    assert_eq!(asks, ["warehouse.check"]);
}

#[test]
fn annotations_are_set_replaced_and_removed() {
    let mut document = open("carrier-quote");
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Node("quoted".into()),
            name: "pos".into(),
            value: Value::Arr(vec![Value::Int(40), Value::Int(-30)]),
        },
    );
    assert_text_contains(&document, "@pos = [40, -30]\nnode quoted(");
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Node("quoted".into()),
            name: "pos".into(),
            value: Value::Arr(vec![Value::Int(1), Value::Int(2)]),
        },
    );
    assert_text_contains(&document, "@pos = [1, 2]\nnode quoted(");
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Flow,
            name: "label".into(),
            value: Value::from("It's \"quoted\"\n"),
        },
    );
    assert_eq!(
        document.graph().annotations[0].value,
        Some(Value::from("It's \"quoted\"\n"))
    );
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Param("primary".into()),
            name: "options".into(),
            value: Value::obj([("a", 1), ("b c", 2)]),
        },
    );
    assert_text_contains(
        &document,
        "@options = { \"a\": 1, \"b c\": 2 }\nparam primary",
    );
    assert_eq!(
        compile(document.text()).hash(),
        load("carrier-quote").0.hash(),
        "annotations don't change the hash"
    );

    apply(
        &mut document,
        Edit::RemoveAnnotation {
            on: Declaration::Node("quoted".into()),
            name: "pos".into(),
        },
    );
    apply(
        &mut document,
        Edit::RemoveAnnotation {
            on: Declaration::Param("primary".into()),
            name: "options".into(),
        },
    );
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Flow,
            name: "label".into(),
            value: Value::from("Carrier quote"),
        },
    );
    assert_eq!(document.text(), load("carrier-quote").1);
    assert_refused!(
        document,
        Edit::RemoveAnnotation {
            on: Declaration::Flow,
            name: "nope".into()
        },
        EditError::NotFound(_)
    );
    assert_refused!(
        document,
        Edit::SetAnnotation {
            on: Declaration::Flow,
            name: "a b".into(),
            value: Value::Null
        },
        EditError::BadName(_)
    );
}

#[test]
fn params_events_and_entries() {
    let mut document = open("return-lifecycle");
    apply(
        &mut document,
        Edit::AddParam {
            name: "graceDays".into(),
            default: Value::Int(2),
        },
    );
    assert_text_contains(&document, "param stallAfterDays = 7\nparam graceDays = 2\n");
    assert_refused!(
        document,
        Edit::AddParam {
            name: "graceDays".into(),
            default: Value::Int(2)
        },
        EditError::BadName(_)
    );
    apply(
        &mut document,
        Edit::RemoveParam {
            name: "graceDays".into(),
        },
    );

    apply(
        &mut document,
        Edit::AddEvent {
            name: "parcel.lost".into(),
        },
    );
    assert_text_contains(&document, "acknowledge, reopen, parcel.lost\n");
    for bad in ["tick", "approve", "9x", "a..b", ".a", "a-"] {
        let result = document.apply(Edit::AddEvent { name: bad.into() });
        assert!(
            matches!(result, Err(EditError::BadName(_))),
            "{bad}: {result:?}"
        );
    }
    apply(
        &mut document,
        Edit::RemoveEvent {
            name: "parcel.lost".into(),
        },
    );
    apply(
        &mut document,
        Edit::RemoveEvent {
            name: "approve".into(),
        },
    );
    assert_text_contains(&document, "events reject, cancel");
    // the flow still compares with 'approve', so that's now an error
    assert_error_mentions(&document, "`approve` is not a declared event");

    apply(
        &mut document,
        Edit::AddEntry {
            node: "closed".into(),
        },
    );
    assert_text_contains(&document, "entry requested, closed\n");
    // an entry with parameters is an error the diagnostics show
    assert_error_mentions(&document, "takes parameters");
    apply(
        &mut document,
        Edit::RemoveEntry {
            node: "closed".into(),
        },
    );
    assert_refused!(
        document,
        Edit::AddEntry {
            node: "nope".into()
        },
        EditError::NotFound(_)
    );
    assert_refused!(
        document,
        Edit::AddEntry {
            node: "requested".into()
        },
        EditError::Invalid(_)
    );
}

#[test]
fn declarations_are_created_when_missing() {
    let mut document = Document::parse("edition 2026\nflow f\n\nnode a = { \"ok\": true }\n");
    apply(&mut document, Edit::AddEntry { node: "a".into() });
    apply(&mut document, Edit::AddEvent { name: "go".into() });
    apply(
        &mut document,
        Edit::AddParam {
            name: "limit".into(),
            default: Value::from("0.5"),
        },
    );
    assert_eq!(document.text(), "edition 2026\nflow f\nentry a\nevents go\n\nparam limit = '0.5'\n\nnode a = { \"ok\": true }\n");
    assert_no_errors(&document);
    apply(&mut document, Edit::RemoveEvent { name: "go".into() });
    apply(
        &mut document,
        Edit::RemoveParam {
            name: "limit".into(),
        },
    );
    assert_eq!(
        document.text(),
        "edition 2026\nflow f\nentry a\n\nnode a = { \"ok\": true }\n"
    );
}

#[test]
fn every_edit_keeps_every_other_line() {
    let mut document = open("return-eligibility");
    let before = document.text().to_string();
    apply(
        &mut document,
        Edit::RenameNode {
            from: "refuse".into(),
            to: "refuseAll".into(),
        },
    );
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Node("evaluate".into()),
            name: "pos".into(),
            value: Value::Arr(vec![Value::Int(0), Value::Int(0)]),
        },
    );
    apply(
        &mut document,
        Edit::AddBranch {
            node: "evaluate".into(),
            guard: "order.lines.len() == 0".into(),
            outcome: "refuseAll('Empty')".into(),
            before: Some(0),
        },
    );
    let (_, removed) = line_diff(&before, document.text());
    assert!(removed.iter().all(|l| l.contains("refuse")), "{removed:?}");
    for comment in before.lines().filter(|l| l.contains('#')) {
        assert!(document.text().contains(comment), "lost: {comment}");
    }
    assert_no_errors(&document);
}

#[test]
fn annotations_land_on_the_right_declaration_when_several_share_a_line() {
    let mut document = Document::parse("edition 2026\nflow f\nentry a\nparam a1 = 1 param b1 = 2\nnode a = b() node b = { \"ok\": true }\n");
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Param("b1".into()),
            name: "label".into(),
            value: Value::from("B"),
        },
    );
    apply(
        &mut document,
        Edit::SetAnnotation {
            on: Declaration::Node("b".into()),
            name: "pos".into(),
            value: Value::Int(1),
        },
    );
    let graph = document.graph();
    let b1 = graph.params.iter().find(|p| p.name == "b1").unwrap();
    assert_eq!(b1.annotations[0].value, Some(Value::from("B")));
    assert!(graph
        .params
        .iter()
        .find(|p| p.name == "a1")
        .unwrap()
        .annotations
        .is_empty());
    let b = graph.nodes.iter().find(|n| n.name == "b").unwrap();
    assert_eq!(b.annotations[0].name, "pos");
    assert!(graph
        .nodes
        .iter()
        .find(|n| n.name == "a")
        .unwrap()
        .annotations
        .is_empty());
    assert_no_errors(&document);
}

#[test]
fn rename_waits_for_a_file_that_parses() {
    let src = "edition 2026\nflow f\nentry a\n\nnode a = when {\n  input.x >: b()\n  else: b()\n}\n\nnode b = { \"ok\": true }\n";
    let mut document = Document::parse(src);
    assert_refused!(
        document,
        Edit::RenameNode {
            from: "b".into(),
            to: "c".into(),
        },
        EditError::Invalid(_)
    );
    assert_eq!(document.text(), src);
}

#[test]
fn edits_at_the_end_of_a_file_without_a_final_line_break() {
    let mut document =
        Document::parse("edition 2026\nflow f\nentry a\nnode a = ignore\nparam a1 = 1");
    apply(
        &mut document,
        Edit::AddParam {
            name: "b1".into(),
            default: Value::Bool(true),
        },
    );
    let text = document.text();
    assert!(
        text.ends_with("param a1 = 1\nparam b1 = true\n"),
        "{text:?}"
    );

    let mut document = Document::parse("edition 2026\nnode a = ignore\nentry a\nflow f");
    apply(&mut document, Edit::AddEvent { name: "go".into() });
    let text = document.text();
    assert!(text.ends_with("flow f\nevents go\n"), "{text:?}");
    assert_no_errors(&document);
}

#[test]
fn values_are_written_as_literals_that_read_back_the_same() {
    let values = [
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::Decimal("-0.50".parse().unwrap()),
        // a whole decimal stays a decimal
        Value::Decimal("2".parse().unwrap()),
        Value::Str("it's \"quoted\"\n\\ {{ not a hole }}".into()),
        Value::Arr(Vec::new()),
        Value::obj([("a key", Value::Arr(vec![Value::Null, Value::Bool(false)]))]),
    ];
    for value in values {
        let mut document = Document::parse("edition 2026\nflow f\nentry a\nnode a = ignore\n");
        apply(
            &mut document,
            Edit::AddParam {
                name: "p".into(),
                default: value.clone(),
            },
        );
        assert_eq!(
            document.graph().params[0].default,
            Some(value),
            "{}",
            document.text()
        );
    }
}

const INLINE: &str = "edition 2026\nflow f\nentry a\n\nnode a = when {\n  input.data.x == 1: b(), input.data.x == 2: c()\n  else: ignore\n}\n\nnode b = { \"b\": true }\nnode c = { \"c\": true }\n";

#[test]
fn a_comment_in_an_edit_is_refused() {
    let mut document = Document::parse(INLINE);
    let edits = [
        Edit::SetOutcome {
            node: "a".into(),
            index: 0,
            outcome: "b() # route to b".into(),
        },
        Edit::SetGuard {
            node: "a".into(),
            index: 0,
            guard: "input.data.x == 1 # one".into(),
        },
        Edit::AddBranch {
            node: "a".into(),
            guard: "true".into(),
            outcome: "{ \"k\": {{ 1 # one\n }} }".into(),
            before: Some(0),
        },
        // and when it hides a closing bracket, it is the error given first
        Edit::SetOutcome {
            node: "a".into(),
            index: 0,
            outcome: "b(1 # one)".into(),
        },
        Edit::SetGuard {
            node: "a".into(),
            index: 0,
            guard: "(input.data.x # c)".into(),
        },
        Edit::SetOutcome {
            node: "a".into(),
            index: 0,
            outcome: "b(\"{{\", (1 # ))".into(),
        },
    ];
    for edit in edits {
        let given = match &edit {
            Edit::SetOutcome { outcome, .. } | Edit::AddBranch { outcome, .. } => outcome.as_str(),
            Edit::SetGuard { guard, .. } => guard.as_str(),
            _ => unreachable!(),
        };
        match document.apply(edit.clone()) {
            Err(EditError::Syntax(errors)) => {
                let span = errors[0].span;
                assert_eq!(&given[span.start as usize..span.start as usize + 1], "#");
                assert!(
                    errors[0].message.contains("can't hold a `#` comment"),
                    "{errors:?}"
                );
            }
            other => panic!("{edit:?}: {other:?}"),
        }
    }
    assert_eq!(document.text(), INLINE);
    // a `#` in a string is text, even when the edit fails for another reason
    let unclosed = Edit::SetOutcome {
        node: "a".into(),
        index: 0,
        outcome: "b(\"{{\", \"#\"".into(),
    };
    match document.apply(unclosed) {
        Err(EditError::Syntax(errors)) => {
            assert!(
                errors.iter().all(|e| !e.message.contains("comment")),
                "{errors:?}"
            )
        }
        other => panic!("{other:?}"),
    }
    apply(
        &mut document,
        Edit::SetOutcome {
            node: "a".into(),
            index: 0,
            outcome: "{ \"tag\": \"#1\" }".into(),
        },
    );
    assert_eq!(document.graph().nodes[0].branches.len(), 3);
}

#[test]
fn syntax_errors_point_into_the_text_given() {
    let mut document = Document::parse(INLINE);
    let cases = [
        (
            Edit::SetGuard {
                node: "a".into(),
                index: 0,
                guard: "   x ==".into(),
            },
            "   x ==",
        ),
        (
            Edit::AddNode {
                name: "n".into(),
                params: vec![],
                body: "  { \"a\": ".into(),
                after: None,
            },
            "  { \"a\": ",
        ),
    ];
    for (edit, given) in cases {
        let Err(EditError::Syntax(errors)) = document.apply(edit.clone()) else {
            panic!("{edit:?}")
        };
        for error in &errors {
            let span = error.span;
            assert!(
                span.start >= 2 && span.end as usize <= given.len(),
                "{edit:?}: {error:?}"
            );
        }
    }
}

#[test]
fn else_stays_last() {
    let mut document = open("return-lifecycle");
    let before = document.text().to_string();
    let refused = [
        Edit::SetGuard {
            node: "requested".into(),
            index: 0,
            guard: "else".into(),
        },
        Edit::AddBranch {
            node: "requested".into(),
            guard: "else".into(),
            outcome: "ignore".into(),
            before: None,
        },
        Edit::AddBranch {
            node: "inspecting".into(),
            guard: "else".into(),
            outcome: "ignore".into(),
            before: None,
        },
    ];
    for edit in refused {
        let result = document.apply(edit.clone());
        assert!(
            matches!(result, Err(EditError::Invalid(_))),
            "{edit:?}: {result:?}"
        );
    }
    assert_eq!(document.text(), before);
}

#[test]
fn new_nodes_land_beside_crowded_and_broken_lines() {
    // the anchor shares its line with another node: the new one goes after
    // the line
    let mut document =
        Document::parse("edition 2026\nflow f\nentry a\nnode a = b() node b = { \"ok\": true }\n");
    let add = |after: Option<&str>| Edit::AddNode {
        name: "n".into(),
        params: vec![],
        body: "ignore".into(),
        after: after.map(Into::into),
    };
    apply(&mut document, add(Some("a")));
    assert_eq!(node_names(&document), ["a", "b", "n"]);
    // after a block that is never closed, the new node is still its own
    let mut document = Document::parse(
        "edition 2026\nflow f\nentry a\nnode a = ignore\nnode b = when {\n  input.x: ignore",
    );
    apply(&mut document, add(None));
    assert!(document.graph().nodes.iter().any(|node| node.name == "n"));
}

#[test]
fn oversized_documents_are_not_read() {
    let big = format!(
        "edition 2026\nflow f\nentry a\nnode a = ignore\n#{}\n",
        "x".repeat(flow_runner::MAX_SOURCE_BYTES)
    );
    let document = Document::parse(big);
    assert!(
        document.diagnostics()[0].message.contains("too large"),
        "{:?}",
        document.diagnostics()
    );
    assert!(document.graph().nodes.is_empty());
    // nor may an edit make one
    let mut document = Document::parse("edition 2026\nflow f\nentry a\nnode a = ignore\n");
    let refused = document.apply(Edit::AddParam {
        name: "p".into(),
        default: Value::Str("x".repeat(flow_runner::MAX_SOURCE_BYTES).as_str().into()),
    });
    assert!(
        matches!(refused, Err(EditError::Invalid(ref why)) if why.contains("too large")),
        "{refused:?}"
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "times renames, so run it with --release")]
fn renaming_grows_linearly_with_the_references() {
    let file = |n: usize| {
        let branches: String = (0..n)
            .map(|i| format!("  input.data.x == {i}: t()\n"))
            .collect();
        format!("edition 2026\nflow f\nentry a\nnode a = when {{\n{branches}  else: ignore\n}}\nnode t = ignore\n")
    };
    let time = |n: usize| {
        let mut document = Document::parse(file(n));
        let started = std::time::Instant::now();
        apply(
            &mut document,
            Edit::RenameNode {
                from: "t".into(),
                to: "target".into(),
            },
        );
        started.elapsed()
    };
    let (small, large) = (time(1_000), time(16_000));
    // 16 times the references: linear work takes about 16 times as long,
    // quadratic work about 256 times
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-4);
    assert!(ratio < 64.0, "{small:?} for 1000, {large:?} for 16000");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "times edits, so run it with --release")]
fn adding_a_node_grows_linearly_with_chained_declarations() {
    // each node ends on the line the next one starts on, so finding a free
    // line after the first walks the whole chain
    let file = |n: usize| {
        let nodes: String = (0..n)
            .map(|i| {
                format!(
                    "node n{i} = when {{\n  input.data.go: n{}()\n  else: ignore\n}} ",
                    (i + 1) % n
                )
            })
            .collect();
        format!("edition 2026\nflow f\nentry n0\n{nodes}\n")
    };
    let time = |n: usize| {
        let mut document = Document::parse(file(n));
        let started = std::time::Instant::now();
        apply(
            &mut document,
            Edit::AddNode {
                name: "extra".into(),
                params: vec![],
                body: "ignore".into(),
                after: Some("n0".into()),
            },
        );
        started.elapsed()
    };
    let (small, large) = (time(1_000), time(16_000));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-4);
    assert!(ratio < 64.0, "{small:?} for 1000, {large:?} for 16000");
}

#[test]
fn removing_a_listed_name_keeps_the_others_comments() {
    let list = "events approve,   # a manager approved\n  reject,   # a manager rejected\n  cancel    # the customer cancelled\n";
    let file = |list: &str| {
        format!("edition 2026\nflow f\nentry a\n{list}\nnode a = when {{\n  input.event == 'approve': ignore\n  else: ignore\n}}\n")
    };
    let cases = [
        (
            "reject",
            "events approve,   # a manager approved\n  cancel    # the customer cancelled\n",
        ),
        (
            "cancel",
            "events approve,   # a manager approved\n  reject   # a manager rejected\n",
        ),
        (
            "approve",
            "events reject,   # a manager rejected\n  cancel    # the customer cancelled\n",
        ),
    ];
    for (name, left) in cases {
        let mut document = Document::parse(file(list));
        apply(&mut document, Edit::RemoveEvent { name: name.into() });
        assert_eq!(document.text(), file(left), "removing {name}");
    }
    let mut document =
        Document::parse("edition 2026\nflow f\nentry a, b\nnode a = ignore\nnode b = ignore\n");
    apply(&mut document, Edit::RemoveEntry { node: "b".into() });
    assert_text_contains(&document, "entry a\n");
}

#[test]
fn a_branch_goes_inside_a_block_that_closes_on_its_line() {
    let mut document = Document::parse("edition 2026\nflow f\nentry a\nnode a = when {\n  input.data.x == 1: b() }\nnode b = { \"ok\": true }\n");
    apply(
        &mut document,
        Edit::AddBranch {
            node: "a".into(),
            guard: "else".into(),
            outcome: "ignore".into(),
            before: None,
        },
    );
    assert_no_errors(&document);
    assert_eq!(document.graph().nodes[0].branches.len(), 2);
}

#[test]
fn moved_branches_take_their_comments_along() {
    let mut document = open("return-lifecycle");
    apply(
        &mut document,
        Edit::MoveBranch {
            node: "closed".into(),
            from: 0,
            to: 1,
        },
    );
    let line_with = |needle: &str| {
        document
            .text()
            .lines()
            .find(|line| line.contains(needle))
            .unwrap()
            .to_string()
    };
    let reopen = line_with("requested()");
    assert!(reopen.contains("# a cycle"), "{reopen}");
    let closed = line_with("\"CLOSED\"");
    assert!(!closed.contains('#'), "{closed}");
}

#[test]
fn new_lines_go_after_every_declaration_on_the_anchor_line() {
    let src = "edition 2026\nflow f\nentry a\nparam p = 1 node a = when {\n  input.data.x == 1: b()\n  else: ignore\n}\nnode b = { \"ok\": true } node c = when {\n  input.arrived: b()\n  else: ignore\n}\n";
    let mut document = Document::parse(src);
    apply(
        &mut document,
        Edit::AddParam {
            name: "q".into(),
            default: Value::Int(2),
        },
    );
    apply(
        &mut document,
        Edit::AddNode {
            name: "n".into(),
            params: vec![],
            body: "ignore".into(),
            after: Some("b".into()),
        },
    );
    assert_eq!(node_names(&document), ["a", "b", "c", "n"]);
    assert_eq!(document.graph().params.len(), 2);
}

#[test]
fn a_file_with_crlf_line_breaks_keeps_them() {
    let src = "edition 2026\r\nflow f\r\nentry a\r\n\r\nparam p = 1\r\n\r\nnode a = ignore\r\n\r\nnode b = ignore\r\n";
    let edit = |document: &mut Document| {
        apply(
            document,
            Edit::AddParam {
                name: "q".into(),
                default: Value::Int(2),
            },
        );
        for (name, after) in [("c", None), ("d", Some("a".to_string()))] {
            apply(
                document,
                Edit::AddNode {
                    name: name.into(),
                    params: vec![],
                    body: "ignore".into(),
                    after,
                },
            );
        }
        apply(document, Edit::RemoveNode { name: "b".into() });
    };
    let mut document = Document::parse(src);
    edit(&mut document);
    let text = document.text();
    assert_eq!(
        text.matches('\n').count(),
        text.matches("\r\n").count(),
        "{text:?}"
    );
    assert!(text.contains("param p = 1\r\nparam q = 2\r\n"), "{text:?}");
    // the same edits on the file with plain line breaks give the same text
    let mut plain = Document::parse(src.replace("\r\n", "\n"));
    edit(&mut plain);
    assert_eq!(text, plain.text().replace('\n', "\r\n"));
}

#[test]
fn values_too_deep_for_a_literal_are_refused_up_front() {
    let deep = |depth: usize| {
        let mut value = Value::Int(1);
        for _ in 0..depth {
            value = Value::Arr(vec![value]);
        }
        value
    };
    let add = |depth| Edit::AddParam {
        name: "p".into(),
        default: deep(depth),
    };
    let empty = || Document::parse("edition 2026\nflow f\nentry a\nnode a = ignore\n");
    apply(&mut empty(), add(MAX_LITERAL_DEPTH));
    let refused = empty().apply(add(MAX_LITERAL_DEPTH + 1));
    let too_deep = format!("nests deeper than {MAX_LITERAL_DEPTH}");
    assert!(
        matches!(refused, Err(EditError::Invalid(ref why)) if why.contains(&too_deep)),
        "{refused:?}"
    );
}
