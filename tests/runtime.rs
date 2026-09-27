//! Runtime semantics, every way a run can fail, and what tokens carry.
#![cfg(feature = "json")]

mod common;
use ciborium::value::Value as Cbor;
use common::*;
use flow_runner::{
    BranchKind, Decision, Flow, Input, InputError, Limits, RebindError, RunError, Value,
};
use serde_json::json;

#[track_caller]
fn run_error(flow: &Flow, input: Input, limits: &Limits) -> RunError {
    match flow.run(input, limits).unwrap() {
        Decision::Error { error, .. } => error,
        other => panic!("expected an error, got {other:?}"),
    }
}

const HEADER: &str = "edition 2026\nflow t\nentry start\nevents go\n";

#[test]
fn the_event_belongs_to_the_first_node_only() {
    let flow = compile(&format!(
        "{HEADER}node start = when {{\n  input.event == 'go': next()\n  else: {{ \"at\": \"start\" }}\n}}\nnode next = {{ \"event\": {{{{ input.event }}}}, \"arrived\": {{{{ input.arrived }}}} }}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, Value::Null).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "event": null, "arrived": true }));
}

#[test]
fn flow_lets_see_the_event_used_up_after_a_jump() {
    let flow = compile(&format!(
        "{HEADER}let ev = input.event\nlet came = input.arrived\nnode start = when {{\n  input.event == 'go': next()\n  else: {{ \"at\": \"start\" }}\n}}\nnode next = {{ \"ev\": {{{{ ev }}}}, \"came\": {{{{ came }}}} }}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, Value::Null).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "ev": null, "came": true }));
}

#[test]
fn arrived_is_true_on_start_and_false_on_resume() {
    let flow = compile(&format!(
        "{HEADER}node start = when {{\n  input.arrived: {{ \"first\": true }}\n  else: {{ \"first\": false }}\n}}\n"
    ));
    let Parked {
        value: out, token, ..
    } = parked(run(&flow, Input::start("start", Value::Null)));
    assert_eq!(out, json!({ "first": true }));
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, Value::Null).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "first": false }));
}

#[test]
fn params_are_set_at_start_and_carried() {
    let flow = compile(&format!(
        "{HEADER}param n = 1\nnode start = {{ \"n\": {{{{ n }}}} }}\n"
    ));
    let Parked {
        value: out, token, ..
    } = parked(run(&flow, Input::start("start", Value::Null).param("n", 5)));
    assert_eq!(out, json!({ "n": 5 }));
    let Parked { value: out, .. } = parked(run(&flow, Input::resume(token.clone(), Value::Null)));
    assert_eq!(out, json!({ "n": 5 }));
    // a param given to a resume is a mistake, not ignored
    assert_eq!(
        flow.run(
            Input::resume(token, Value::Null).param("n", 9),
            &Limits::default()
        ),
        Err(InputError::ParamOnResume("n".into()))
    );
}

#[test]
fn data_is_fresh_on_every_run() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"v\": {{{{ input.data.v }}}} }}\n"
    ));
    let token = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "v": 1 }))),
    ))
    .token;
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, from_json(json!({ "v": 2 }))),
    ));
    assert_eq!(out, json!({ "v": 2 }));
}

#[test]
fn a_parked_node_keeps_its_arguments() {
    let flow = compile(&format!(
        "{HEADER}node start = hold(41)\nnode hold(n) = when {{\n  input.event == 'go': {{ \"n\": {{{{ n + 1 }}}} }}\n  else: {{ \"n\": {{{{ n }}}} }}\n}}\n"
    ));
    let Parked {
        value: out,
        node,
        token,
        ..
    } = parked(run(&flow, Input::start("start", Value::Null)));
    assert_eq!((out, node.as_str()), (json!({ "n": 41 }), "hold"));
    let Parked {
        value: out, token, ..
    } = parked(run(
        &flow,
        Input::resume(token, Value::Null).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "n": 42 }));
    // the value parks it again with the same argument
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(token, Value::Null).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "n": 42 }));
}

#[test]
fn helpers_flow_lets_and_node_lets_compose() {
    let flow = compile(&format!(
        "{HEADER}param rate = 0.18\nlet tax(x) = x * rate\nlet base = input.data.amount\nnode start =\n  let total = base + tax(base)\n  {{ \"total\": {{{{ total }}}} }}\n"
    ));
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "amount": 100 }))),
    ));
    assert_eq!(out["total"].to_string(), "118.00");
}

#[test]
fn decimals_stay_exact() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"t\": {{{{ input.data.a + input.data.b }}}} }}\n"
    ));
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "a": 0.1, "b": 0.2 }))),
    ));
    assert_eq!(out["t"].to_string(), "0.3");
}

