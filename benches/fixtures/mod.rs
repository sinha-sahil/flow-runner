//! The flows and data the benchmarks and the throughput example share.
// each program uses some of them
#![allow(dead_code)]

use flow_runner::{Decision, Flow, Input, Limits, Token, Value};
use serde_json::json;

/// The example flows.
pub const EXAMPLES: [&str; 4] = [
    "return-eligibility",
    "return-lifecycle",
    "spend-approval",
    "carrier-quote",
];

/// The time every run is evaluated at, in epoch seconds.
pub const NOW: i64 = 1_774_166_400;

/// An example flow's source.
pub fn source(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/examples/flows/{name}.flow",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `src` compiled; a report of what's wrong fails the run.
pub fn compile(src: &str) -> Flow {
    Flow::compile(src).unwrap_or_else(|errors| panic!("{}", errors.report(src)))
}

/// An example flow, compiled.
pub fn example(name: &str) -> Flow {
    compile(&source(name))
}

/// An order for the eligibility flow with `n` lines, a third of them partly claimed already.
pub fn order(n: usize) -> Value {
    let lines: Vec<_> = (0..n)
        .map(|i| json!({ "lineId": format!("L{i}"), "fulfilledQuantity": 3 }))
        .collect();
    let claimed: Vec<_> = (0..n)
        .step_by(3)
        .map(|i| json!({ "lineId": format!("L{i}"), "quantity": 1 }))
        .collect();
    Value::from(json!({
        "evaluatedAt": NOW,
        "claimedLines": claimed,
        "order": { "deliveredAt": NOW - 5 * 86_400, "discountCodes": ["SALE20"], "lines": lines }
    }))
}

/// The data every run of the lifecycle flow gets.
pub fn return_data() -> Value {
    Value::from(json!({ "evaluatedAt": NOW, "returnId": "R1" }))
}

/// The data every run of the carrier flow gets.
pub fn quote_data() -> Value {
    Value::from(json!({ "pincode": "560001", "weightGrams": 900 }))
}

/// A run's decision, refusing errors so nothing times a failure.
pub fn ok(result: Result<Decision, flow_runner::InputError>) -> Decision {
    let decision = result.unwrap();
    assert!(!decision.is_error(), "{decision:?}");
    decision
}

/// A whole return with the lifecycle flow, driven as a host would, from start to settled refund.
pub fn return_journey(flow: &Flow, data: &Value, limits: &Limits) -> Decision {
    let run = |input: Input| ok(flow.run(input, limits));
    let resume = |token: &Token| Input::resume(token.clone(), data.clone());
    let started = run(Input::start("requested", data.clone()));
    let approved = run(resume(started.token().unwrap()).event("approve", Value::Null));
    let lines = Value::from(json!({ "lines": [{ "lineId": "L1", "quantity": 1 }] }));
    let asked = run(resume(approved.token().unwrap()).event("parcel.received", lines));
    let answer = Value::from(json!({ "passed": true, "acceptedValue": 1299 }));
    let refunding = run(resume(asked.token().unwrap()).result(answer));
    run(resume(refunding.token().unwrap()).event("refund.settled", Value::Null))
}
