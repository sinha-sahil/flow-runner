//! Every compile-time rule, and what the author sees when they break it.

mod common;
use common::{compile, on_small_stack, MAX_NAME_BYTES};
use flow_runner::{Diagnostic, Document, Flow, Severity};

/// A flow: the header, then `extra_header`, then `nodes`.
fn flow(extra_header: &str, nodes: &str) -> String {
    format!("edition 2026\nflow t\nentry start\n{extra_header}\n{nodes}\n")
}

fn errors_in(src: &str) -> Vec<Diagnostic> {
    Flow::check(src)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn warnings_in(src: &str) -> Vec<Diagnostic> {
    Flow::check(src)
        .into_iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect()
}

/// Assert an error containing `needle`, in its message or help, on `line`.
#[track_caller]
fn one_error(src: &str, needle: &str, line: usize) {
    let report = Flow::check(src).report(src);
    let errors = errors_in(src);
    let Some(error) = errors.iter().find(|error| {
        error.message.contains(needle)
            || error
                .help
                .as_deref()
                .is_some_and(|help| help.contains(needle))
    }) else {
        panic!("no error mentioning {needle:?}:\n{report}");
    };
    assert_eq!(
        error.line_col(src).0,
        line,
        "wrong line for {needle:?}:\n{report}"
    );
    assert!(Flow::compile(src).is_err());
}

#[test]
fn edition_is_required_first_and_known() {
    one_error(
        "flow t\nentry s\nnode s = ignore\n",
        "must start with `edition 2026`",
        1,
    );
    one_error(
        "edition 1999\nflow t\nentry s\nnode s = ignore\n",
        "`edition 1999` isn't supported",
        1,
    );
    one_error(
        "flow t\nedition 2026\nentry s\nnode s = ignore\n",
        "must be the first line",
        2,
    );
    one_error(
        "edition\nflow t\nentry s\nnode s = ignore\n",
        "edition year",
        1,
    );
}

#[test]
fn flow_name_and_entry_are_required() {
    one_error(
        "edition 2026\nentry s\nnode s = ignore\n",
        "missing `flow <name>`",
        1,
    );
    one_error(
        "edition 2026\nflow t\nflow u\nentry s\nnode s = ignore\n",
        "declaration `flow` appears twice",
        3,
    );
    one_error("edition 2026\nflow t\nnode s = ignore\n", "no `entry`", 2);
    one_error(&flow("", "node strat = ignore"), "did you mean `strat`", 3);
    one_error(&flow("", "node start(x) = ignore"), "takes parameters", 3);
    one_error(
        "edition 2026\nflow t\nentry s, s\nnode s = ignore\n",
        "entry `s` appears twice",
        3,
    );
}

#[test]
fn empty_and_garbage_files_explain_themselves() {
    assert!(!errors_in("").is_empty());
    one_error(
        "edition 2026\nflow t\nentry s\nnode s = ignore\n?? what\n",
        "expected a declaration: `edition`, `flow`",
        5,
    );
    one_error(
        "edition 2026\nflow t\nentry s\nnode s = ignore\nnodes x = ignore\n",
        "expected a declaration",
        5,
    );
}

#[test]
fn oversized_sources_are_refused() {
    let big = format!(
        "edition 2026\nflow t\nentry s\nnode s = ignore\n#{}\n",
        "x".repeat(flow_runner::MAX_SOURCE_BYTES)
    );
    let errors = errors_in(&big);
    assert!(errors[0].message.contains("too large"), "{errors:?}");
}

#[test]
fn names_are_unique_and_not_reserved() {
    one_error(
        &flow("param start = 1", "node start = ignore"),
        "already declared as a param",
        5,
    );
    one_error(
        &flow("let x = 1\nlet x = 2", "node start = ignore"),
        "already declared as a let",
        5,
    );
    one_error(
        &flow("param input = 1", "node start = ignore"),
        "reserved name",
        4,
    );
    one_error(
        &flow("param wake = 1", "node start = ignore"),
        "reserved name",
        4,
    );
    one_error(
        &flow("let round = 1", "node start = ignore"),
        "built-in function",
        4,
    );
    one_error(
        &flow("param p = 1", "node start = next(1)\nnode next(p) = ignore"),
        "parameters need their own names",
        6,
    );
    one_error(
        &flow("", "node start =\n  let x = 1\n  let x = 2\n  ignore"),
        "already declared",
        7,
    );
    one_error(&flow("", "node start(a, a) = ignore"), "appears twice", 5);
    // temple names start with a letter
    one_error(
        &flow("param _p = 1", "node start = ignore"),
        "not a valid name",
        4,
    );
}

#[test]
fn scopes_follow_declaration_order() {
    // a flow let can only use lets above it
    one_error(
        &flow("let a = b + 1\nlet b = 2", "node start = ignore"),
        "unknown identifier `b`",
        4,
    );
    // a node let can only use node lets above it
    one_error(
        &flow("", "node start =\n  let a = b\n  let b = 1\n  ignore"),
        "unknown identifier `b`",
        6,
    );
    // helpers see params and their own parameters, not flow lets
    one_error(
        &flow("let k = 1\nlet f(x) = x + k", "node start = ignore"),
        "unknown identifier `k`",
        5,
    );
    // a node can't see another node's lets
    one_error(
        &flow(
            "",
            "node start =\n  let mine = 1\n  other()\nnode other = { \"v\": {{ mine }} }",
        ),
        "unknown identifier `mine`",
        8,
    );
    // a misspelt name is matched against everything in scope
    one_error(
        &flow("param total = 1\nlet x = totl + 1", "node start = ignore"),
        "did you mean `total`",
        5,
    );
    // names inside a string's `{{ }}` are in scope too
    compile(&flow(
        "param name = 'Ada'",
        "node start = { \"greeting\": \"hi {{ name }}\" }",
    ));
}

#[test]
fn helpers_are_checked() {
    one_error(
        &flow("let f(x) = f(x)", "node start = ignore"),
        "calls itself",
        4,
    );
    one_error(
        &flow("let f(x) = g(x)\nlet g(x) = f(x)", "node start = ignore"),
        "calls itself",
        4,
    );
    one_error(
        &flow("let f(x) = x", "node start = { \"v\": {{ f(1, 2) }} }"),
        "takes 1 argument",
        5,
    );
    one_error(
        &flow("", "node start =\n  let f(x) = x\n  ignore"),
        "only be declared at the top level",
        6,
    );
    one_error(
        &flow("let f(x, x) = x", "node start = ignore"),
        "appears twice",
        4,
    );
}

#[test]
fn jumps_must_name_real_nodes_with_the_right_arguments() {
    one_error(
        &flow("", "node start = nxet()\nnode next = ignore"),
        "did you mean `next`",
        5,
    );
    one_error(
        &flow("", "node start = next()\nnode next(a) = ignore"),
        "takes 1 argument, got 0",
        5,
    );
    one_error(
        &flow(
            "",
            "node start = ask a.b() -> next(1, 2)\nnode next(a) = next2()\nnode next2 = ignore",
        ),
        "got 2",
        5,
    );
}

#[test]
fn when_blocks_are_exhaustive_and_ordered() {
    one_error(
        &flow("", "node start = when {\n  input.arrived: ignore\n}"),
        "has no `else`",
        5,
    );
    one_error(&flow("", "node start = when {\n}"), "no branches", 5);
    one_error(
        &flow(
            "",
            "node start = when {\n  else: ignore\n  input.arrived: ignore\n}",
        ),
        "never runs",
        7,
    );
}

#[test]
fn outcomes_are_one_of_four_things() {
    one_error(
        &flow(
            "",
            "node start = when {\n  else: next\n}\nnode next = ignore",
        ),
        "write `next()`",
        6,
    );
    one_error(&flow("", "node start = when {\n  else when\n}"), "`:`", 6);
    one_error(
        &flow("", "node start = when {\n  input.arrived: ignore extra\n}"),
        "after this outcome",
        6,
    );
    one_error(
        &flow("", "node start = when {\n  else: ignore\n"),
        "never closed",
        5,
    );
    one_error(&flow("", "node start ="), "has no body", 5);
    one_error(
        &flow("", "node start = ask warehouse.inspect({}) next()"),
        "`->`",
        5,
    );
}

#[test]
fn expression_errors_point_at_the_right_line() {
    one_error(
        &flow(
            "",
            "node start = when {\n  input.x == 'unterminated: ignore\n  else: ignore\n}",
        ),
        "",
        6,
    );
    one_error(
        &flow(
            "",
            "node start = when {\n  nope > 1: ignore\n  else: ignore\n}",
        ),
        "unknown identifier `nope`",
        6,
    );
    one_error(
        &flow("", "node start = { \"a\": {{ inptu.x }} }"),
        "did you mean `input`",
        5,
    );
    one_error(
        &flow("", "node start = { \"a\": {{ upprr(1) }} }"),
        "did you mean `upper`",
        5,
    );
}

#[test]
fn only_declared_events_can_be_compared() {
    one_error(
        &flow(
            "events approve",
            "node start = when {\n  input.event == 'aprove': ignore\n  else: ignore\n}",
        ),
        "did you mean `approve`",
        6,
    );
    one_error(
        &flow(
            "events approve",
            "node start = when {\n  input.event == 'refund': ignore\n  else: ignore\n}",
        ),
        "declare it",
        6,
    );
    one_error(
        &flow(
            "events approve",
            "node start = when {\n  input.event == 3: ignore\n  else: ignore\n}",
        ),
        "never matches",
        6,
    );
    one_error(
        &flow(
            "events approve",
            "node start = when {\n  'aprove' == input?.event: ignore\n  else: ignore\n}",
        ),
        "did you mean `approve`",
        6,
    );
    compile(&flow("events approve", "node start = when {\n  input.event == 'tick': ignore\n  input.event != 'approve': ignore\n  else: ignore\n}"));
    one_error(
        &flow("events a, a", "node start = ignore"),
        "event `a` appears twice",
        4,
    );
    let warnings = warnings_in(&flow("events tick", "node start = ignore"));
    assert!(warnings[0].message.contains("built in"), "{warnings:?}");
}

#[test]
fn ask_continuations_are_entered_only_by_answers() {
    one_error(
        &flow(
            "",
            "node start = ask a.b() -> got()\nnode got = { \"x\": {{ input.result }} }",
        ),
        "must jump or ask again",
        6,
    );
    one_error(
        &flow("", "node start = ask a.b() -> got()\nnode got = done()\nnode done = when {\n  input.arrived: got()\n  else: ignore\n}"),
        "nothing can jump to it",
        6,
    );
    one_error(
        "edition 2026\nflow t\nentry start, got\nnode start = ask a.b() -> got()\nnode got = start()\n",
        "can't be an entry",
        5,
    );
    one_error(
        &flow("", "node start = { \"x\": {{ input.result }} }"),
        "only exist in the node an ask continues at",
        5,
    );
    one_error(
        &flow("let r = input.failure", "node start = ignore"),
        "flow-level lets and helpers",
        4,
    );
    compile(&flow("", "node start = ask a.b() -> got()\nnode got = when {\n  input.failure != null: start()\n  else: done(input.result)\n}\nnode done(r) = { \"r\": {{ r }} }"));
}

#[test]
fn loops_that_need_no_event_are_refused() {
    one_error(&flow("", "node start = start()"), "loop forever", 5);
    one_error(
        &flow("", "node start = a()\nnode a = when {\n  input.arrived: b()\n  else: ignore\n}\nnode b = a()"),
        "`a`, `b`",
        7,
    );
    // a loop that needs an event can go round at most once per run
    compile(&flow(
        "events go",
        "node start = when {\n  input.event == 'go': again()\n  else: { \"at\": \"start\" }\n}\nnode again = start()",
    ));
    // in any place of an `&&` chain, either way round
    for guard in [
        "input.data.ready && input.event == 'go'",
        "'go' == input.event",
        "(input.arrived && (input.event == 'go'))",
    ] {
        compile(&flow(
            "events go",
            &format!("node start = when {{\n  {guard}: start()\n  else: ignore\n}}"),
        ));
    }
    // `!=`, `||` and `? :` don't make a jump need an event
    for guard in [
        "input.event != 'go'",
        "input.event == 'go' || input.arrived",
        "(input.event == 'go' ? true : true)",
    ] {
        one_error(
            &flow(
                "events go",
                &format!("node start = when {{\n  {guard}: start()\n  else: ignore\n}}"),
            ),
            "loop forever",
            6,
        );
    }
}

#[test]
fn unreachable_nodes_are_warned_about() {
    let src = flow("", "node start = ignore\nnode orphan = ignore");
    let compiled = compile(&src);
    let warnings = warnings_in(&src);
    assert!(
        warnings
            .iter()
            .any(|d| d.message.contains("`orphan` can never be reached")),
        "{warnings:?}"
    );
    assert_eq!(compiled.warnings().len(), 1);
    // not while an entry is missing: what can be reached isn't known
    for src in [
        "edition 2026\nflow t\nentry strat\nnode start = next()\nnode next = ignore\n",
        "edition 2026\nflow t\nnode start = next()\nnode next = ignore\n",
    ] {
        let diagnostics = Flow::check(src);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    }
}

#[test]
fn a_node_declared_twice_brings_no_other_errors() {
    // the rules across nodes wait until each node has a name of its own
    let src = flow(
        "",
        "node start = ask check() -> b()\nnode b = when {\n  input.result.ok: done()\n  else: done()\n}\nnode b = done()\nnode done = { \"ok\": true }",
    );
    let diagnostics = Flow::check(&src);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].message.contains("already declared"),
        "{diagnostics:?}"
    );
}

