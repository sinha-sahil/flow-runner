//! Latency of every public operation, across input sizes.
//!
//! `cargo bench --features json --bench flow_bench`; BENCHMARKS.md has the
//! numbers.

mod fixtures;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fixtures::{
    compile, example, ok, order, quote_data, return_data, return_journey, source, EXAMPLES, NOW,
};
use flow_runner::{Decision, Flow, Input, Limits, Token, Value};
use serde_json::json;
use std::time::Duration;

// The eligibility flow's defaults, which the hand-written version takes as arguments.
const WINDOW_DAYS: i64 = 30;
const RESTRICTED: [&str; 2] = ["FINALSALE", "CLEARANCE"];

/// The eligibility flow written by hand in Rust, the floor the engine is compared against.
fn eligibility_native(data: &Value, window_days: i64, restricted: &[&str]) -> Value {
    static NULL: Value = Value::Null;
    fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
        match value {
            Value::Obj(object) => object.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
    fn int(value: &Value) -> i64 {
        match value {
            Value::Int(number) => *number,
            _ => 0,
        }
    }
    fn arr(value: &Value) -> &[Value] {
        match value {
            Value::Arr(items) => items,
            _ => &[],
        }
    }
    let order = get(data, "order");
    let claimed = arr(get(data, "claimedLines"));
    let expired =
        int(get(data, "evaluatedAt")) - int(get(order, "deliveredAt")) > window_days * 86_400;
    let blocked = arr(get(order, "discountCodes"))
        .iter()
        .any(|c| matches!(c, Value::Str(s) if restricted.contains(&s.as_str())));
    let reasons = Value::Arr(
        ["Damaged", "WrongItem", "NotAsDescribed", "ChangedMind"]
            .iter()
            .map(|s| Value::from(*s))
            .collect(),
    );
    let refusal = if blocked {
        Some("NotReturnable")
    } else if expired {
        Some("WindowExpired")
    } else {
        None
    };
    let lines: Vec<Value> = arr(get(order, "lines"))
        .iter()
        .map(|line| {
            let id = get(line, "lineId");
            let (available, eligible, reason, applicable) = match refusal {
                Some(refused) => (false, 0, Value::from(refused), Value::Arr(Vec::new())),
                None => {
                    let used: i64 = claimed
                        .iter()
                        .filter(|c| get(c, "lineId") == id)
                        .map(|c| int(get(c, "quantity")))
                        .sum();
                    let eligible = int(get(line, "fulfilledQuantity")) - used;
                    (eligible > 0, eligible, Value::Null, reasons.clone())
                }
            };
            Value::obj([
                ("lineId", id.clone()),
                (
                    "actions",
                    Value::obj([(
                        "Return",
                        Value::obj([
                            ("available", Value::Bool(available)),
                            ("eligibleQuantity", Value::Int(eligible)),
                            ("unavailableReason", reason),
                            ("applicableReasons", applicable),
                            ("exchangeOption", Value::Null),
                        ]),
                    )]),
                ),
            ])
        })
        .collect();
    Value::obj([("lines", Value::Arr(lines))])
}

/// `n` nodes linked forward and back by events, each with a let, a helper call and four branches.
fn wide_flow(n: usize) -> String {
    let mut src = String::from(
        "edition 2026\nflow wide\nentry n0\nevents go, back\n\nparam limit = 10\n\nlet twice(x) = x * 2 + limit\n\n\
         node n0 = when {\n  input.event == 'go': n1(1)\n  input.arrived: { \"at\": 0 }\n  else: ignore\n}\n",
    );
    for i in 1..n {
        let back = if i == 1 {
            "n0()".to_string()
        } else {
            format!("n{}(w)", i - 1)
        };
        let next = if i + 1 < n {
            format!("n{}(w % 1000)", i + 1)
        } else {
            "n0()".to_string()
        };
        src.push_str(&format!(
            "\n@label = 'Step {i}'\nnode n{i}(v) =\n  let w = twice(v) + input.data.k\n  when {{\n    \
             input.event == 'go':   {next}\n    input.event == 'back': {back}\n    \
             w > 500:               {{ \"at\": {i}, \"w\": {{{{ w }}}} }}\n    \
             else:                  {{ \"at\": {i}, \"v\": {{{{ v }}}} }}\n  }}\n"
        ));
    }
    src
}

/// `n` nodes, each jumping straight to the next, ending in a value.
fn chain_flow(n: usize) -> String {
    let mut src = String::from("edition 2026\nflow chain\nentry c0\n\nnode c0 = c1(0)\n");
    for i in 1..n {
        if i + 1 < n {
            src.push_str(&format!("node c{i}(x) = c{}(x + 1)\n", i + 1));
        } else {
            src.push_str(&format!("node c{i}(x) = {{ \"hops\": {{{{ x }}}} }}\n"));
        }
    }
    src
}

fn bench_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("compile");
    for name in EXAMPLES {
        let src = source(name);
        group.throughput(Throughput::Bytes(src.len() as u64));
        group.bench_with_input(BenchmarkId::new("example", name), &src, |b, src| {
            b.iter(|| compile(black_box(src)))
        });
    }
    for n in [10, 100, 1000] {
        let src = wide_flow(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("nodes", n), &src, |b, src| {
            b.iter(|| compile(black_box(src)))
        });
    }
    group.finish();

    let mut group = c.benchmark_group("check");
    let broken = source("return-lifecycle")
        .replace("awaitingParcel(input.data", "awaitingParcle(input.data");
    group.bench_function("with-errors", |b| {
        b.iter(|| Flow::check(black_box(&broken)))
    });
    group.finish();
}