#[test]
fn wake_null_means_no_wake() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"x\": 1 }} wake input.data.when\n"
    ));
    let Parked { wake_at, .. } = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "when": null }))),
    ));
    assert_eq!(wake_at, None);
    let Parked { wake_at, .. } = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "when": 99 }))),
    ));
    assert_eq!(wake_at, Some(99));
}

#[test]
fn wake_takes_whole_decimals() {
    // a decimal param override makes the wake time a decimal
    let (flow, _) = load("return-lifecycle");
    let start =
        Input::start("requested", lifecycle_data()).param("stallAfterDays", from_json(json!(2.0)));
    let token = parked(run(&flow, start)).token;
    let Parked { wake_at, .. } = parked(run(
        &flow,
        Input::resume(token, lifecycle_data()).event("approve", Value::Null),
    ));
    assert_eq!(wake_at, Some(NOW + 2 * DAY));
}

#[test]
fn a_run_that_leaves_the_instance_where_it_was_hands_its_token_back() {
    let flow = compile(&format!(
        "{HEADER}node start = hold(1)\nnode hold(n) = when {{\n  input.event == 'go': hold(n + 1)\n  else: {{ \"n\": {{{{ n }}}}, \"seen\": {{{{ input.data }}}} }}\n}}\n"
    ));
    let kept = parked(run(&flow, Input::start("start", Value::Null))).token;
    assert_eq!(kept.inspect().unwrap().sequence, 1);
    // new data, same place: the same token, byte for byte
    let Parked {
        value: out,
        token: again,
        ..
    } = parked(run(
        &flow,
        Input::resume(kept.clone(), from_json(json!("new"))),
    ));
    assert_eq!(out["seen"], json!("new"));
    assert_eq!(again, kept);
    // a move is a new token, one sequence number on
    let moved = parked(run(
        &flow,
        Input::resume(kept, Value::Null).event("go", Value::Null),
    ))
    .token;
    assert_eq!(moved.inspect().unwrap().sequence, 2);
}

#[test]
fn a_new_wake_time_is_a_move() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"x\": 1 }} wake input.data.at\n"
    ));
    let token = parked(run(
        &flow,
        Input::start("start", from_json(json!({ "at": 5 }))),
    ))
    .token;
    let same = parked(run(
        &flow,
        Input::resume(token.clone(), from_json(json!({ "at": 5 }))),
    ))
    .token;
    assert_eq!(same, token);
    let later = parked(run(
        &flow,
        Input::resume(token, from_json(json!({ "at": 6 }))),
    ))
    .token;
    let info = later.inspect().unwrap();
    assert_eq!((info.sequence, info.wake_at), (2, Some(6)));
}

#[test]
fn tokens_say_what_they_hold() {
    let (flow, _) = load("return-lifecycle");
    let token = parked(run(&flow, Input::start("requested", lifecycle_data()))).token;
    let Parked { token, wake_at, .. } = parked(run(
        &flow,
        Input::resume(token, lifecycle_data()).event("approve", Value::Null),
    ));
    let info = token.inspect().unwrap();
    assert_eq!(info.flow, "return-lifecycle");
    assert_eq!(info.hash, flow.hash());
    assert_eq!(info.node, "awaitingParcel");
    assert_eq!((info.sequence, info.asks), (2, 0));
    assert_eq!((info.ask, info.wake_at), (None, wake_at));
    let waiting = asked(run(
        &flow,
        Input::resume(token, lifecycle_data())
            .event("parcel.received", from_json(json!({ "lines": [] }))),
    ))
    .token;
    let info = waiting.inspect().unwrap();
    assert_eq!(info.ask.as_deref(), Some("warehouse.inspect"));
    assert_eq!(
        (info.node.as_str(), info.asks, info.wake_at),
        ("inspected", 1, None)
    );
}

#[test]
fn a_token_from_another_flow_is_named() {
    let (lifecycle, _) = load("return-lifecycle");
    let (approval, _) = load("spend-approval");
    let token = parked(run(&lifecycle, Input::start("requested", lifecycle_data()))).token;
    let other = Err(InputError::OtherFlow("return-lifecycle".into()));
    assert_eq!(
        approval.run(
            Input::resume(token.clone(), lifecycle_data()),
            &Limits::default()
        ),
        other
    );
    assert_eq!(
        approval.rebind(&token),
        Err(RebindError::Token(InputError::OtherFlow(
            "return-lifecycle".into()
        )))
    );
}

