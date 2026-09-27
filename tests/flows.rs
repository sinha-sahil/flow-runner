//! The reference flows, run end to end.
#![cfg(feature = "json")]

mod common;
use common::*;
use flow_runner::{
    AskInfo, Decision, Flow, Input, InputError, Limits, LoadError, Token, Value, BLOB_VERSION,
};
use serde_json::json;
use sha2::{Digest, Sha256};

// Where a blob's version and hash start, and its checksum's length, as in `src/blob.rs`.
const VERSION_AT: usize = 4;
const HASH_AT: usize = 8;
const CHECKSUM_LEN: usize = 8;

fn order(codes: &[&str], delivered_days_ago: i64, claimed: serde_json::Value) -> Value {
    from_json(json!({
        "evaluatedAt": NOW,
        "claimedLines": claimed,
        "order": {
            "deliveredAt": NOW - delivered_days_ago * DAY,
            "discountCodes": codes,
            "lines": [
                { "lineId": "L1", "fulfilledQuantity": 3 },
                { "lineId": "L2", "fulfilledQuantity": 1 }
            ]
        }
    }))
}

#[test]
fn eligibility_counts_what_is_still_open() {
    let (flow, _) = load("return-eligibility");
    let data = order(&["SALE20"], 15, json!([{ "lineId": "L1", "quantity": 1 }]));
    let Parked {
        value: out,
        node,
        wake_at,
        ..
    } = parked(run(&flow, Input::start("evaluate", data)));
    assert_eq!(node, "evaluate");
    assert_eq!(wake_at, None);
    let first = &out["lines"][0]["actions"]["Return"];
    assert_eq!(first["available"], json!(true));
    assert_eq!(first["eligibleQuantity"], json!(2));
    assert_eq!(first["unavailableReason"], json!(null));
    assert_eq!(
        out["lines"][1]["actions"]["Return"]["eligibleQuantity"],
        json!(1)
    );
}

#[test]
fn eligibility_refuses_for_codes_and_the_window() {
    let (flow, _) = load("return-eligibility");
    let blocked = order(&["FINALSALE"], 3, json!([]));
    let Parked {
        value: out, node, ..
    } = parked(run(&flow, Input::start("evaluate", blocked)));
    assert_eq!(node, "refuse");
    assert_eq!(
        out["lines"][0]["actions"]["Return"]["unavailableReason"],
        json!("NotReturnable")
    );
    assert_eq!(
        out["lines"][1]["actions"]["Return"]["available"],
        json!(false)
    );

    let late = order(&[], 31, json!([]));
    let Parked { value: out, .. } = parked(run(&flow, Input::start("evaluate", late)));
    assert_eq!(
        out["lines"][0]["actions"]["Return"]["unavailableReason"],
        json!("WindowExpired")
    );
}

#[test]
fn eligibility_params_come_from_the_host() {
    let (flow, _) = load("return-eligibility");
    let late = order(&[], 31, json!([]));
    let input = Input::start("evaluate", late).param("windowDays", 45);
    let Parked { value: out, .. } = parked(run(&flow, input));
    assert_eq!(
        out["lines"][0]["actions"]["Return"]["available"],
        json!(true)
    );

    let only_clearance = order(&["FINALSALE"], 1, json!([]));
    let input = Input::start("evaluate", only_clearance)
        .param("restricted", from_json(json!(["CLEARANCE"])));
    let Parked { node, .. } = parked(run(&flow, input));
    assert_eq!(node, "evaluate");

    let bad = Input::start("evaluate", order(&[], 1, json!([]))).param("windowDay", 45);
    assert_eq!(
        flow.run(bad, &Limits::default()),
        Err(InputError::UnknownParam("windowDay".into()))
    );
    let bad = Input::start("evaluate", order(&[], 1, json!([]))).param("windowDays", "30");
    assert!(matches!(
        flow.run(bad, &Limits::default()),
        Err(InputError::ParamType { .. })
    ));
}