#[test]
fn annotations_and_params_take_literals() {
    one_error(
        &flow("@label = 'x'\nlet k = 1", "node start = ignore"),
        "only be placed on a flow, a param or a node",
        4,
    );
    one_error(
        &flow("@a = 1\n@a = 2\nparam p = 1", "node start = ignore"),
        "appears twice",
        5,
    );
    one_error(
        &flow("@a = input.x\nparam p = 1", "node start = ignore"),
        "must be a literal",
        4,
    );
    one_error(
        &flow("param p = 1 + 1", "node start = ignore"),
        "must be a literal",
        4,
    );
    one_error(&flow("param p", "node start = ignore"), "default value", 4);
    one_error(
        &flow("", "node start = ignore\n@dangling = 1"),
        "not attached",
        6,
    );
    for not_literal in ["(1)", "[1][0]", "'a'.upper()", "-p"] {
        one_error(
            &flow(
                &format!("param p = 1\nparam q = {not_literal}"),
                "node start = ignore",
            ),
            "must be a literal",
            5,
        );
    }
    compile(&flow(
        "@a\n@b = [1, 'x', { \"k\": -2.5 }]\nparam p = { \"a\": [true, null], \"b\"?: null }\nparam q = \"{{ not a hole }}\"",
        "node start = ignore",
    ));
}