#[test]
fn tokens_made_by_hand_are_refused_not_a_panic() {
    let flow = compile(&format!(
        "{HEADER}param p = 1\nnode start = hold(1)\nnode hold(n) = when {{\n  input.arrived: {{ \"s\": 1 }}\n  else: ignore\n}}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let resume = |token| {
        flow.run(
            Input::resume(token, Value::Null).event("go", Value::Null),
            &Limits::default(),
        )
    };
    let corrupt = |token| matches!(resume(token), Err(InputError::CorruptToken(_)));
    let exhausted = Cbor::Integer(u64::MAX.into());
    assert!(corrupt(forge::with(
        &token,
        forge::SEQUENCE,
        exhausted.clone()
    )));
    assert!(corrupt(forge::with(&token, forge::ASKS, exhausted)));
    assert!(corrupt(forge::with(
        &token,
        forge::NODE,
        Cbor::Text("nope".into())
    )));
    assert!(corrupt(forge::with(
        &token,
        forge::ARGS,
        Cbor::Array(vec![])
    )));
    assert!(corrupt(forge::with(
        &token,
        forge::ASK,
        Cbor::Text("x".into())
    )));
    let bad_decimal = Cbor::Map(vec![(Cbor::Text("D".into()), Cbor::Text("1.2.3".into()))]);
    let bad_arg = forge::with(&token, forge::ARGS, Cbor::Array(vec![bad_decimal]));
    assert!(corrupt(bad_arg.clone()));
    // reading a token skips its state, so it still shows where it is
    assert_eq!(bad_arg.inspect().unwrap().node, "hold");
    let param = |name: &str| {
        Cbor::Array(vec![
            Cbor::Text(name.into()),
            Cbor::Map(vec![(Cbor::Text("I".into()), Cbor::Integer(1.into()))]),
        ])
    };
    for params in [vec![], vec![param("q")], vec![param("p"), param("p")]] {
        assert!(corrupt(forge::with(
            &token,
            forge::PARAMS,
            Cbor::Array(params)
        )));
    }
    let mut body = Vec::new();
    ciborium::ser::into_writer(&Cbor::Array(forge::fields(&token)), &mut body).unwrap();
    // a stray byte after the fields
    body.push(0);
    assert!(corrupt(forge::seal_raw(forge::TOKEN_VERSION, &body)));
    // a newer layout, or a newer meaning, is the host's to upgrade for
    body.pop();
    assert_eq!(
        resume(forge::seal_raw(forge::TOKEN_VERSION + 1, &body)),
        Err(InputError::NewerToken)
    );
    let newer = forge::with(
        &token,
        forge::SEMANTICS,
        Cbor::Integer((forge::SEMANTICS_VERSION + 1).into()),
    );
    assert_eq!(resume(newer.clone()), Err(InputError::NewerToken));
    assert_eq!(
        flow.rebind(&newer),
        Err(RebindError::Token(InputError::NewerToken))
    );
    // the forging itself is sound: an untouched field still resumes
    let same = forge::with(&token, forge::SEQUENCE, Cbor::Integer(7.into()));
    assert!(resume(same).is_ok());
}

#[test]
fn rebind_takes_tokens_made_under_older_semantics() {
    let flow = compile(&format!("{HEADER}node start = {{ \"x\": 1 }}\n"));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let older = forge::with(
        &token,
        forge::SEMANTICS,
        Cbor::Integer((forge::SEMANTICS_VERSION - 1).into()),
    );
    let rebound = flow.rebind(&older).unwrap();
    assert_eq!(
        forge::fields(&rebound)[forge::SEMANTICS],
        Cbor::Integer(forge::SEMANTICS_VERSION.into())
    );
    assert!(flow
        .run(Input::resume(rebound, Value::Null), &Limits::default())
        .is_ok());
}

#[test]
fn the_largest_token_still_resumes() {
    // names at their limit, and state just under its limit
    let name = |prefix: &str| format!("{prefix}{}", "x".repeat(MAX_NAME_BYTES - prefix.len()));
    let (flow_name, node, ask_name) = (name("f"), name("n"), name("a"));
    let src = format!(
        "edition 2026\nflow {flow_name}\nentry start\n\nnode start = ask {ask_name}(null) -> {node}(input.data.s)\nnode {node}(s) = ask {ask_name}(null) -> {node}(s)\n"
    );
    let flow = compile(&src);
    let mut len = MAX_TOKEN_STATE;
    let token = loop {
        let data = Value::obj([("s", Value::Str("x".repeat(len).as_str().into()))]);
        match run(&flow, Input::start("start", data)) {
            Decision::Ask { token, .. } => break token,
            _ => len -= 1,
        }
    };
    let info = token.inspect().unwrap();
    assert_eq!((info.flow, info.node), (flow_name, node));
    assert!(flow
        .run(
            Input::resume(token, Value::Null).result(1),
            &Limits::default()
        )
        .is_ok());
}

#[test]
fn guards_must_be_booleans() {
    let src =
        format!("{HEADER}node start = when {{\n  input.data.flag: ignore\n  else: ignore\n}}\n");
    let flow = compile(&src);
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "flag": "yes" }))),
        &Limits::default(),
    );
    assert!(matches!(error, RunError::GuardNotBool { .. }), "{error:?}");
    let report = error.report(&src);
    assert!(report.contains("--> 6:3"), "{report}");
    assert!(report.contains("true or false"), "{report}");
}