#[test]
fn eligibility_missing_data_is_an_error_not_a_wrong_answer() {
    let (flow, src) = load("return-eligibility");
    let data = json!({ "evaluatedAt": 1, "claimedLines": [], "ordr": {} });
    match run(&flow, Input::start("evaluate", from_json(data))) {
        Decision::Error { error, token } => {
            assert!(token.is_none());
            let report = error.report(&src);
            assert!(report.contains("order"), "{report}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn lifecycle_happy_path_start_to_close_and_reopen() {
    let (flow, _) = load("return-lifecycle");

    let Parked {
        value: out,
        node,
        token: requested,
        ..
    } = parked(run(&flow, Input::start("requested", lifecycle_data())));
    assert_eq!(
        (out, node.as_str()),
        (json!({ "status": "INITIATED" }), "requested")
    );
    assert_eq!(requested.inspect().unwrap().sequence, 1);

    let Parked {
        value: out,
        node,
        token: approved,
        wake_at,
    } = parked(run(
        &flow,
        Input::resume(requested, lifecycle_data()).event("approve", Value::Null),
    ));
    assert_eq!(
        (out, node.as_str()),
        (json!({ "status": "INPROGRESS" }), "awaitingParcel")
    );
    assert_eq!(wake_at, Some(NOW + 7 * DAY));

    // the tick keeps the instance where it is, with its arguments
    let Parked {
        value: out,
        node,
        token: ticked,
        ..
    } = parked(run(
        &flow,
        Input::resume(approved, lifecycle_data()).event("tick", Value::Null),
    ));
    assert_eq!(out["flag"], json!("STALLED"));
    assert_eq!(node, "awaitingParcel");

    let lines = json!([{ "lineId": "L1", "qty": 1 }]);
    let Asked {
        name,
        payload,
        node: resume_at,
        token: inspecting,
    } = asked(run(
        &flow,
        Input::resume(ticked, lifecycle_data())
            .event("parcel.received", from_json(json!({ "lines": lines }))),
    ));
    assert_eq!(name, "warehouse.inspect");
    assert_eq!(payload, json!({ "returnId": "R-1", "lines": lines }));
    assert_eq!(resume_at, "inspected");
    assert_eq!(
        inspecting.inspect().unwrap().ask.as_deref(),
        Some("warehouse.inspect")
    );

    let Parked {
        value: out,
        node,
        token: refunding,
        ..
    } = parked(run(
        &flow,
        Input::resume(inspecting, lifecycle_data())
            .result(from_json(json!({ "passed": true, "acceptedValue": 1200 }))),
    ));
    assert_eq!(node, "refunding");
    assert_eq!(out["do"][0], json!({ "type": "refund", "amount": 1200 }));

    let Parked {
        value: out,
        node,
        token: closed,
        ..
    } = parked(run(
        &flow,
        Input::resume(refunding, lifecycle_data()).event("refund.settled", Value::Null),
    ));
    assert_eq!(
        (out, node.as_str()),
        (
            json!({ "status": "CLOSED", "reason": "Refunded" }),
            "closed"
        )
    );

    let Parked {
        value: out,
        node,
        token: reopened,
        ..
    } = parked(run(
        &flow,
        Input::resume(closed, lifecycle_data()).event("reopen", Value::Null),
    ));
    assert_eq!(
        (out, node.as_str()),
        (json!({ "status": "INITIATED" }), "requested")
    );
    assert_eq!(reopened.inspect().unwrap().sequence, 7);
    assert_eq!(reopened.inspect().unwrap().asks, 1);
}

#[test]
fn lifecycle_inspection_failure_goes_to_a_person() {
    let (flow, _) = load("return-lifecycle");
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    let token = parked(run(
        &flow,
        Input::resume(token, lifecycle_data()).event("approve", Value::Null),
    ))
    .token;
    let token = asked(run(
        &flow,
        Input::resume(token, lifecycle_data())
            .event("parcel.received", from_json(json!({ "lines": [] }))),
    ))
    .token;
    let Parked {
        value: out,
        node,
        token,
        ..
    } = parked(run(
        &flow,
        Input::resume(token, lifecycle_data()).failure("timeout"),
    ));
    assert_eq!(node, "manualReview");
    assert_eq!(out["flag"], json!("NEEDS_REVIEW"));
    let Parked {
        value: out, node, ..
    } = parked(run(
        &flow,
        Input::resume(token, lifecycle_data())
            .event("approve", from_json(json!({ "amount": 900 }))),
    ));
    assert_eq!(node, "refunding");
    assert_eq!(out["do"][0]["amount"], json!(900));
}

#[test]
fn lifecycle_events_that_do_not_apply_change_nothing() {
    let (flow, _) = load("return-lifecycle");
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    let out = run(
        &flow,
        Input::resume(token.clone(), lifecycle_data()).event("refund.settled", Value::Null),
    );
    assert_eq!(out, Decision::Ignored);
    // an undeclared event is the host's mistake
    assert_eq!(
        flow.run(
            Input::resume(token, lifecycle_data()).event("aprove", Value::Null),
            &Limits::default()
        ),
        Err(InputError::UnknownEvent("aprove".into()))
    );
}

#[test]
fn resuming_with_the_wrong_kind_of_input_is_refused() {
    let (flow, _) = load("return-lifecycle");
    let limits = Limits::default();
    let kept = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    assert_eq!(
        flow.run(
            Input::resume(kept.clone(), lifecycle_data()).result(1),
            &limits
        ),
        Err(InputError::AnswerUnexpected)
    );

    let approved = parked(run(
        &flow,
        Input::resume(kept, lifecycle_data()).event("approve", Value::Null),
    ))
    .token;
    let awaiting = asked(run(
        &flow,
        Input::resume(approved, lifecycle_data())
            .event("parcel.received", from_json(json!({ "lines": [] }))),
    ))
    .token;
    assert_eq!(
        flow.run(Input::resume(awaiting.clone(), lifecycle_data()), &limits),
        Err(InputError::AnswerMissing)
    );
    assert_eq!(
        flow.run(
            Input::resume(awaiting.clone(), lifecycle_data())
                .result(1)
                .failure(2),
            &limits
        ),
        Err(InputError::AnswerAmbiguous)
    );
    assert_eq!(
        flow.run(
            Input::resume(awaiting, lifecycle_data())
                .result(1)
                .event("approve", Value::Null),
            &limits
        ),
        Err(InputError::EventDuringAsk)
    );
}

#[test]
fn tokens_are_checked() {
    let (flow, _) = load("return-lifecycle");
    let (other, _) = load("spend-approval");
    let limits = Limits::default();
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;

    assert_eq!(
        other.run(Input::resume(token.clone(), lifecycle_data()), &limits),
        Err(InputError::OtherFlow("return-lifecycle".into()))
    );
    let bytes = token.as_bytes().to_vec();
    for i in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[i] ^= 0x5a;
        let result = flow.run(
            Input::resume(Token::from_bytes(damaged), lifecycle_data()),
            &limits,
        );
        assert!(
            matches!(result, Err(InputError::CorruptToken(_))),
            "byte {i}: {result:?}"
        );
    }
    for cut in 0..bytes.len() {
        let result = flow.run(
            Input::resume(Token::from_bytes(bytes[..cut].to_vec()), lifecycle_data()),
            &limits,
        );
        assert!(result.is_err(), "cut {cut}");
    }
    assert!(Token::from_base64("not base64!").is_err());
    assert_eq!(Token::from_base64(&token.to_base64()).unwrap(), token);
    assert_eq!(
        flow.run(Input::start("closed", lifecycle_data()), &limits),
        Err(InputError::UnknownEntry("closed".into()))
    );
}

fn approval_data(amount: i64) -> Value {
    from_json(json!({ "vendor": "acme", "amount": amount, "evaluatedAt": NOW }))
}

#[test]
fn approval_auto_approves_small_safe_spend() {
    let (flow, _) = load("spend-approval");
    let data = approval_data(1_000);
    let (out, asked) = drive(
        &flow,
        Input::start("submitted", data.clone()),
        vec![Ok(json!({ "score": 10 }))],
        &data,
    );
    assert_eq!(
        asked,
        vec![(
            "risk.score".to_string(),
            json!({ "vendor": "acme", "amount": 1000 })
        )]
    );
    let Parked {
        value: out, node, ..
    } = parked(out);
    assert_eq!(
        (out, node.as_str()),
        (json!({ "status": "APPROVED", "approvers": [] }), "approved")
    );
}

#[test]
fn approval_rejects_risky_vendors() {
    let (flow, _) = load("spend-approval");
    let data = approval_data(100_000);
    let (out, _) = drive(
        &flow,
        Input::start("submitted", data.clone()),
        vec![Ok(json!({ "score": 95 }))],
        &data,
    );
    let Parked { value: out, .. } = parked(out);
    assert_eq!(
        out,
        json!({ "status": "REJECTED", "reason": "High-risk vendor" })
    );
}

#[test]
fn approval_quorum_lives_in_the_node_args() {
    let (flow, _) = load("spend-approval");
    let data = approval_data(80_000);
    let (out, _) = drive(
        &flow,
        Input::start("submitted", data.clone()),
        vec![Ok(json!({ "score": 20 }))],
        &data,
    );
    let Parked {
        value: out,
        node,
        token,
        wake_at,
    } = parked(out);
    assert_eq!(node, "pending");
    assert_eq!(out["level"], json!("finance"));
    assert_eq!(wake_at, Some(NOW + 3 * DAY));

    let approve = |token: Token, who: &str| {
        run(
            &flow,
            Input::resume(token, data.clone())
                .event("approve", from_json(json!({ "approverId": who }))),
        )
    };
    let Parked {
        value: out,
        node,
        token,
        ..
    } = parked(approve(token, "ana"));
    assert_eq!(
        (out["approvals"].clone(), node.as_str()),
        (json!(["ana"]), "pending")
    );
    // the same approver twice doesn't count twice
    let Parked {
        value: out, token, ..
    } = parked(approve(token, "ana"));
    assert_eq!(out["approvals"], json!(["ana"]));
    let Parked {
        value: out, node, ..
    } = parked(approve(token, "bo"));
    assert_eq!(node, "approved");
    assert_eq!(out["approvers"], json!(["ana", "bo"]));
}

#[test]
fn approval_escalates_and_survives_a_failed_risk_check() {
    let (flow, _) = load("spend-approval");
    let data = approval_data(10_000);
    let (out, _) = drive(
        &flow,
        Input::start("submitted", data.clone()),
        vec![Err(json!("timeout"))],
        &data,
    );
    let Parked {
        value: out,
        node,
        token,
        ..
    } = parked(out);
    assert_eq!(node, "pending");
    assert_eq!(out["note"], json!("Risk check unavailable"));
    assert_eq!(out["level"], json!("manager"));
    let Parked {
        value: out,
        node,
        token,
        wake_at,
    } = parked(run(
        &flow,
        Input::resume(token, data.clone()).event("tick", Value::Null),
    ));
    assert_eq!(
        (out["escalated"].clone(), node.as_str()),
        (json!(true), "pending")
    );
    assert!(wake_at.is_some());
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, data).event("withdraw", Value::Null),
    ));
    assert_eq!(out, json!({ "status": "WITHDRAWN" }));
}