#[test]
fn reports_are_readable_snippets() {
    let src = flow("", "node start = nxet()\nnode next = ignore");
    let report = Flow::check(&src).report(&src);
    assert!(report.contains("error: unknown node `nxet`"), "{report}");
    assert!(report.contains("--> 5:14"), "{report}");
    assert!(report.contains("^^^^"), "{report}");
    assert!(report.contains("= help: did you mean `next`?"), "{report}");
    let errors = Flow::compile(&src).unwrap_err();
    assert!(errors.to_string().contains("1 error"), "{errors}");
}

#[test]
fn unicode_is_fine_everywhere_it_is_allowed() {
    compile(&flow(
        "@label = 'Rückgabe — 返品'",
        "# ünïcødé comment\nnode start = { \"msg\": \"héllo {{ input.data.name }} ✓\" }",
    ));
}

#[test]
fn deep_expressions_are_refused_not_a_crash() {
    on_small_stack(|| {
        let chain = format!("1{}", " + 1".repeat(200_000));
        let shapes = [
            flow(
                "",
                &format!("node start =\n  let x = {chain}\n  {{ \"x\": {{{{ x }}}} }}"),
            ),
            flow(
                "",
                &format!("node start = when {{\n  {chain} > 0: ignore\n  else: ignore\n}}"),
            ),
            flow("", &format!("node start = {{ \"x\": {{{{ {chain} }}}} }}")),
            flow(
                "",
                &format!("node start = {}ignore", "true ? ignore : ".repeat(100_000)),
            ),
            flow(
                "",
                &format!(
                    "let deep = null{}\nnode start = ignore",
                    " ?? null".repeat(200_000)
                ),
            ),
        ];
        for src in &shapes {
            let errors = errors_in(src);
            assert!(
                errors
                    .iter()
                    .any(|d| d.message.contains("nests deeper than")
                        || d.message.starts_with("expected")),
                "{errors:?}"
            );
        }
        // the limit is far above anything a person writes
        let fine = flow(
            "",
            &format!("node start = {{ \"x\": {{{{ 1{} }}}} }}", " + 1".repeat(80)),
        );
        assert!(errors_in(&fine).is_empty(), "{:?}", errors_in(&fine));
    });
}

