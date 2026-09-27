//! Asks: the flow asks for outside data, and the host answers with its own
//! code. Here the first carrier is down, so the flow falls back to the
//! second. The flow never calls anything itself; the host decides what each
//! ask may do.
//!
//! ```text
//! cargo run --example asks --features json
//! ```

use flow_runner::{Decision, Flow, Input, Limits};
use serde_json::{json, Value as Json};
use std::collections::HashMap;

/// Answers an ask, or says why it couldn't.
type Handler = fn(&Json) -> Result<Json, Json>;

fn main() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/flows/carrier-quote.flow"
    );
    let src = std::fs::read_to_string(path).expect("the example flow");
    let flow = Flow::compile(&src).unwrap_or_else(|errors| panic!("{}", errors.report(&src)));

    let handlers: HashMap<&str, Handler> = HashMap::from([("carrier.rates", rates as Handler)]);
    // every ask the flow can make is known when it compiles, so a host can
    // refuse a flow that asks for something it doesn't allow
    for ask in flow.asks() {
        assert!(handlers.contains_key(ask), "no handler for `{ask}`");
    }

    let data = json!({ "pincode": "560001", "weightGrams": 900 });
    let quote = drive(&flow, &handlers, Input::start("quote", data.clone()), &data);
    println!("quote: {quote}");
    assert_eq!(quote["carrier"], "bluedart");
}

/// Run until the flow decides, answering each ask along the way.
fn drive(flow: &Flow, handlers: &HashMap<&str, Handler>, mut input: Input, data: &Json) -> Json {
    let limits = Limits::default();
    loop {
        match flow.run(input, &limits).expect("a well-formed input") {
            Decision::Value { value, .. } => return Json::from(value),
            Decision::Ask {
                name,
                payload,
                token,
                ..
            } => {
                let payload = Json::from(payload);
                let resume = Input::resume(token, data.clone());
                input = match handlers[name.as_str()](&payload) {
                    Ok(result) => {
                        println!("{name}({payload}) answered {result}");
                        resume.result(result)
                    }
                    Err(reason) => {
                        println!("{name}({payload}) failed: {reason}");
                        resume.failure(reason)
                    }
                };
            }
            other => panic!("{other:?}"),
        }
    }
}

/// A carrier's rates, as a host might fetch them. Delhivery is down today.
fn rates(payload: &Json) -> Result<Json, Json> {
    match payload["carrier"].as_str() {
        Some("delhivery") => Err(json!("timeout")),
        Some(_) => Ok(json!({ "serviceable": true, "fee": 80, "etaDays": 2 })),
        None => Err(json!("no carrier given")),
    }
}