#[test]
fn carrier_quote_paths() {
    let (flow, _) = load("carrier-quote");
    let data = quote_data();
    let start = || Input::start("quote", quote_data());

    let (out, asked) = drive(
        &flow,
        start(),
        vec![Ok(json!({ "serviceable": true, "fee": 49, "etaDays": 3 }))],
        &data,
    );
    assert_eq!(asked[0].1["carrier"], json!("delhivery"));
    assert_eq!(
        parked(out).value,
        json!({ "serviceable": true, "carrier": "delhivery", "fee": 49, "etaDays": 3 })
    );

    let (out, asked) = drive(
        &flow,
        start(),
        vec![
            Err(json!("timeout")),
            Ok(json!({ "serviceable": true, "fee": 60, "etaDays": 2 })),
        ],
        &data,
    );
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].1["carrier"], json!("bluedart"));
    assert_eq!(parked(out).value["carrier"], json!("bluedart"));

    let (out, _) = drive(
        &flow,
        start(),
        vec![Err(json!("x")), Err(json!("y"))],
        &data,
    );
    assert_eq!(
        parked(out).value,
        json!({ "serviceable": false, "reason": "CARRIER_DOWN" })
    );

    let (out, _) = drive(
        &flow,
        start(),
        vec![Ok(json!({ "serviceable": false }))],
        &data,
    );
    assert_eq!(
        parked(out).value,
        json!({ "serviceable": false, "reason": "NOT_SERVICEABLE" })
    );
}

