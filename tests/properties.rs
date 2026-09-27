//! Property tests: nothing panics on hostile input, runs are deterministic,
//! and a flow is safe to share across threads.
#![cfg(feature = "json")]

mod common;
use ciborium::value::Value as Cbor;
use common::*;
use flow_runner::{Decision, Declaration, Document, Edit, Flow, Input, Limits, Token, Value};
use proptest::prelude::*;
use serde_json::json;

/// `PROPTEST_CASES` raises the case count for a deeper run.
fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn sources() -> Vec<String> {
    EXAMPLES.iter().map(|name| load(name).1).collect()
}

/// Arbitrary JSON, a few levels deep.
fn json_value() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(json!(null)),
        any::<bool>().prop_map(|b| json!(b)),
        any::<i64>().prop_map(|i| json!(i)),
        (-1e9f64..1e9).prop_map(|f| json!(f)),
        "[a-z0-9 ]{0,12}".prop_map(|s| json!(s)),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(serde_json::Value::Array),
            prop::collection::btree_map("[a-zA-Z]{1,12}", inner, 0..8)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Like [`json_value`], with strings long enough to cross buffer sizes.
fn json_with_long_text() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(|i| json!(i)),
        (-1e9f64..1e9).prop_map(|f| json!(f)),
        (0usize..9000).prop_map(|n| json!("é".repeat(n / 2) + &"x".repeat(n % 7))),
        "[a-zA-Z0-9 ]{0,20}".prop_map(|s| json!(s)),
    ];
    leaf.prop_recursive(3, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
            prop::collection::btree_map("[a-z]{1,8}", inner, 0..6)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

/// A plausible order for the eligibility flow, with random shapes mixed in.
fn eligibility_data() -> impl Strategy<Value = serde_json::Value> {
    (
        prop::collection::vec((0i64..20, "[A-C][0-9]"), 0..30),
        prop::collection::vec((0i64..5, "[A-C][0-9]"), 0..30),
        prop::collection::vec(prop::sample::select(vec!["SALE20", "FINALSALE", "X"]), 0..3),
        0i64..100,
    )
        .prop_map(|(lines, claimed, codes, days)| {
            json!({
                "evaluatedAt": NOW,
                "claimedLines": claimed.into_iter().map(|(q, id)| json!({ "lineId": id, "quantity": q })).collect::<Vec<_>>(),
                "order": {
                    "deliveredAt": NOW - days * DAY,
                    "discountCodes": codes,
                    "lines": lines.into_iter().map(|(q, id)| json!({ "lineId": id, "fulfilledQuantity": q })).collect::<Vec<_>>(),
                }
            })
        })
}

/// Check `src`: nothing panics, every diagnostic's span slices it, and the report renders.
fn check_cleanly(src: &str) -> Result<(), TestCaseError> {
    let diagnostics = Flow::check(src);
    for diagnostic in &diagnostics {
        prop_assert!(src.get(diagnostic.span.range()).is_some(), "{diagnostic:?}");
    }
    let _ = diagnostics.report(src);
    Ok(())
}

/// One random change: insert text, delete a stretch, repeat a line, or insert flow syntax.
fn mutate(src: &mut String, at: prop::sample::Index, kind: u8, text: &str) {
    const SYNTAX: [&str; 18] = [
        "{",
        "}",
        "(",
        ")",
        ":",
        "->",
        "when",
        "else",
        "node",
        "'",
        "\"",
        "{{",
        "}}",
        "@",
        "#",
        "€",
        "\nnode n = ",
        "\n@a = ",
    ];
    if src.is_empty() {
        return;
    }
    let floor = |s: &str, mut i: usize| {
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let i = floor(src, at.index(src.len()));
    match kind {
        0 => src.insert_str(i, text),
        1 => {
            let j = floor(src, (i + text.len() + 1).min(src.len()));
            src.replace_range(i..j, "");
        }
        2 => {
            let line = src[i..].split('\n').next().unwrap_or("").to_string();
            src.insert_str(i, &line);
        }
        _ => src.insert_str(i, SYNTAX[text.len() % SYNTAX.len()]),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(400), .. ProptestConfig::default() })]

    #[test]
    fn checking_random_text_is_clean(src in "\\PC{0,400}") {
        check_cleanly(&src)?;
    }

    #[test]
    fn checking_mutated_flows_is_clean(
        which in 0..EXAMPLES.len(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), 0u8..4, "\\PC{0,6}"), 1..6),
    ) {
        let mut src = sources()[which].clone();
        for (at, kind, text) in edits {
            mutate(&mut src, at, kind, &text);
        }
        check_cleanly(&src)?;
    }

    #[test]
    fn eligibility_never_panics_and_is_deterministic(data in eligibility_data(), window in 0i64..400) {
        let (flow, _) = load("return-eligibility");
        let input = || Input::start("evaluate", from_json(data.clone())).param("windowDays", window);
        let first = flow.run(input(), &Limits::default()).unwrap();
        let second = flow.run(input(), &Limits::default()).unwrap();
        prop_assert_eq!(&first, &second);
        prop_assert!(matches!(first, Decision::Value { .. }), "{:?}", first);
    }

    #[test]
    fn any_flow_survives_any_data(which in 0..EXAMPLES.len(), data in json_value(), pick in 0usize..9) {
        let (flow, _) = load(EXAMPLES[which]);
        let entry = flow.entries().next().unwrap().to_string();
        let started = flow.run(Input::start(entry, from_json(data.clone())), &Limits::default());
        // and any declared event on a parked instance
        if let Ok(Decision::Value { token, .. }) = started {
            let events: Vec<&str> = flow.events().collect();
            if !events.is_empty() {
                let event = events[pick % events.len()].to_string();
                let _ = flow.run(Input::resume(token, from_json(data.clone())).event(event, from_json(data)), &Limits::default());
            }
        }
    }

    #[test]
    fn any_value_survives_a_token(data in json_with_long_text()) {
        let flow = compile(
            "edition 2026\nflow keep\nentry start\n\nnode start = kept(input.data)\nnode kept(v) = { \"v\": {{ v }} }\n",
        );
        let Decision::Value { token, value: first, .. } = flow.run(Input::start("start", from_json(data.clone())), &Limits::default()).unwrap() else {
            panic!("expected a value")
        };
        let again = flow.run(Input::resume(token.clone(), Value::Null), &Limits::default()).unwrap();
        let Decision::Value { value: second, .. } = again else { panic!("{again:?}") };
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(to_json(&second)["v"].clone(), to_json(&from_json(data)));
        // and through base64, as a host would store it
        let restored = Token::from_base64(&token.to_base64()).unwrap();
        prop_assert_eq!(restored, token);
    }

    #[test]
    fn random_tokens_are_rejected_cleanly(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let (flow, _) = load("return-lifecycle");
        let result = flow.run(Input::resume(Token::from_bytes(bytes.clone()), Value::Null), &Limits::default());
        prop_assert!(result.is_err());
        let _ = Token::from_bytes(bytes).inspect();
    }

    // Random bytes fail the checksum, so these tests seal what they make to reach the decoder.
    #[test]
    fn tokens_with_any_field_are_handled(seed in any::<prop::sample::Index>(), field in 0..=forge::ASK + 1, value in cbor()) {
        let seeds = token_seeds();
        let (flow, token) = &seeds[seed.index(seeds.len())];
        let mut fields = forge::fields(token);
        if field < fields.len() {
            fields[field] = value;
        } else {
            fields.push(value);
        }
        exercise(flow, &forge::seal(fields));
    }

    #[test]
    fn tokens_with_any_bytes_are_handled(
        seed in any::<prop::sample::Index>(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0u8..4), 1..6),
    ) {
        let seeds = token_seeds();
        let (flow, token) = &seeds[seed.index(seeds.len())];
        let mut body = forge::body(token);
        for (at, byte, kind) in edits {
            if body.is_empty() {
                break;
            }
            let i = at.index(body.len());
            match kind {
                0 => body[i] = byte,
                1 => body.insert(i, byte),
                2 => {
                    body.remove(i);
                }
                _ => body[i] ^= 1 << (byte % 8),
            }
        }
        exercise(flow, &forge::seal_raw(forge::TOKEN_VERSION, &body));
    }

    #[test]
    fn random_blobs_are_rejected_cleanly(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let mut with_header = b"FLOW\x01\x00\x00\x00".to_vec();
        with_header.extend(&bytes);
        prop_assert!(Flow::from_bytes(&with_header).is_err());
        let _ = Flow::from_bytes(&bytes);
    }

    #[test]
    fn hashes_ignore_blank_lines_and_comments(which in 0..EXAMPLES.len(), at in any::<prop::sample::Index>()) {
        let src = &sources()[which];
        let flow = Flow::compile(src).unwrap();
        let mut lines: Vec<&str> = src.split('\n').collect();
        lines.insert(at.index(lines.len()), "   # noise");
        if let Ok(noisy) = Flow::compile(&lines.join("\n")) {
            prop_assert_eq!(noisy.hash(), flow.hash());
        }
    }
}