#[test]
fn wake_must_be_whole_seconds() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"x\": 1 }} wake input.data.when\n"
    ));
    // whole, but past the last second an `i64` holds
    let too_late = Value::Decimal("100000000000000000000.0".parse().unwrap());
    for bad in [
        from_json(json!("tomorrow")),
        from_json(json!(1.5)),
        from_json(json!([1])),
        too_late,
    ] {
        let error = run_error(
            &flow,
            Input::start("start", Value::obj([("when", bad)])),
            &Limits::default(),
        );
        assert!(matches!(error, RunError::WakeNotTime { .. }), "{error:?}");
    }
}

#[test]
fn expression_errors_name_the_node_and_line() {
    let src = format!("{HEADER}node start = {{ \"x\": {{{{ 10 / input.data.d }}}} }}\n");
    let flow = compile(&src);
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "d": 0 }))),
        &Limits::default(),
    );
    assert!(error.to_string().contains("in node `start`"), "{error}");
    assert!(error.to_string().contains("divide by zero"), "{error}");
    let report = error.report(&src);
    assert!(report.contains("--> 5:"), "{report}");
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({}))),
        &Limits::default(),
    );
    assert!(error.to_string().contains("missing field `d`"), "{error}");
}

#[test]
fn flow_level_let_errors_have_no_node() {
    let flow = compile(&format!(
        "{HEADER}let x = input.data.missing.deep\nnode start = ignore\n"
    ));
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({}))),
        &Limits::default(),
    );
    assert!(
        matches!(error, RunError::Expression { node: None, .. }),
        "{error:?}"
    );
}

#[test]
fn a_failed_run_can_be_retried_with_the_same_token() {
    let flow = compile(&format!(
        "{HEADER}node start = when {{\n  input.event == 'go': {{ \"r\": {{{{ 100 / input.data.d }}}} }}\n  else: {{ \"waiting\": true }}\n}}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let out = run(
        &flow,
        Input::resume(token.clone(), from_json(json!({ "d": 0 }))).event("go", Value::Null),
    );
    let Decision::Error {
        token: Some(back), ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(back, token, "the error hands back the token it resumed");
    let Parked { value: out, .. } = parked(run(
        &flow,
        Input::resume(back, from_json(json!({ "d": 4 }))).event("go", Value::Null),
    ));
    assert_eq!(out, json!({ "r": 25 }));
}

#[test]
fn a_jump_that_reaches_ignore_is_an_error() {
    let flow = compile(&format!(
        "{HEADER}events other\nnode start = when {{\n  input.event == 'go': next()\n  input.arrived: {{ \"at\": \"start\" }}\n  else: ignore\n}}\nnode next = when {{\n  input.event == 'other': {{ \"at\": \"next\" }}\n  else: ignore\n}}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    match run(
        &flow,
        Input::resume(token.clone(), Value::Null).event("go", Value::Null),
    ) {
        Decision::Error {
            error,
            token: returned,
        } => {
            assert_eq!(
                error,
                RunError::JumpIgnored {
                    node: "next".into()
                }
            );
            assert_eq!(returned, Some(token), "the resumed token comes back");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_answer_that_reaches_ignore_is_an_error() {
    let flow = compile(
        "edition 2026\nflow t\nentry go\n\nnode go = ask svc.check() -> checked()\nnode checked = decide(input.result)\nnode decide(r) = when {\n  r == 'yes': { \"ok\": true }\n  else: ignore\n}\n",
    );
    let token = asked(run(&flow, Input::start("go", Value::Null))).token;
    match run(
        &flow,
        Input::resume(token.clone(), Value::Null).result("nope"),
    ) {
        Decision::Error { error, token: kept } => {
            assert_eq!(
                error,
                RunError::AnswerIgnored {
                    node: "decide".into()
                }
            );
            assert_eq!(kept.as_ref(), Some(&token), "the waiting token comes back");
        }
        other => panic!("{other:?}"),
    }
    let Parked { value: out, .. } =
        parked(run(&flow, Input::resume(token, Value::Null).result("yes")));
    assert_eq!(out, json!({ "ok": true }));
}

#[test]
fn the_budget_stops_runaway_work() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"n\": {{{{ input.data.xs.map(a -> input.data.xs.map(b -> a * b).sum()).sum() }}}} }}\n"
    ));
    let xs: Vec<i64> = (0..300).collect();
    let limits = Limits {
        steps: 50_000,
        ..Limits::default()
    };
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "xs": xs }))),
        &limits,
    );
    assert!(error.is_budget_exceeded(), "{error:?}");
    let big = Limits {
        steps: 10_000_000,
        ..Limits::default()
    };
    assert!(matches!(
        flow.run(Input::start("start", from_json(json!({ "xs": xs }))), &big)
            .unwrap(),
        Decision::Value { .. }
    ));
}