/// A generated flow that stresses the part of the compiler `kind` names, `n` times over.
fn stress(kind: &str, n: usize) -> String {
    let mut src = String::from("edition 2026\nflow big\n");
    let branches = |branch: &dyn Fn(usize) -> String| -> String {
        let body: String = (0..n).map(|i| format!("  {}\n", branch(i))).collect();
        format!("node n0 = when {{\n{body}  else: ignore\n}}\n")
    };
    let names = |prefix: &str| {
        (0..n)
            .map(|i| format!("{prefix}{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match kind {
        "params" => {
            src += "entry n0\n";
            src += &(0..n)
                .map(|i| format!("param p{i} = {i}\n"))
                .collect::<String>();
            src += "node n0 = { \"ok\": true }\n";
            src += &(1..n)
                .map(|i| format!("node n{i}(x) = {{ \"v\": {{{{ x + p{i} }}}} }}\n"))
                .collect::<String>();
        }
        "events" => {
            src += &format!("entry n0\nevents {}\n", names("e"));
            src += &branches(&|i| format!("input.event == 'e{i}': ignore"));
        }
        "undeclared" => {
            src += &format!("entry n0\nevents {}\n", names("e"));
            src += &branches(&|i| format!("input.event == 'x{i}': ignore"));
        }
        "unknown" => {
            src += "entry n0\n";
            src += &branches(&|i| format!("input.data.k == {i}: target{i}()"));
            src += &(0..n)
                .map(|i| format!("node node{i} = ignore\n"))
                .collect::<String>();
        }
        "unknown-names" => {
            // every let reads a name that doesn't exist, among n params
            src += "entry n0\n";
            src += &(0..n)
                .map(|i| format!("param p{i} = {i}\n"))
                .collect::<String>();
            src += &(0..n)
                .map(|i| format!("let l{i} = zzzzzz + p{i}\n"))
                .collect::<String>();
            src += "node n0 = ignore\n";
        }
        "reserved" => {
            // one reserved param name among many good ones
            src += "entry n0\nparam input = 1\n";
            src += &(0..n)
                .map(|i| format!("param p{i} = {i}\n"))
                .collect::<String>();
            src += &(0..n)
                .map(|i| format!("let l{i} = p{i} + 1\n"))
                .collect::<String>();
            src += "node n0 = ignore\n";
        }
        "unclosed" => {
            // temple would read each open value to the end of the file
            src += "entry n0\n";
            src += &(0..n)
                .map(|i| format!("node n{i} = {{\n  \"k\": '\n"))
                .collect::<String>();
        }
        "loop" => {
            // the loop's first node by name jumps out of it n times before
            // it jumps in
            src += "entry n0\n";
            src += &branches(&|i| format!("input.data.k == {i}: out{i}()"))
                .replace("else: ignore", "else: next0()");
            src += &(0..n)
                .map(|i| format!("node out{i} = ignore\nnode next{i} = next{}()\n", i + 1))
                .collect::<String>();
            src += &format!("node next{n} = n0()\n");
        }
        "entries" => {
            src += &format!("entry {}\n", names("n"));
            src += &(0..n)
                .map(|i| format!("node n{i} = ignore\n"))
                .collect::<String>();
        }
        other => panic!("no stress test of kind {other:?}"),
    }
    src
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "times the compiler, so run it with --release"
)]
fn checking_grows_linearly_with_the_file() {
    let time = |src: &str| {
        (0..3)
            .map(|_| {
                let started = std::time::Instant::now();
                let _ = Flow::check(src).report(src);
                started.elapsed()
            })
            .min()
            .unwrap()
    };
    for kind in [
        "params",
        "events",
        "unknown",
        "entries",
        "undeclared",
        "unknown-names",
        "reserved",
        "unclosed",
        "loop",
    ] {
        let small = time(&stress(kind, 500));
        let large = time(&stress(kind, 8000));
        // 16 times the input: linear work takes about 16 times as long,
        // quadratic work about 256 times
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-4);
        assert!(
            ratio < 64.0,
            "{kind}: {small:?} for 500, {large:?} for 8000"
        );
    }
}