/// The data every run of the seeds' flows gets.
fn seed_data() -> Value {
    from_json(json!({ "evaluatedAt": NOW, "returnId": "R", "vendor": "v", "amount": 90000 }))
}

/// Real tokens to take apart: parked, waiting on an ask, of two flows.
fn token_seeds() -> Vec<(Flow, Token)> {
    let (lifecycle, _) = load("return-lifecycle");
    let (approval, _) = load("spend-approval");
    let token = |flow: &Flow, input: Input| run(flow, input).token().unwrap().clone();
    let requested = token(&lifecycle, Input::start("requested", seed_data()));
    let waiting_parcel = token(
        &lifecycle,
        Input::resume(requested.clone(), seed_data()).event("approve", Value::Null),
    );
    let lines = from_json(json!({ "lines": [1, "x", 2.5] }));
    let inspecting = token(
        &lifecycle,
        Input::resume(waiting_parcel.clone(), seed_data()).event("parcel.received", lines),
    );
    let asking = token(&approval, Input::start("submitted", seed_data()));
    let scored = token(
        &approval,
        Input::resume(asking.clone(), seed_data()).result(from_json(json!({ "score": 1 }))),
    );
    vec![
        (lifecycle.clone(), requested),
        (lifecycle.clone(), waiting_parcel),
        (lifecycle, inspecting),
        (approval.clone(), asking),
        (approval, scored),
    ]
}