#[test]
fn quadratic_builtins_count_against_the_budget() {
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"n\": {{{{ input.data.xs.unique().len() }}}} }}\n"
    ));
    let xs: Vec<i64> = (0..5_000).collect();
    let limits = Limits {
        steps: 200_000,
        ..Limits::default()
    };
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "xs": xs }))),
        &limits,
    );
    assert!(error.is_budget_exceeded(), "{error:?}");
}

#[test]
fn long_jump_chains_hit_the_jump_limit() {
    let mut src = format!("{HEADER}node start = n0()\n");
    for i in 0..50 {
        src.push_str(&format!("node n{i} = n{}()\n", i + 1));
    }
    src.push_str("node n50 = { \"done\": true }\n");
    let flow = compile(&src);
    assert!(matches!(
        run(&flow, Input::start("start", Value::Null)),
        Decision::Value { .. }
    ));
    let limits = Limits {
        jumps: 10,
        ..Limits::default()
    };
    let (decision, trace) = flow
        .run_traced(Input::start("start", Value::Null), &limits)
        .unwrap();
    let Decision::Error { error, .. } = decision else {
        panic!("{decision:?}")
    };
    assert!(
        matches!(error, RunError::TooManyJumps { limit: 10, ref node } if node == "n9"),
        "{error:?}"
    );
    // ten jumps taken, then the one refused
    let kinds: Vec<BranchKind> = trace.branches.iter().map(|taken| taken.kind).collect();
    assert_eq!(kinds.len(), 11);
    assert!(kinds[..10].iter().all(|&kind| kind == BranchKind::Jump));
    assert_eq!(kinds[10], BranchKind::Error);
}

#[test]
fn the_trace_shows_where_a_run_failed() {
    let flow = compile(&format!(
        "{HEADER}node start = when {{\n  10 / input.data.a > 0: {{ \"x\": 1 }} wake input.data.at\n  else: ignore\n}}\n"
    ));
    for data in [json!({ "a": 0 }), json!({ "a": 1, "at": "soon" })] {
        let (decision, trace) = flow
            .run_traced(Input::start("start", from_json(data)), &Limits::default())
            .unwrap();
        assert!(decision.is_error(), "{decision:?}");
        let last = trace.branches.last().unwrap();
        assert_eq!(
            (last.node.as_str(), last.branch, last.kind),
            ("start", 0, BranchKind::Error)
        );
    }
}

#[test]
fn ask_loops_hit_the_ask_limit() {
    let (flow, _) = load("carrier-quote");
    let limits = Limits {
        asks: 1,
        ..Limits::default()
    };
    let token = asked(
        flow.run(Input::start("quote", quote_data()), &limits)
            .unwrap(),
    )
    .token;
    let error = run_error(
        &flow,
        Input::resume(token, quote_data()).failure("down"),
        &limits,
    );
    assert!(
        matches!(error, RunError::TooManyAsks { limit: 1, .. }),
        "{error:?}"
    );
}

#[test]
fn values_too_deep_for_a_token_are_an_error_not_a_crash() {
    let nested = |depth: usize| (0..depth).fold(json!(1), |inner, _| json!([inner]));
    // through a path, temple's own depth cap stops it
    let flow = compile(&format!(
        "{HEADER}node start = keep(input.data.deep)\nnode keep(d) = {{ \"ok\": true }}\n"
    ));
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "deep": nested(200) }))),
        &Limits::default(),
    );
    assert!(matches!(error, RunError::Expression { .. }), "{error:?}");
    // a host-supplied param is refused before the run starts
    let flow = compile(&format!(
        "{HEADER}param p = []\nnode start = {{ \"ok\": true }}\n"
    ));
    assert_eq!(
        flow.run(
            Input::start("start", Value::Null).param("p", from_json(nested(200))),
            &Limits::default()
        ),
        Err(InputError::ParamTooDeep("p".into()))
    );
    // a value the flow builds deeper than a token holds is an error: 60
    // levels of brackets around 100 levels of data
    let wrapped = format!("{}input.data.d{}", "[".repeat(60), "]".repeat(60));
    let flow = compile(&format!(
        "{HEADER}node start = keep({wrapped})\nnode keep(d) = {{ \"ok\": true }}\n"
    ));
    let error = run_error(
        &flow,
        Input::start("start", from_json(json!({ "d": nested(100) }))),
        &Limits::default(),
    );
    assert!(matches!(error, RunError::ValueTooDeep { .. }), "{error:?}");
}

