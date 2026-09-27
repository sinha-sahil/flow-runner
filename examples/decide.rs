//! One decision per call: compile a flow once, then run it for each input.
//! Shows params, the trace of branches a run took, and a run that fails on
//! bad data. Values are read with `Value` alone, without the `json` feature.
//!
//! ```text
//! cargo run --example decide
//! ```

use flow_runner::{Decision, Flow, Input, Limits, Value};

const DISCOUNT: &str = "\
edition 2026
flow discount
entry price

param loyalPercent = 10
param bulkFrom = 20

let less(total, percent) = total - total * percent / 100

node price =
  let total = input.data.items.map(item -> item.price * item.quantity).sum()
  let count = input.data.items.map(item -> item.quantity).sum()
  when {
    count >= bulkFrom: bulk(total)
    input.data.loyal:  { \"total\": {{ less(total, loyalPercent) }}, \"reason\": \"loyal\" }
    else:              { \"total\": {{ total }}, \"reason\": \"none\" }
  }

node bulk(total) = { \"total\": {{ less(total, 15) }}, \"reason\": \"bulk\" }
";

fn main() {
    // compile when the flow is saved, not per run; a compiled flow is
    // immutable, so every thread can share one
    let flow =
        Flow::compile(DISCOUNT).unwrap_or_else(|errors| panic!("{}", errors.report(DISCOUNT)));
    let limits = Limits::default();

    let orders = [
        ("a small order", order(false, &[(40, 2), (5, 3)])),
        ("a loyal customer", order(true, &[(40, 2), (5, 3)])),
        ("a bulk order", order(false, &[(4, 25)])),
    ];
    for (label, data) in orders {
        let (decision, trace) = flow
            .run_traced(Input::start("price", data), &limits)
            .expect("the input names a real entry");
        let Decision::Value { value, .. } = decision else {
            panic!("{label}: {decision:?}")
        };
        // the trace says why: each branch taken, never the values
        let path: Vec<String> = trace
            .branches
            .iter()
            .map(|taken| format!("{}[{}] {}", taken.node, taken.branch, taken.kind))
            .collect();
        println!("{label}: {} ({})", describe(&value), path.join(" -> "));
    }

    // a param is fixed for an instance when it starts
    let promotion =
        Input::start("price", order(true, &[(40, 2), (5, 3)])).param("loyalPercent", 25);
    let Ok(Decision::Value { value, .. }) = flow.run(promotion, &limits) else {
        panic!("the promotion should price the order")
    };
    println!("with a 25% promotion: {}", describe(&value));
    assert_eq!(field(&value, "reason"), &Value::from("loyal"));

    // bad data fails the run, not the host: the error points into the flow
    let decision = flow
        .run(
            Input::start("price", Value::obj([("loyal", true)])),
            &limits,
        )
        .expect("the input names a real entry");
    let Decision::Error { error, .. } = decision else {
        panic!("an order without items should fail: {decision:?}")
    };
    println!("an order without items fails:\n{}", error.report(DISCOUNT));
}

/// An order: whether the customer is loyal, and a price and quantity per item.
fn order(loyal: bool, items: &[(i64, i64)]) -> Value {
    let items: Vec<Value> = items
        .iter()
        .map(|&(price, quantity)| Value::obj([("price", price), ("quantity", quantity)]))
        .collect();
    Value::obj([("loyal", Value::from(loyal)), ("items", Value::from(items))])
}

/// A field of an object, or `null`.
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    match value {
        Value::Obj(object) => object.get(key).unwrap_or(&NULL),
        _ => &NULL,
    }
}

fn describe(value: &Value) -> String {
    let total = match field(value, "total") {
        Value::Int(total) => total.to_string(),
        Value::Decimal(total) => total.to_string(),
        other => format!("{other:?}"),
    };
    let Value::Str(reason) = field(value, "reason") else {
        return format!("{value:?}");
    };
    format!("total {total}, discount: {reason}")
}