#[test]
fn a_short_typo_keeps_its_hint_among_many_long_names() {
    // what a search costs depends on the typo: only names about as long are
    // compared letter by letter
    let steps: String = (0..3000)
        .map(|i| {
            format!("node step_{i:06}_validate_customer_shipping_address_and_region = ignore\n")
        })
        .collect();
    let src = flow(
        "",
        &format!("node start = nxet()\nnode next = ignore\n{steps}"),
    );
    one_error(&src, "did you mean `next`", 5);
}

/// Long names that share most of their letters make each did-you-mean search as slow as it gets.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "times the compiler, so run it with --release"
)]
fn hints_among_long_alike_names_stay_quick() {
    let name = |i: usize| format!("{}{i:08}", "n".repeat(MAX_NAME_BYTES - 8));
    let unknown = |branch: &dyn Fn(String) -> String| -> String {
        let body: String = (0..100)
            .map(|i| format!("  {}\n", branch(name(90_000_000 + i))))
            .collect();
        format!("node start = when {{\n{body}  else: ignore\n}}")
    };
    let many =
        |line: &dyn Fn(String) -> String| -> String { (0..8000).map(|i| line(name(i))).collect() };
    let events: Vec<String> = (0..8000).map(name).collect();
    let sources = [
        flow(
            "",
            &(unknown(&|target| format!("input.data.k == 1: {target}()"))
                + "\n"
                + &many(&|node| format!("node {node} = ignore\n"))),
        ),
        flow(
            &many(&|param| format!("param {param} = 1\n")),
            &unknown(&|variable| format!("{variable} > 0: ignore")),
        ),
        flow(
            &format!("events {}", events.join(", ")),
            &unknown(&|event| format!("input.event == '{event}': ignore")),
        ),
    ];
    for src in &sources {
        let started = std::time::Instant::now();
        let errors = errors_in(src);
        let took = started.elapsed();
        assert!(errors.len() >= 100, "{}", errors.len());
        assert!(took.as_secs_f64() < 1.0, "{took:?}");
    }
}