fn bench_blob(c: &mut Criterion) {
    let mut group = c.benchmark_group("blob");
    for name in ["return-eligibility", "return-lifecycle"] {
        let flow = example(name);
        let bytes = flow.to_bytes();
        group.bench_function(BenchmarkId::new("to_bytes", name), |b| {
            b.iter(|| black_box(&flow).to_bytes())
        });
        group.bench_function(BenchmarkId::new("from_bytes", name), |b| {
            b.iter(|| Flow::from_bytes(black_box(&bytes)).unwrap())
        });
    }
    let big = compile(&wide_flow(1000));
    let bytes = big.to_bytes();
    group.throughput(Throughput::Elements(1000));
    group.bench_function(BenchmarkId::new("from_bytes", "1000-nodes"), |b| {
        b.iter(|| Flow::from_bytes(black_box(&bytes)).unwrap())
    });
    group.finish();
}

fn bench_eligibility(c: &mut Criterion) {
    let flow = example("return-eligibility");
    let limits = Limits::default();
    let mut group = c.benchmark_group("eligibility");
    for n in [1, 10, 100, 1000] {
        let data = order(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("flow", n), &data, |b, data| {
            b.iter(|| ok(flow.run(Input::start("evaluate", data.clone()), &limits)))
        });
        group.bench_with_input(BenchmarkId::new("native", n), &data, |b, data| {
            b.iter(|| eligibility_native(black_box(data), WINDOW_DAYS, &RESTRICTED))
        });
        // the copy of the data each run takes, which native code doesn't
        group.bench_with_input(BenchmarkId::new("clone-input", n), &data, |b, data| {
            b.iter(|| black_box(data.clone()))
        });
    }
    // the engine and the baseline must agree
    let data = order(10);
    let Decision::Value { value, .. } =
        ok(flow.run(Input::start("evaluate", data.clone()), &limits))
    else {
        panic!("expected a value")
    };
    assert_eq!(value, eligibility_native(&data, WINDOW_DAYS, &RESTRICTED));
    group.finish();
}

fn bench_lifecycle(c: &mut Criterion) {
    let flow = example("return-lifecycle");
    let limits = Limits::default();
    let data = return_data();
    let start = |flow: &Flow| ok(flow.run(Input::start("requested", data.clone()), &limits));
    let Decision::Value {
        token: requested, ..
    } = start(&flow)
    else {
        panic!("expected a value")
    };

    let mut group = c.benchmark_group("lifecycle");
    group.bench_function("start", |b| b.iter(|| start(&flow)));
    group.bench_function("resume/event+jump", |b| {
        b.iter(|| {
            ok(flow.run(
                Input::resume(requested.clone(), data.clone()).event("approve", Value::Null),
                &limits,
            ))
        })
    });
    group.bench_function("resume/ignored", |b| {
        b.iter(|| ok(flow.run(Input::resume(requested.clone(), data.clone()), &limits)))
    });
    group.bench_function("full-journey", |b| {
        b.iter(|| return_journey(&flow, &data, &limits))
    });
    group.finish();
}