#[test]
fn nothing_to_do_hands_back_no_token() {
    let flow = compile(&format!(
        "{HEADER}node start = when {{\n  input.arrived: {{ \"x\": 1 }}\n  else: ignore\n}}\n"
    ));
    let token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let ignored = run(
        &flow,
        Input::resume(token.clone(), Value::Null).event("go", Value::Null),
    );
    assert_eq!(ignored, Decision::Ignored);
    assert_eq!(ignored.token(), None);
    // the kept token is as good as before
    assert_eq!(
        run(
            &flow,
            Input::resume(token, Value::Null).event("go", Value::Null)
        ),
        Decision::Ignored
    );
}

#[test]
fn host_mistakes_are_refused_and_nothing_runs() {
    let flow = compile(&format!("{HEADER}param n = 1\nnode start = ignore\n"));
    let limits = Limits::default();
    let start = || Input::start("start", Value::Null);
    assert_eq!(
        flow.run(Input::start("nope", Value::Null), &limits),
        Err(InputError::UnknownEntry("nope".into()))
    );
    assert_eq!(
        flow.run(start().event("stop", Value::Null), &limits),
        Err(InputError::UnknownEvent("stop".into()))
    );
    assert_eq!(
        flow.run(start().param("m", 1), &limits),
        Err(InputError::UnknownParam("m".into()))
    );
    assert!(matches!(
        flow.run(start().param("n", true), &limits),
        Err(InputError::ParamType { .. })
    ));
    // a start answers no ask
    assert_eq!(
        flow.run(start().result(1), &limits),
        Err(InputError::AnswerUnexpected)
    );
    assert_eq!(
        flow.run(start().failure("x"), &limits),
        Err(InputError::AnswerUnexpected)
    );
    // `tick` is always allowed
    assert!(flow
        .run(start().event("tick", Value::Null), &limits)
        .is_ok());
    // messages are sentences a host developer can act on
    assert!(InputError::OtherVersion.to_string().contains("rebind it"));
}

#[test]
fn answers_are_checked() {
    let (flow, _) = load("carrier-quote");
    let waiting = asked(run(&flow, Input::start("quote", quote_data()))).token;
    let resume = || Input::resume(waiting.clone(), quote_data());
    let limits = Limits::default();
    assert_eq!(flow.run(resume(), &limits), Err(InputError::AnswerMissing));
    assert_eq!(
        flow.run(resume().result(1).failure(2), &limits),
        Err(InputError::AnswerAmbiguous)
    );
    // a null failure would read as no failure
    assert_eq!(
        flow.run(resume().failure(Value::Null), &limits),
        Err(InputError::NullFailure)
    );
    assert_eq!(
        flow.run(resume().result(1).event("tick", Value::Null), &limits),
        Err(InputError::EventDuringAsk)
    );
    assert!(flow.run(resume().result(Value::Null), &limits).is_ok());
}

#[test]
fn helpers_that_nest_too_deeply_are_an_error_not_a_crash() {
    on_small_stack(|| {
        // h0 calls h1 at the bottom of a long chain, h1 calls h2, and so on
        let n = 60;
        let helpers: String = (0..n)
            .map(|i| {
                let call = if i + 1 < n {
                    format!("h{}(x)", i + 1)
                } else {
                    "x".into()
                };
                format!("let h{i}(x) = {call}{}\n", " + 1".repeat(80))
            })
            .collect();
        let flow = compile(&format!(
            "{HEADER}{helpers}node start = {{ \"v\": {{{{ h0(1) }}}} }}\n"
        ));
        let error = run_error(
            &flow,
            Input::start("start", Value::Null),
            &Limits::default(),
        );
        assert!(error.to_string().contains("nests more than"), "{error}");
        assert!(error.span().is_some());
    });
}

#[test]
fn copying_data_over_and_over_runs_out_of_budget() {
    // each item copies the whole list again: memory would grow with
    // items × list, so copies are charged by what they copy
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"n\": {{{{ input.data.items.map(i -> input.data.big).len() }}}} }}\n"
    ));
    let data = json!({
        "items": (0..1000).collect::<Vec<_>>(),
        "big": (0..500).map(|i| json!({ "k": i })).collect::<Vec<_>>(),
    });
    let limits = Limits {
        steps: 100_000,
        ..Limits::default()
    };
    let error = run_error(&flow, Input::start("start", from_json(data)), &limits);
    assert!(error.is_budget_exceeded(), "{error}");
    // shared text too: 1000 references to one 100 KB string
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"s\": {{{{ input.data.items.map(i -> input.data.text).join('') }}}} }}\n"
    ));
    let data = json!({ "items": (0..1000).collect::<Vec<_>>(), "text": "x".repeat(100_000) });
    let error = run_error(
        &flow,
        Input::start("start", from_json(data)),
        &Limits::default(),
    );
    assert!(error.is_budget_exceeded(), "{error}");
}