#[test]
fn long_jump_chains_are_checked_without_recursion() {
    on_small_stack(|| {
        let n = 50_000;
        let chain: String = (0..n)
            .map(|i| format!("node n{i} = n{}()\n", i + 1))
            .collect();
        let open =
            format!("edition 2026\nflow f\nentry n0\n{chain}node n{n} = {{ \"done\": true }}\n");
        let errors = errors_in(&open);
        assert!(errors.is_empty(), "{:?}", errors.first());
        // the same chain closed into a loop is found and reported once
        let closed = open.replace(
            &format!("node n{n} = {{ \"done\": true }}"),
            &format!("node n{n} = n0()"),
        );
        let errors = errors_in(&closed);
        assert_eq!(errors.len(), 1, "{:?}", errors.first().map(|e| &e.message));
        assert!(errors[0].message.contains("loop"), "{}", errors[0].message);
    });
}

#[test]
fn long_helper_chains_are_refused_without_recursion() {
    on_small_stack(|| {
        let n = 50_000;
        let helpers: String = (0..n)
            .map(|i| {
                if i + 1 < n {
                    format!("let h{i}(x) = h{}(x)\n", i + 1)
                } else {
                    format!("let h{i}(x) = x\n")
                }
            })
            .collect();
        let errors = errors_in(&flow(&helpers, "node start = { \"v\": {{ h0(1) }} }"));
        assert!(!errors.is_empty());
        assert!(
            errors[0].message.contains("nest deeper than 64"),
            "{}",
            errors[0].message
        );
    });
}

#[test]
fn comparing_the_event_with_null_does_not_break_a_loop() {
    let src = flow(
        "",
        "node start = when {\n  input.event == null: other()\n  else: ignore\n}\nnode other = start()",
    );
    let errors = errors_in(&src);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("loop"), "{errors:?}");
}