#[test]
fn blobs_round_trip_and_keep_tokens_valid() {
    for name in EXAMPLES {
        let (flow, _) = load(name);
        let bytes = flow.to_bytes();
        let loaded = Flow::from_bytes(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(loaded.hash(), flow.hash(), "{name}");
        assert_eq!(loaded.to_bytes(), bytes, "{name}: bytes are stable");
        assert!(loaded.entries().eq(flow.entries()), "{name}");
        assert!(loaded.asks().eq(flow.asks()), "{name}");
    }
    let (flow, _) = load("return-lifecycle");
    let loaded = Flow::from_bytes(&flow.to_bytes()).unwrap();
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    let from_original = run(
        &flow,
        Input::resume(token.clone(), lifecycle_data()).event("approve", Value::Null),
    );
    let from_loaded = run(
        &loaded,
        Input::resume(token, lifecycle_data()).event("approve", Value::Null),
    );
    assert_eq!(from_original, from_loaded);
}

#[test]
fn damaged_blobs_are_refused() {
    let (flow, _) = load("spend-approval");
    let bytes = flow.to_bytes();
    assert!(Flow::from_bytes(&[]).is_err());
    assert!(Flow::from_bytes(b"FLOW\x01\x00\x00\x00garbage").is_err());
    for i in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[i] ^= 0x20;
        assert!(Flow::from_bytes(&damaged).is_err(), "flip at {i}");
    }
    for cut in 0..bytes.len() {
        assert!(Flow::from_bytes(&bytes[..cut]).is_err(), "cut {cut}");
    }
    // the checksum is checked first, so a damaged version reads as damage
    let mut other_version = bytes.clone();
    other_version[VERSION_AT] = 2;
    assert!(matches!(
        Flow::from_bytes(&other_version),
        Err(LoadError::Corrupt(_))
    ));
    // a real blob of another version says so
    assert!(matches!(
        Flow::from_bytes(&resealed(&other_version)),
        Err(LoadError::IncompatibleVersion {
            found: 2,
            expected: BLOB_VERSION
        })
    ));
    // so does one whose source this flow-runner hashes differently
    let mut other_hash = bytes.clone();
    other_hash[HASH_AT] ^= 1;
    assert!(matches!(
        Flow::from_bytes(&resealed(&other_hash)),
        Err(LoadError::HashChanged)
    ));
}