fn bench_asks(c: &mut Criterion) {
    let flow = example("carrier-quote");
    let limits = Limits::default();
    let data = quote_data();
    let mut group = c.benchmark_group("ask");
    group.bench_function("start->ask", |b| {
        b.iter(|| ok(flow.run(Input::start("quote", data.clone()), &limits)))
    });
    let Decision::Ask { token, .. } = ok(flow.run(Input::start("quote", data.clone()), &limits))
    else {
        panic!("expected an ask")
    };
    let answer = Value::from(json!({ "serviceable": true, "fee": 80, "etaDays": 2 }));
    group.bench_function("answer->value", |b| {
        b.iter(|| {
            ok(flow.run(
                Input::resume(token.clone(), data.clone()).result(answer.clone()),
                &limits,
            ))
        })
    });
    group.bench_function("failure->retry-ask", |b| {
        b.iter(|| {
            ok(flow.run(
                Input::resume(token.clone(), data.clone()).failure("timeout"),
                &limits,
            ))
        })
    });
    group.finish();
}

fn bench_jumps(c: &mut Criterion) {
    let limits = Limits::default();
    let mut group = c.benchmark_group("jumps");
    for n in [10, 100, 900] {
        let flow = compile(&chain_flow(n));
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("chain", n), |b| {
            b.iter(|| ok(flow.run(Input::start("c0", Value::Null), &limits)))
        });
    }
    let wide = compile(&wide_flow(1000));
    let data = Value::from(json!({ "k": 7 }));
    let Decision::Value { token, .. } = ok(wide.run(Input::start("n0", data.clone()), &limits))
    else {
        panic!("expected a value")
    };
    group.throughput(Throughput::Elements(1));
    group.bench_function("1000-node-flow/event", |b| {
        b.iter(|| {
            ok(wide.run(
                Input::resume(token.clone(), data.clone()).event("go", Value::Null),
                &limits,
            ))
        })
    });
    group.finish();
}

fn bench_tokens(c: &mut Criterion) {
    let flow = example("spend-approval");
    let limits = Limits::default();
    let data = Value::from(json!({ "vendor": "acme", "amount": 90_000, "evaluatedAt": NOW }));
    let Decision::Ask { token, .. } =
        ok(flow.run(Input::start("submitted", data.clone()), &limits))
    else {
        panic!("expected an ask")
    };
    let Decision::Value { token, .. } = ok(flow.run(
        Input::resume(token, data.clone()).result(json!({ "score": 10 })),
        &limits,
    )) else {
        panic!("expected a value")
    };
    let text = token.to_base64();
    let mut group = c.benchmark_group("token");
    group.bench_function("inspect", |b| {
        b.iter(|| black_box(&token).inspect().unwrap())
    });
    group.bench_function("to_base64", |b| b.iter(|| black_box(&token).to_base64()));
    group.bench_function("from_base64", |b| {
        b.iter(|| Token::from_base64(black_box(&text)).unwrap())
    });
    group.bench_function("rebind", |b| {
        b.iter(|| flow.rebind(black_box(&token)).unwrap())
    });
    // a quorum vote: resume, fold the approval into args, park again
    group.bench_function("resume/vote", |b| {
        b.iter(|| {
            ok(flow.run(
                Input::resume(token.clone(), data.clone())
                    .event("approve", json!({ "approverId": "a1" })),
                &limits,
            ))
        })
    });
    group.finish();
}

fn bench_input(c: &mut Criterion) {
    let mut group = c.benchmark_group("host");
    for n in [10, 1000] {
        let json = serde_json::Value::from(order(n));
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("json->value", n), &json, |b, json| {
            b.iter(|| Value::from(black_box(json).clone()))
        });
    }
    group.finish();
}

fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(60)
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_compile, bench_blob, bench_eligibility, bench_lifecycle, bench_asks, bench_jumps, bench_tokens, bench_input
}
criterion_main!(benches);