#[test]
fn reports_on_one_long_line_stay_small() {
    // a whole flow on one line, with a warning for each of 10,000 nodes
    let nodes: String = (0..10_000)
        .map(|i| format!(" node n{i} = ignore"))
        .collect();
    let src = format!("edition 2026\nflow f entry start node start = ignore{nodes}\n");
    let diagnostics = Flow::check(&src);
    assert_eq!(diagnostics.len(), 10_000);
    let started = std::time::Instant::now();
    let report = diagnostics.report(&src);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    assert!(report.len() < 10_000 * 600, "{} bytes", report.len());
    // each snippet still points at its own node, with the right column
    let last = report.rsplit("\n\n").next().unwrap();
    assert!(last.contains("node `n9999` can never be reached"), "{last}");
    let column = src.lines().nth(1).unwrap().find("node n9999").unwrap() + 1;
    assert!(last.contains(&format!("--> 2:{column}")), "{last}");
    assert!(
        last.contains('…') && last.contains("node n9999 = ignore"),
        "{last}"
    );
}

#[test]
fn a_let_in_a_guard_does_not_hide_a_loop() {
    // with no event the guard holds, so the jump would repeat forever
    one_error(
        &flow(
            "events go",
            "node start = when {\n  let v = true && input.event == 'go' && true in !v: start()\n  else: ignore\n}",
        ),
        "loop forever",
        6,
    );
}

#[test]
fn text_in_a_value_is_not_an_event_test() {
    compile(&flow(
        "events go",
        r#"node start = { "m": "{{ "input.event == 5" }}", "k#": "input.event == 6" }"#,
    ));
    // a test inside a hole still is
    one_error(
        &flow(
            "events go",
            r#"node start = { "m": "{{ input.event == 'og' ? 1 : 2 }}" }"#,
        ),
        "did you mean `go`",
        5,
    );
}

#[test]
fn spans_never_split_a_character() {
    for src in [
        flow("", "node é = ignore"),
        flow("", "node start = when {\n  é: ignore\n  else: ignore\n}"),
        flow("", "node start = ignore é"),
        // temple's own spans, held to its text and to whole characters
        flow("", "node start = { \"v\": {{ 1 € }} }"),
        flow("", "node start = { \"v\": {{ 1\n@é = 1\nnode b = ignore"),
        "let f(a) = let".to_string(),
    ] {
        let diagnostics = Flow::check(&src);
        assert!(!diagnostics.is_empty(), "{src:?}");
        for diagnostic in &diagnostics {
            assert!(src.get(diagnostic.span.range()).is_some(), "{diagnostic:?}");
        }
    }
}