/// `blob` with its checksum made valid again.
fn resealed(blob: &[u8]) -> Vec<u8> {
    let body = &blob[..blob.len() - CHECKSUM_LEN];
    let mut out = body.to_vec();
    out.extend_from_slice(&Sha256::digest(body)[..CHECKSUM_LEN]);
    out
}

#[test]
fn blobs_are_the_source_plus_a_few_bytes() {
    let (flow, src) = load("return-lifecycle");
    assert_eq!(flow.to_bytes().len(), src.len() + 48);
    assert_eq!(flow.source(), src);
}

#[test]
fn hash_ignores_layout_comments_annotations_and_order() {
    let (flow, src) = load("carrier-quote");
    let noisy = src
        .replace(
            "node quoted",
            "# a comment\n\n@label = 'Quoted!'\nnode   quoted",
        )
        .replace("@label = 'Carrier quote'", "@label = 'Renamed'");
    let reordered = {
        let (head, tail) = src.split_at(src.find("node quote =").unwrap());
        let unavailable = tail.find("node unavailable").unwrap();
        format!("{head}{}\n{}", &tail[unavailable..], &tail[..unavailable])
    };
    for other in [noisy, reordered] {
        assert_eq!(compile(&other).hash(), flow.hash());
    }
    let changed = src.replace("'CARRIER_DOWN'", "'CARRIER_OFFLINE'");
    assert_ne!(compile(&changed).hash(), flow.hash());
}

#[test]
fn rebind_moves_a_live_instance_to_a_fixed_flow() {
    let (flow, src) = load("return-lifecycle");
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    let token = parked(run(
        &flow,
        Input::resume(token, lifecycle_data()).event("approve", Value::Null),
    ))
    .token;

    let fixed = compile(&src.replace(
        "\"flag\": \"STALLED\"",
        "\"flag\": \"STALLED\", \"escalate\": true",
    ));
    assert_eq!(
        fixed.run(
            Input::resume(token.clone(), lifecycle_data()),
            &Limits::default()
        ),
        Err(InputError::OtherVersion)
    );
    let moved = fixed.rebind(&token).unwrap();
    let Parked {
        value: out, node, ..
    } = parked(run(
        &fixed,
        Input::resume(moved, lifecycle_data()).event("tick", Value::Null),
    ));
    assert_eq!(node, "awaitingParcel");
    assert_eq!(out["escalate"], json!(true));

    // a flow where that node lost its parameter refuses
    let broken = compile(
        &src.replace("node awaitingParcel(approvedAt) =", "node awaitingParcel =")
            .replace(
                "dueAt(approvedAt, stallAfterDays)",
                "dueAt(0, stallAfterDays)",
            )
            .replace("awaitingParcel(input.data.evaluatedAt)", "awaitingParcel()"),
    );
    assert!(broken.rebind(&token).is_err());
}