/// Everything a host can do with a token. Nothing may panic.
fn exercise(flow: &Flow, token: &Token) {
    let resume = || Input::resume(token.clone(), seed_data());
    let limits = Limits::default();
    let _ = token.inspect();
    let _ = flow.rebind(token);
    let _ = flow.run(resume(), &limits);
    let _ = flow.run(resume().event("approve", Value::Null), &limits);
    let _ = flow.run(resume().event("tick", Value::Null), &limits);
    let _ = flow.run(
        resume().result(from_json(json!({ "passed": true, "acceptedValue": 3 }))),
        &limits,
    );
    let _ = flow.run(resume().failure("x"), &limits);
    assert_eq!(&Token::from_base64(&token.to_base64()).unwrap(), token);
}

/// The tags a token's stored values carry.
const TAGS: [&str; 7] = ["N", "B", "I", "D", "S", "A", "O"];

/// Any CBOR, with the tags and names a token holds mixed in.
fn cbor() -> impl Strategy<Value = Cbor> {
    let names = [
        "requested",
        "awaitingParcel",
        "inspected",
        "pending",
        "stallAfterDays",
        "return-lifecycle",
        "spend-approval",
        "1e30",
        "-0",
        "79228162514264337593543950336",
    ];
    let leaf = prop_oneof![
        any::<i64>().prop_map(|i| Cbor::Integer(i.into())),
        any::<u64>().prop_map(|i| Cbor::Integer(i.into())),
        any::<f64>().prop_map(Cbor::Float),
        any::<bool>().prop_map(Cbor::Bool),
        Just(Cbor::Null),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(Cbor::Bytes),
        "[A-Za-z0-9.eE+\\-_ ]{0,30}".prop_map(Cbor::Text),
        prop::sample::select(TAGS.into_iter().chain(names).collect::<Vec<_>>())
            .prop_map(|text| Cbor::Text(text.into())),
    ];
    leaf.prop_recursive(5, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Cbor::Array),
            prop::collection::vec((inner.clone(), inner.clone()), 0..4).prop_map(Cbor::Map),
            (prop::sample::select(TAGS.to_vec()), inner.clone())
                .prop_map(|(tag, value)| Cbor::Map(vec![(Cbor::Text(tag.into()), value)])),
            (any::<u64>(), inner).prop_map(|(tag, value)| Cbor::Tag(tag, Box::new(value))),
        ]
    })
}