#[test]
fn a_broken_helper_is_reported_once() {
    let errors = errors_in(&flow(
        "let bad(x) = zzz + x\nlet good(y) = bad(y)",
        "node start = { \"v\": {{ good(1) }} }",
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("zzz"), "{errors:?}");
}

#[test]
fn a_missing_flow_line_does_not_hide_the_other_errors() {
    let assert_errors_mention = |src: &str, expected: &[&str]| {
        let messages: Vec<String> = errors_in(src).into_iter().map(|d| d.message).collect();
        for needle in expected {
            assert!(
                messages.iter().any(|m| m.contains(needle)),
                "{needle}: {messages:?}"
            );
        }
    };
    assert_errors_mention(
        "edition 2026\nentry n\nnode n = { \"v\": {{ nope }} }\nnode m = zz()\n",
        &["missing `flow <name>`", "`nope`", "`zz`"],
    );
    // nor the rules that need every node compiled, after an error elsewhere
    let nodes = "entry start\nevents go\nnode start = when {\n  input.event == 'og': start()\n  input.data.x: other()\n  else: ignore\n}\nnode other = start()\n";
    for header in [
        "edition 2026\n",
        "edition 2026\nflow f\nparam p = input.data\n",
    ] {
        assert_errors_mention(
            &format!("{header}{nodes}"),
            &["not a declared event", "loop forever"],
        );
    }
}

#[test]
fn names_have_a_length_limit() {
    let longest = "n".repeat(MAX_NAME_BYTES);
    compile(&flow(
        "",
        &format!("node start = {longest}()\nnode {longest} = {{ \"ok\": true }}"),
    ));
    let longer = "n".repeat(MAX_NAME_BYTES + 1);
    let too_large = format!("the name is too large: {} bytes", longer.len());
    one_error(
        &flow(
            "",
            &format!("node start = {longer}()\nnode {longer} = ignore"),
        ),
        &too_large,
        5,
    );
    // each part is short, the whole name is not
    one_error(
        &flow(
            &format!("events {}", "e.".repeat(200) + "e"),
            "node start = ignore",
        ),
        "the name is too large",
        4,
    );
    // names inside expressions too, but not text in a string
    for value in [
        format!("{{{{ let {longer} = 1 in 2 }}}}"),
        format!("{{{{ input.data.{longer} }}}}"),
        format!("{{ \"k\": 1, \"v\": {{{{ this.{longer} }}}} }}"),
    ] {
        one_error(&flow("", &format!("node start = {value}")), &too_large, 5);
    }
    compile(&flow(
        "",
        &format!("node start = {{ \"v\": {{{{ '{longer}' }}}} }}"),
    ));
}

#[test]
fn expressions_and_values_have_a_size_limit() {
    let list = |items: usize| format!("[{}]", vec!["1"; items].join(", "));
    // the limit, 512 KiB, is about 175,000 items of three bytes
    compile(&flow(
        "",
        &format!("node start = {{ \"v\": {} }}", list(170_000)),
    ));
    one_error(
        &flow("", &format!("node start = {{ \"v\": {} }}", list(180_000))),
        "the value is too large",
        5,
    );
    one_error(
        &flow("", &format!("node start = f({})", list(180_000))),
        "the expression is too large",
        5,
    );
}

#[test]
fn a_value_left_open_ends_at_the_next_declaration() {
    let src = flow(
        "",
        "node start = {\n  \"a\": 1\nnode after = { \"ok\": true }\n@label = 'x'\nnode last = [",
    );
    let errors = errors_in(&src);
    assert_eq!(errors.len(), 2, "{errors:?}");
    // where the open value's own text ends, and at the open bracket
    assert_eq!(errors[0].line_col(&src), (6, 9), "{errors:?}");
    assert_eq!(errors[1].line_col(&src).0, 9, "{errors:?}");
    assert!(Document::parse(src)
        .graph()
        .nodes
        .iter()
        .any(|node| node.name == "after"));
}

#[test]
fn an_error_where_a_declaration_runs_out_stays_in_it() {
    // each runs into the next node, and the error is on its own last line
    for (nodes, needle) in [
        (
            "node start(x,\nnode b = ignore",
            "expected a parameter name",
        ),
        (
            "node start = b(1,\nnode b(x) = ignore",
            "expected an expression",
        ),
        (
            "node start = ask check(1) ->\nnode b = ignore",
            "expected the node to continue at",
        ),
        (
            "node start = { \"v\": {{ 1\n\n# the next node\nnode b = ignore",
            "expected `}}`",
        ),
    ] {
        one_error(&flow("", nodes), needle, 5);
    }
    // just past its last token: before a comment on that line, after a
    // string that holds a line like a comment, and at the end of the file
    for (nodes, line_col) in [
        ("node start = b(1,  # one\nnode b(x, y) = ignore", (5, 18)),
        ("node start = b('x\n# y', 1,\nnode b(x, y) = ignore", (6, 9)),
        ("node start = b(1,\n\n# the end", (5, 18)),
    ] {
        let src = flow("", nodes);
        let errors = errors_in(&src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].line_col(&src), line_col, "{nodes:?}");
    }
}

#[test]
fn a_column_zero_let_ends_a_node() {
    one_error(&flow("", "node start =\nlet x = 1"), "has no body", 5);
}

#[test]
fn an_error_at_the_next_declaration_keeps_it() {
    // the `when` runs into the next node, which still parses
    let src = flow(
        "",
        "node start = when {\n  input.arrived: next()\nnode next = { \"ok\": true }",
    );
    let errors = errors_in(&src);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("never closed"), "{errors:?}");
    assert!(Document::parse(src)
        .graph()
        .nodes
        .iter()
        .any(|node| node.name == "next"));
    // and a list can't run into it
    one_error(
        "edition 2026\nflow t\nentry start,\nnode start = ignore\n",
        "expected a node name after `,`",
        3,
    );
}