#[test]
fn trace_names_the_path_without_values() {
    let (flow, _) = load("return-eligibility");
    let data = order(&["FINALSALE"], 1, json!([]));
    let (out, trace) = flow
        .run_traced(Input::start("evaluate", data), &Limits::default())
        .unwrap();
    assert!(matches!(out, Decision::Value { .. }), "{out:?}");
    let path: Vec<(&str, usize, &str)> = trace
        .branches
        .iter()
        .map(|s| (s.node.as_str(), s.branch, s.kind.as_str()))
        .collect();
    assert_eq!(path, vec![("evaluate", 0, "jump"), ("refuse", 0, "value")]);
    assert!(trace.steps_used > 0);
}

#[test]
fn introspection_for_builders() {
    let (flow, _) = load("return-lifecycle");
    assert_eq!(flow.name(), "return-lifecycle");
    assert!(flow.entries().eq(["requested"]));
    assert!(flow.events().any(|event| event == "parcel.received"));
    assert!(flow.asks().eq(["warehouse.inspect"]));
    assert_eq!(flow.params()[0].name, "stallAfterDays");
    let inspecting = flow.node("inspecting").unwrap();
    assert_eq!(
        inspecting.asks,
        [AskInfo {
            name: "warehouse.inspect",
            node: "inspected"
        }]
    );
    assert_eq!(inspecting.branch_count, 1);
    assert!(flow.node("inspected").unwrap().resumes_ask);
    let closed = flow.node("closed").unwrap();
    assert_eq!(closed.jumps_to, vec!["requested"]);
    assert_eq!(closed.annotations[0].name, "label");
    assert_eq!(flow.nodes().count(), 8);
    assert!(flow.warnings().is_empty());
}

/// Whether two flows that differ only in `a` and `b` hash alike.
fn same_hash(a: &str, b: &str) -> bool {
    let hash = |body: &str| compile(&format!("edition 2026\nflow f\nentry n\n{body}\n")).hash();
    hash(a) == hash(b)
}

#[test]
fn the_hash_sees_every_change_in_meaning() {
    // text in a string inside a hole
    assert!(!same_hash(
        r#"node n = { "m": "{{ "a b" }}" }"#,
        r#"node n = { "m": "{{ "a  b" }}" }"#
    ));
    // a `#` in such a string is text, not a comment that hides the rest
    assert!(!same_hash(
        r##"node n = { "m": "{{ "#" }}", "refund": 100 }"##,
        r##"node n = { "m": "{{ "#" }}", "refund": 999999 }"##
    ));
    // a line break between list items is not an index
    assert!(!same_hash(
        "node n =\n  let x = [input.data.a\n[0]]\n  { \"x\": {{ x }} }",
        "node n =\n  let x = [input.data.a[0]]\n  { \"x\": {{ x }} }"
    ));
    // an integer is not a decimal, and a decimal keeps its scale
    assert!(!same_hash(
        "param p = 1\nnode n = { \"p\": {{ p }} }",
        "param p = 1.0\nnode n = { \"p\": {{ p }} }"
    ));
    assert!(!same_hash(
        "param p = 1.5\nnode n = { \"p\": {{ p }} }",
        "param p = 1.50\nnode n = { \"p\": {{ p }} }"
    ));
    // nor is a negative zero a zero
    assert!(!same_hash(
        "param p = - 0.0\nnode n = { \"p\": {{ p }} }",
        "param p = 0.0\nnode n = { \"p\": {{ p }} }"
    ));
}