/// The picks [`build`] makes a random edit from; they double as numbers where an edit needs one.
#[derive(Debug, Clone, Copy)]
struct EditChoices {
    kind: u8,
    name: usize,
    other_name: usize,
    snippet: usize,
    other_snippet: usize,
}

fn edit_choices() -> impl Strategy<Value = EditChoices> {
    (0u8..16, 0usize..64, 0usize..64, 0usize..8, 0usize..8).prop_map(
        |(kind, name, other_name, snippet, other_snippet)| EditChoices {
            kind,
            name,
            other_name,
            snippet,
            other_snippet,
        },
    )
}

const NAMES: [&str; 8] = ["fresh", "other", "when", "9bad", "a.b", "tick", "x_1", ""];
const GUARDS: [&str; 8] = [
    "true",
    "input.event == 'approve'",
    "x >",
    "a: b",
    "input.data.n > 3 && true",
    "else",
    "'s'",
    "((",
];
const OUTCOMES: [&str; 8] = [
    "ignore",
    "{ \"ok\": true }",
    "fresh()",
    "ask svc.call(1) -> fresh()",
    "{ \"a\": ",
    "ignore }",
    "{ \"n\": {{ input.data.n }} } wake 5",
    "when { else: ignore }",
];

fn build(document: &Document, choices: EditChoices) -> Edit {
    let EditChoices {
        kind,
        name,
        other_name,
        snippet,
        other_snippet,
    } = choices;
    let graph = document.graph();
    let pick = |names: &[String], k: usize| -> String {
        if names.is_empty() || k % 5 == 0 {
            NAMES[k % NAMES.len()].to_string()
        } else {
            names[k % names.len()].clone()
        }
    };
    let nodes: Vec<String> = graph.nodes.iter().map(|n| n.name.clone()).collect();
    let params: Vec<String> = graph.params.iter().map(|p| p.name.clone()).collect();
    let node = pick(&nodes, name);
    match kind {
        0 => Edit::AddNode {
            name: pick(&[], other_name),
            params: vec![],
            body: OUTCOMES[snippet].into(),
            after: Some(node),
        },
        1 => Edit::RemoveNode { name: node },
        2 => Edit::RenameNode {
            from: node,
            to: pick(&nodes, other_name),
        },
        3 => Edit::AddBranch {
            node,
            guard: GUARDS[snippet].into(),
            outcome: OUTCOMES[other_snippet].into(),
            before: (other_name % 3 == 0).then_some(other_name % 4),
        },
        4 => Edit::RemoveBranch {
            node,
            index: snippet,
        },
        5 => Edit::MoveBranch {
            node,
            from: snippet % 4,
            to: other_snippet % 4,
        },
        6 => Edit::SetGuard {
            node,
            index: snippet % 4,
            guard: GUARDS[other_snippet].into(),
        },
        7 => Edit::SetOutcome {
            node,
            index: snippet % 4,
            outcome: OUTCOMES[other_snippet].into(),
        },
        8 => Edit::SetAnnotation {
            on: Declaration::Node(node),
            name: "pos".into(),
            value: Value::Arr(vec![Value::Int(other_name as i64), Value::from("x'\\\"")]),
        },
        9 => Edit::RemoveAnnotation {
            on: if other_name % 2 == 0 {
                Declaration::Flow
            } else {
                Declaration::Node(node)
            },
            name: "label".into(),
        },
        10 => Edit::AddParam {
            name: pick(&params, other_name),
            default: Value::Int(snippet as i64),
        },
        11 => Edit::RemoveParam {
            name: pick(&params, other_name),
        },
        12 => Edit::AddEvent {
            name: pick(&graph.events, other_name),
        },
        13 => Edit::RemoveEvent {
            name: pick(&graph.events, other_name),
        },
        14 => Edit::AddEntry { node },
        _ => Edit::RemoveEntry { node },
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(300), .. ProptestConfig::default() })]

    #[test]
    fn random_edits_never_break_the_document(which in 0..EXAMPLES.len(), edits in prop::collection::vec(edit_choices(), 1..25)) {
        let mut document = Document::parse(sources()[which].clone());
        for choices in edits {
            let edit = build(&document, choices);
            let before = document.text().to_string();
            let syntax_before = syntax_errors(&before);
            match document.apply(edit.clone()) {
                Ok(()) => {
                    prop_assert!(syntax_errors(document.text()) <= syntax_before, "{edit:?} broke the syntax:\n{}", document.text());
                    let reopened = Document::parse(document.text().to_string());
                    prop_assert_eq!(reopened.graph(), document.graph());
                }
                Err(_) => prop_assert_eq!(document.text(), before.as_str()),
            }
        }
    }
}

