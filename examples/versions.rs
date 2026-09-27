//! Shipping a new version of a flow. A flow's hash is its version: layout,
//! comments and annotations don't change it, behavior does. Tokens run only on
//! the version that made them, until the host rebinds them to a new one.
//!
//! ```text
//! cargo run --example versions
//! ```

use flow_runner::{Decision, Flow, Input, InputError, Limits, Value};

const V1: &str = "\
edition 2026
flow approval
entry submitted
events approve

node submitted = when {
  input.event == 'approve': { \"status\": \"APPROVED\" }
  input.arrived:            { \"status\": \"PENDING\" }
  else: ignore
}
";

/// The same flow, laid out and commented differently.
const V1_REFORMATTED: &str = "\
edition 2026
@label = 'Approval'
flow approval
entry submitted
events approve

# waits for a manager
node submitted = when {
  input.event == 'approve': {\"status\":\"APPROVED\"}
  input.arrived: {\"status\":\"PENDING\"}
  else: ignore
}
";

/// A new version: approvals now record who approved.
const V2: &str = "\
edition 2026
flow approval
entry submitted
events approve

node submitted = when {
  input.event == 'approve': { \"status\": \"APPROVED\", \"by\": {{ input.payload.by }} }
  input.arrived:            { \"status\": \"PENDING\" }
  else: ignore
}
";

fn main() {
    let v1 = compile(V1);
    assert_eq!(v1.hash(), compile(V1_REFORMATTED).hash());
    println!("v1 is {}", &v1.hash()[..12]);

    // store the flow as bytes, and load it back where it runs
    let blob = v1.to_bytes();
    let loaded = Flow::from_bytes(&blob).expect("a blob this build wrote");
    assert_eq!(loaded.hash(), v1.hash());
    println!("its blob is {} bytes", blob.len());

    let limits = Limits::default();
    let Ok(Decision::Value { token, .. }) =
        loaded.run(Input::start("submitted", Value::Null), &limits)
    else {
        panic!("a new instance parks at `submitted`")
    };
    let info = token.inspect().expect("a token v1 made");
    println!(
        "an instance waits at `{}` on {} {}",
        info.node,
        info.flow,
        &info.hash[..12]
    );

    let v2 = compile(V2);
    println!("v2 is {}", &v2.hash()[..12]);
    let approve =
        |token| Input::resume(token, Value::Null).event("approve", Value::obj([("by", "ana")]));

    // v2 refuses a token v1 made: the host decides when instances move
    assert_eq!(
        v2.run(approve(token.clone()), &limits),
        Err(InputError::OtherVersion)
    );

    // rebinding checks the token's node and params still exist in v2
    let moved = v2.rebind(&token).expect("`submitted` is still there");
    let Ok(Decision::Value { value, .. }) = v2.run(approve(moved), &limits) else {
        panic!("the rebound instance takes the event")
    };
    assert_eq!(value, Value::obj([("status", "APPROVED"), ("by", "ana")]));
    println!("rebound to v2, the instance is approved by ana");
}

fn compile(src: &str) -> Flow {
    Flow::compile(src).unwrap_or_else(|errors| panic!("{}", errors.report(src)))
}
