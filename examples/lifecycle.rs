//! A process that takes days. The host stores one token per return, delivers
//! events as they happen, sends a `tick` when the flow asked to wake, and
//! answers the warehouse's ask. The engine keeps nothing between calls.
//!
//! ```text
//! cargo run --example lifecycle --features json
//! ```

use flow_runner::{Decision, Flow, Input, Limits, Token};
use serde_json::{json, Value as Json};
use std::collections::HashMap;

const DAY: i64 = 86_400;
const MONDAY: i64 = 1_774_166_400;

fn main() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/flows/return-lifecycle.flow"
    );
    let src = std::fs::read_to_string(path).expect("the example flow");
    let flow = Flow::compile(&src).unwrap_or_else(|errors| panic!("{}", errors.report(&src)));
    let mut host = Host::new(flow);

    host.start("R-1", MONDAY);
    host.event("R-1", MONDAY + DAY, "approve", Json::Null);

    // the flow asked to be woken if the parcel hasn't arrived by then
    let wake_at = host
        .wake_at("R-1")
        .expect("waiting for the parcel sets a wake time");
    println!(
        "  the host schedules a tick for day {}",
        (wake_at - MONDAY) / DAY
    );
    host.event("R-1", wake_at, "tick", Json::Null);
    // an event the flow ignores where it is changes nothing
    host.event("R-1", wake_at, "approve", Json::Null);

    let lines = json!({ "lines": [{ "lineId": "L1", "quantity": 1 }] });
    host.event("R-1", wake_at + DAY, "parcel.received", lines);
    host.answer(
        "R-1",
        wake_at + DAY,
        json!({ "passed": true, "acceptedValue": 1299 }),
    );
    host.event("R-1", wake_at + 2 * DAY, "refund.settled", Json::Null);

    assert_eq!(host.node("R-1"), "closed");
}

/// What a host keeps: a token per instance, stored as text. A real one keeps
/// them in a database and saves only when the stored sequence number is
/// still the one it loaded, so two workers can't both move an instance.
struct Host {
    flow: Flow,
    tokens: HashMap<String, String>,
    limits: Limits,
}

impl Host {
    fn new(flow: Flow) -> Host {
        Host {
            flow,
            tokens: HashMap::new(),
            limits: Limits::default(),
        }
    }

    fn start(&mut self, id: &str, now: i64) {
        let input = Input::start("requested", data(id, now));
        self.run(id, "start", input);
    }

    fn event(&mut self, id: &str, now: i64, name: &str, payload: Json) {
        let input = Input::resume(self.token(id), data(id, now)).event(name, payload);
        self.run(id, name, input);
    }

    fn answer(&mut self, id: &str, now: i64, result: Json) {
        let input = Input::resume(self.token(id), data(id, now)).result(result);
        self.run(id, "answer", input);
    }

    /// Run the flow and keep what it hands back.
    fn run(&mut self, id: &str, what: &str, input: Input) {
        let decision = self
            .flow
            .run(input, &self.limits)
            .expect("a well-formed input");
        match decision {
            Decision::Value { value, token, .. } => {
                println!("{what:>15}: {}", Json::from(value));
                self.keep(id, &token);
            }
            Decision::Ask {
                name,
                payload,
                token,
                ..
            } => {
                println!("{what:>15}: asks {name} with {}", Json::from(payload));
                self.keep(id, &token);
            }
            Decision::Ignored => println!("{what:>15}: ignored, the stored token stays"),
            Decision::Error { error, .. } => panic!("{id}: {error}"),
            other => panic!("{id}: {other:?}"),
        }
    }

    fn keep(&mut self, id: &str, token: &Token) {
        let info = token.inspect().expect("a token the flow just made");
        println!(
            "{:>15}  at `{}`, sequence {}, {} bytes",
            "",
            info.node,
            info.sequence,
            token.as_bytes().len()
        );
        self.tokens.insert(id.to_string(), token.to_base64());
    }

    fn token(&self, id: &str) -> Token {
        Token::from_base64(&self.tokens[id]).expect("a token this host stored")
    }

    /// Where an instance is, read from its token without running anything.
    fn node(&self, id: &str) -> String {
        self.token(id).inspect().expect("a stored token").node
    }

    fn wake_at(&self, id: &str) -> Option<i64> {
        self.token(id).inspect().expect("a stored token").wake_at
    }
}

/// The data every run gets: what the flow reads as `input.data`.
fn data(id: &str, now: i64) -> Json {
    json!({ "returnId": id, "evaluatedAt": now })
}