/// Parse errors in `text`, told apart from errors of meaning by their wording.
fn syntax_errors(text: &str) -> usize {
    Flow::check(text)
        .iter()
        .filter(|d| d.is_error() && is_syntax(&d.message))
        .count()
}

fn is_syntax(message: &str) -> bool {
    message.starts_with("expected")
        || message.starts_with("unexpected")
        || message.contains("is never closed")
        || message.contains("has no body")
        || message.contains("must start with")
}

#[test]
fn flow_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Flow>();
    assert_send_sync::<Token>();
    assert_send_sync::<Decision>();
}

// WebAssembly has no threads; the type-level check above still runs there.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_flow_is_shared_across_threads() {
    use std::sync::Arc;

    /// An order of one line of `i` items, delivered `i` days ago.
    fn order(i: i64) -> Value {
        from_json(json!({
            "evaluatedAt": NOW, "claimedLines": [],
            "order": { "deliveredAt": NOW - i * DAY, "discountCodes": [], "lines": [{ "lineId": "L", "fulfilledQuantity": i }] }
        }))
    }

    let (flow, _) = load("return-eligibility");
    let flow = Arc::new(flow);
    let expected: Vec<Decision> = (0..64)
        .map(|i| {
            flow.run(Input::start("evaluate", order(i)), &Limits::default())
                .unwrap()
        })
        .collect();
    let expected = Arc::new(expected);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let flow = Arc::clone(&flow);
            let expected = Arc::clone(&expected);
            std::thread::spawn(move || {
                for round in 0..20 {
                    for i in 0..64 {
                        let got = flow
                            .run(Input::start("evaluate", order(i)), &Limits::default())
                            .unwrap();
                        assert_eq!(got, expected[i as usize], "round {round} case {i}");
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}