#[test]
fn reading_a_large_value_in_place_is_charged() {
    // each item compares the whole list with null, reading it without a copy
    let flow = compile(&format!(
        "{HEADER}node start = {{ \"ok\": {{{{ input.data.xs.all(x -> input.data.xs != null) }}}} }}\n"
    ));
    let data = json!({ "xs": (0..60_000).collect::<Vec<_>>() });
    let error = run_error(
        &flow,
        Input::start("start", from_json(data)),
        &Limits::default(),
    );
    assert!(error.is_budget_exceeded(), "{error}");
}

#[test]
fn state_too_large_for_a_token_is_an_error() {
    // a node that folds every event's payload into its argument
    let flow = compile(&format!(
        "{HEADER}node start = collect([])\nnode collect(items) = when {{\n  input.event == 'go': collect(items.concat([input.payload]))\n  else: {{ \"n\": {{{{ items.len() }}}} }}\n}}\n"
    ));
    let mut token = parked(run(&flow, Input::start("start", Value::Null))).token;
    let chunk = json!("x".repeat(100_000));
    let mut last = None;
    for _ in 0..20 {
        match flow
            .run(
                Input::resume(token.clone(), Value::Null).event("go", from_json(chunk.clone())),
                &Limits::default(),
            )
            .unwrap()
        {
            Decision::Value { token: next, .. } => token = next,
            Decision::Error {
                error,
                token: returned,
            } => {
                assert_eq!(
                    returned.as_ref(),
                    Some(&token),
                    "the old token is handed back"
                );
                last = Some(error);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let error = last.expect("the token should outgrow its limit");
    assert!(matches!(error, RunError::TokenTooLarge { .. }), "{error}");
    assert!(error.to_string().contains("input.data"), "{error}");
    // the last good token is under the limit and still resumes
    assert!(
        token.as_bytes().len() < MAX_TOKEN_STATE,
        "{}",
        token.as_bytes().len()
    );
    assert!(matches!(
        flow.run(Input::resume(token, Value::Null), &Limits::default())
            .unwrap(),
        Decision::Value { .. }
    ));
}

#[test]
fn deep_host_values_are_refused_not_a_crash() {
    on_small_stack(|| {
        let deep = || {
            let mut value = Value::Int(1);
            for _ in 0..100_000 {
                value = Value::Arr(vec![value]);
            }
            value
        };
        let limits = Limits::default();
        // a param is stored in every token, so it is checked when the run starts
        let flow = compile(&format!(
            "{HEADER}param xs = []\nnode start = {{ \"n\": {{{{ xs.len() }}}} }}\n"
        ));
        assert_eq!(
            flow.run(
                Input::start("start", Value::Null).param("xs", deep()),
                &limits
            ),
            Err(InputError::ParamTooDeep("xs".into()))
        );
        // data is only refused where it would be copied or compared
        let data = Value::obj([("xs", Value::Arr(vec![deep(), Value::Int(2)]))]);
        let flow = compile(&format!(
            "{HEADER}node start = {{ \"n\": {{{{ input.data.xs.len() }}}} }}\n"
        ));
        assert!(matches!(
            flow.run(Input::start("start", data), &limits).unwrap(),
            Decision::Value { .. }
        ));
        for method in ["unique", "reverse"] {
            let data = Value::obj([("xs", Value::Arr(vec![deep(), Value::Int(2)]))]);
            let flow = compile(&format!(
                "{HEADER}node start = {{ \"u\": {{{{ input.data.xs.{method}() }}}} }}\n"
            ));
            let error = run_error(&flow, Input::start("start", data), &limits);
            assert!(error.to_string().contains("deep"), "{method}: {error}");
        }
    });
}

#[test]
fn rebind_refuses_a_parked_token_on_a_node_that_takes_answers() {
    let old = compile("edition 2026\nflow t\nentry start\n\nnode start = when {\n  input.arrived: { \"s\": 1 }\n  else: ignore\n}\n");
    let token = parked(run(&old, Input::start("start", Value::Null))).token;
    let new = compile(
        "edition 2026\nflow t\nentry go\n\nnode go = ask svc.call() -> start()\nnode start = when {\n  input.failure != null: done()\n  else: done()\n}\nnode done = { \"ok\": true }\n",
    );
    assert_eq!(
        new.rebind(&token),
        Err(RebindError::NodeChanged("start".into()))
    );
}

#[test]
fn rebind_refuses_what_a_token_would_outgrow() {
    let old = compile("edition 2026\nflow t\nentry start\n\nnode start = { \"s\": 1 }\n");
    let token = parked(run(&old, Input::start("start", Value::Null))).token;
    // each default fits the 512 KiB a value may take, but three outgrow what a token carries
    let big = format!("'{}'", "x".repeat(400_000));
    let new = compile(&format!(
        "edition 2026\nflow t\nentry start\nparam p = {big}\nparam q = {big}\nparam r = {big}\n\nnode start = {{ \"s\": 1 }}\n"
    ));
    assert_eq!(new.rebind(&token), Err(RebindError::TooLarge));
}

#[test]
fn token_size_is_counted_as_encoded() {
    // 44,000 zeros take about 176 KB, well under the 1 MiB limit
    let flow = compile(&format!(
        "{HEADER}node start = keep(input.data)\nnode keep(xs) = {{ \"n\": {{{{ xs.len() }}}} }}\n"
    ));
    let zeros = Value::Arr(vec![Value::Int(0); 44_000]);
    let Parked {
        value: out, token, ..
    } = parked(run(&flow, Input::start("start", zeros)));
    assert_eq!(out, json!({ "n": 44_000 }));
    assert!(
        token.as_bytes().len() < 200_000,
        "{}",
        token.as_bytes().len()
    );
}

#[test]
fn the_deepest_values_a_token_holds_come_back() {
    on_small_stack(|| {
        let flow = compile(&format!(
            "{HEADER}param p = null\nnode start = hold(input.data.d)\nnode hold(d) = when {{\n  input.event == 'go': hold(d)\n  else: {{ \"ok\": true }}\n}}\n"
        ));
        let nest = |object: bool, depth: usize| {
            let mut value = Value::Int(1);
            for _ in 0..depth {
                value = if object {
                    Value::obj([("a", value)])
                } else {
                    Value::Arr(vec![value])
                };
            }
            value
        };
        for object in [true, false] {
            // as deep as a path reads, and as deep as a param may be
            let starts = [
                Input::start("start", Value::obj([("d", nest(object, 127))])),
                Input::start("start", Value::obj([("d", Value::Null)]))
                    .param("p", nest(object, 128)),
            ];
            for start in starts {
                let token = parked(run(&flow, start)).token;
                assert!(token.inspect().is_ok());
                let resumed = run(
                    &flow,
                    Input::resume(token.clone(), Value::Null).event("go", Value::Null),
                );
                assert!(matches!(resumed, Decision::Value { .. }), "{resumed:?}");
                assert!(flow.rebind(&token).is_ok());
            }
        }
    });
}

#[test]
fn a_negative_zero_stays_negative() {
    let flow = compile(&format!(
        "{HEADER}node start = hold(-input.data.z)\nnode hold(n) = {{ \"n\": {{{{ n }}}} }}\n"
    ));
    let zero = Value::obj([("z", Value::Decimal("0.00".parse().unwrap()))]);
    let Parked {
        value: out,
        token: kept,
        ..
    } = parked(run(&flow, Input::start("start", zero)));
    assert_eq!(out["n"].to_string(), "-0.00");
    let Parked {
        value: out,
        token: again,
        ..
    } = parked(run(&flow, Input::resume(kept.clone(), Value::Null)));
    assert_eq!(out["n"].to_string(), "-0.00");
    assert_eq!(again, kept);
}

#[test]
fn hand_made_tokens_cant_exhaust_the_stack() {
    on_small_stack(|| {
        let flow = compile(&format!(
            "{HEADER}node start = hold(1)\nnode hold(n) = {{ \"n\": {{{{ n }}}} }}\n"
        ));
        let token = parked(run(&flow, Input::start("start", Value::Null))).token;
        let resume = |token| flow.run(Input::resume(token, Value::Null), &Limits::default());
        let decimal = |digits: String| {
            Cbor::Array(vec![Cbor::Map(vec![(
                Cbor::Text("D".into()),
                Cbor::Text(digits),
            )])])
        };
        // a decimal's parser recurses per leading zero
        let long = forge::with(
            &token,
            forge::ARGS,
            decimal(format!("{}1", "0".repeat(10_000))),
        );
        assert!(matches!(resume(long), Err(InputError::CorruptToken(_))));
        // the longest decimal there is, at `MAX_DECIMAL_TEXT` bytes, still reads
        let longest = "-0.0000000000000000000000000001".to_string();
        let fine = forge::with(&token, forge::ARGS, decimal(longest));
        assert!(resume(fine).is_ok());
        // nesting far past any value a token holds: the fields, with the
        // arguments as 100,000 lists inside each other
        let fields = forge::fields(&token);
        let mut body = vec![0x80 | fields.len() as u8];
        for (index, field) in fields.iter().enumerate() {
            if index == forge::ARGS {
                body.extend(std::iter::repeat(0x81).take(100_000));
                body.push(0xf6);
            } else {
                ciborium::ser::into_writer(field, &mut body).unwrap();
            }
        }
        assert!(matches!(
            resume(forge::seal_raw(forge::TOKEN_VERSION, &body)),
            Err(InputError::CorruptToken(_))
        ));
    });
}