#[test]
fn the_hash_ignores_what_means_nothing() {
    // layout inside a hole
    assert!(same_hash(
        r#"node n = { "m": "hi {{input.data}}" }"#,
        r#"node n = { "m": "hi {{ input.data }}" }"#
    ));
    // the order of params, which are read by name
    assert!(same_hash(
        "param x = 1\nparam y = 2\nnode n = { \"s\": {{ x + y }} }",
        "param y = 2\nparam x = 1\nnode n = { \"s\": {{ x + y }} }"
    ));
    // layout in an expression as deep as temple parses
    let deep = |open: &str, close: &str| {
        format!(
            "let x = {}1{}\nnode n = {{ \"x\": {{{{ x }}}} }}",
            open.repeat(63),
            close.repeat(63)
        )
    };
    assert!(same_hash(&deep("(", ")"), &deep("( ", " )")));
    assert!(same_hash(&deep("(", ")"), &deep("( # open\n", ")")));
}

#[test]
fn a_token_runs_only_on_the_flow_that_made_it() {
    let original = "edition 2026\nflow refund\nentry wait\nevents approve\n\nnode wait = when {\n  input.event == 'approve': { \"note\": \"{{ input.data.ref ?? \"#none\" }}\", \"refund\": 100 }\n  input.arrived: { \"status\": \"WAITING\" }\n  else: ignore\n}\n";
    let changed = original.replace(
        "\"refund\": 100 }",
        "\"refund\": 100000, \"to\": {{ input.data.account }} }",
    );
    let (original, changed) = (compile(original), compile(&changed));
    let token = parked(run(&original, Input::start("wait", Value::Null))).token;
    let resume = Input::resume(token, Value::Null).event("approve", Value::Null);
    assert_eq!(
        changed.run(resume, &Limits::default()),
        Err(InputError::OtherVersion)
    );
}

#[test]
fn a_token_survives_its_params_being_reordered() {
    let ordered = compile("edition 2026\nflow f\nentry n\nparam x = 1\nparam y = 2\nnode n = { \"s\": {{ x + y }} }\n");
    let reordered = compile("edition 2026\nflow f\nentry n\nparam y = 2\nparam x = 1\nnode n = { \"s\": {{ x + y }} }\n");
    let token = parked(run(&ordered, Input::start("n", Value::Null).param("x", 10))).token;
    let Parked { value: out, .. } = parked(run(&reordered, Input::resume(token, Value::Null)));
    assert_eq!(out, json!({ "s": 12 }));
}

/// Stored tokens name a flow by its hash, so a change here breaks every live
/// token: if one is intended, bump the version `common/versions.rs` names for
/// it. The test also runs on WebAssembly, pinning the values across platforms.
#[test]
fn hashes_and_tokens_are_stable() {
    let expected = [
        (
            "return-eligibility",
            "b85b0ca4da9bb6caa7227028ca8d796d568f71934a6e81e7f74a58b6ed238c97",
        ),
        (
            "return-lifecycle",
            "02ce3c5d10ead9c4afda2ad7c98cb62c80700e8f25ec38367e9d37abedb6e91f",
        ),
        (
            "spend-approval",
            "10abbdfbc520b014a295833d79453852c537b495d79e4ee9934c06449222281d",
        ),
        (
            "carrier-quote",
            "8c04093a9ff65b4d31d45848d20c915b69a2fc911492f5c58d0583c63e97d909",
        ),
    ];
    for (name, hash) in expected {
        assert_eq!(load(name).0.hash(), hash, "{name}");
    }
    // text outside ASCII, some of it newer than older compilers' tables
    let unicode = compile(
        "edition 2026\nflow f\nentry a\nparam p = 'Rückgabe — 返品 \u{1C89}\u{0897}'\nnode a = { \"p\": {{ p }} }\n",
    );
    assert_eq!(
        unicode.hash(),
        "e386c00b3c41cdf7badbb527940c80baaa69f58a90beb39af8656d1aa5388230"
    );
    let (flow, _) = load("spend-approval");
    let Asked { token, .. } = asked(run(&flow, Input::start("submitted", approval_data(90_000))));
    assert_eq!(
        token.to_base64(),
        "RlRPSwGKAVggEKu9-8UgsBSilYM9eUU4UsU3tJXXnk7pk0wGRJIiKB1uc3BlbmQtYXBwcm92YWxmcm91dGVkgISCaWF1dG9CZWxvd6FhSRkTiIJsZmluYW5jZUFib3ZloWFJGcNQgmZxdW9ydW2hYUkCgmdzbGFEYXlzoWFJAwEB9mpyaXNrLnNjb3Jl5ZaPTaiY-EY"
    );
}
