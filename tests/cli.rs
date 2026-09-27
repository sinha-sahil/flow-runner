//! The `flow` command, run as a user would.
#![cfg(feature = "cli")]

mod common;
use common::{lifecycle_data, load, to_json, DAY, NOW};
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn flow(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flow"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run flow")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[track_caller]
fn printed_json(output: &Output) -> Json {
    match serde_json::from_str(&stdout(output)) {
        Ok(json) => json,
        Err(error) => panic!("{error}\n{}\n{}", stdout(output), stderr(output)),
    }
}

/// A directory for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("flow-cli-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    /// Copy the file at `path` in, under its own name.
    fn copy(&self, path: &str) {
        let name = Path::new(path).file_name().unwrap();
        std::fs::copy(path, self.0.join(name)).unwrap();
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const LIFECYCLE: &str = "examples/flows/return-lifecycle.flow";

#[test]
fn check_passes_clean_files_and_reports_broken_ones() {
    let ok = flow(&["check", "examples/flows/return-eligibility.flow", LIFECYCLE]);
    assert!(ok.status.success(), "{}", stderr(&ok));
    let out = stdout(&ok);
    assert!(out.contains("return-lifecycle.flow: ok"), "{out}");

    let scratch = Scratch::new("check");
    let src = std::fs::read_to_string(LIFECYCLE)
        .unwrap()
        .replace("closed('Refunded')", "closd('Refunded')");
    let path = scratch.file("broken.flow", &src);
    let bad = flow(&["check", &path]);
    let out = stdout(&bad);
    assert_eq!(bad.status.code(), Some(1), "{out}");
    assert!(out.contains("unknown node `closd`"), "{out}");
    assert!(out.contains(&format!("--> {path}:")), "{out}");
    assert!(out.contains("did you mean `closed`?"), "{out}");

    let missing = flow(&["check", "no/such.flow"]);
    assert_eq!(missing.status.code(), Some(2));
}

#[test]
fn run_starts_and_resumes_with_the_token() {
    let data = to_json(&lifecycle_data()).to_string();
    let started = flow(&["run", LIFECYCLE, "--data", &data]);
    assert!(started.status.success(), "{}", stderr(&started));
    let out = printed_json(&started);
    assert_eq!(out["decision"], "value");
    assert_eq!(out["value"], json!({ "status": "INITIATED" }));
    let token = out["token"].as_str().unwrap();

    let approved = flow(&[
        "run", LIFECYCLE, "--token", token, "--data", &data, "--event", "approve", "--trace",
    ]);
    let out = printed_json(&approved);
    assert_eq!(out["node"], "awaitingParcel");
    assert_eq!(out["wakeAt"], NOW + 7 * DAY);
    assert_eq!(
        out["trace"][0],
        json!({ "node": "requested", "branch": 0, "kind": "jump" })
    );

    let params = flow(&[
        "run",
        "examples/flows/carrier-quote.flow",
        "--data",
        r#"{"pincode":"1","weightGrams":1}"#,
        "--param",
        "primary=ecom",
    ]);
    let out = printed_json(&params);
    assert_eq!(out["decision"], "ask");
    assert_eq!(out["payload"]["carrier"], "ecom");
}

#[test]
fn run_reports_mistakes_plainly() {
    // the host's mistakes exit 2
    for (args, message) in [
        (&["--event", "nope"][..], "`nope` is not a declared event"),
        (&["--token", "garbage"], "corrupt token"),
        (&["--result", "1"], "--token"),
        (&["--data", "{not json"], "not valid JSON"),
        (&["--payload", "{}"], "--payload goes with an --event"),
        (&["--frobnicate"], "unknown option"),
    ] {
        let refused = flow(&[&["run", LIFECYCLE][..], args].concat());
        let errors = stderr(&refused);
        assert_eq!(refused.status.code(), Some(2), "{args:?}");
        assert!(errors.contains(message), "{args:?}: {errors}");
    }

    // the flow's mistakes exit 1: a run that fails, or a file that doesn't compile
    let failed = flow(&[
        "run",
        "examples/flows/return-eligibility.flow",
        "--data",
        "{}",
    ]);
    assert_eq!(failed.status.code(), Some(1));
    let (out, errors) = (stdout(&failed), stderr(&failed));
    assert!(out.contains("\"decision\": \"error\""), "{out}");
    assert!(errors.contains("return-eligibility.flow:"), "{errors}");

    let scratch = Scratch::new("run");
    let path = scratch.file(
        "broken.flow",
        "edition 2026\nflow f\nentry a\nnode a = b()\n",
    );
    let broken = flow(&["run", &path]);
    let errors = stderr(&broken);
    assert_eq!(broken.status.code(), Some(1));
    assert!(errors.contains("unknown node `b`"), "{errors}");
}

#[test]
fn test_runs_every_example() {
    let tested = flow(&["test", "examples/flows"]);
    let out = stdout(&tested);
    assert!(tested.status.success(), "{out}\n{}", stderr(&tested));
    assert!(out.contains(" passed, 0 failed"), "{out}");
}

#[test]
fn test_explains_failures() {
    let scratch = Scratch::new("explains");
    scratch.copy(LIFECYCLE);
    let yaml = "tests:\n  - name: wrong on purpose\n    steps:\n      - entry: requested\n        data: { evaluatedAt: 1 }\n        expect: { node: closed, value: { status: DONE } }\n";
    let path = scratch.file("return-lifecycle.flow.test.yaml", yaml);
    let tested = flow(&["test", &path]);
    let out = stdout(&tested);
    assert_eq!(tested.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL  wrong on purpose"), "{out}");
    assert!(
        out.contains("node: expected closed, got \"requested\""),
        "{out}"
    );
    assert!(
        out.contains("value.status: expected \"DONE\", got \"INITIATED\""),
        "{out}"
    );
    assert!(out.contains("0 passed, 1 failed"), "{out}");
}

#[test]
fn a_bad_test_file_fails_and_the_rest_still_run() {
    let scratch = Scratch::new("bad-file");
    scratch.copy(LIFECYCLE);
    scratch.file("a.flow.test.yaml", "tests:\n  - name: x\n    stepz: []\n");
    scratch.file(
        "b.flow.test.yaml",
        "flow: return-lifecycle.flow\ntests:\n  - name: starts\n    steps:\n      - entry: requested\n        data: { evaluatedAt: 1 }\n",
    );
    let tested = flow(&["test", scratch.path().to_str().unwrap()]);
    let out = stdout(&tested);
    assert_eq!(tested.status.code(), Some(1), "{out}");
    assert!(out.contains("unknown field `stepz`"), "{out}");
    assert!(out.contains("ok    starts"), "{out}");
    assert!(out.contains("1 passed, 1 failed"), "{out}");
}

#[cfg(unix)]
#[test]
fn test_does_not_follow_links_to_directories() {
    let scratch = Scratch::new("links");
    scratch.copy(LIFECYCLE);
    scratch.copy("examples/flows/return-lifecycle.flow.test.yaml");
    let tally = |output: &Output| {
        stdout(output)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    let alone = flow(&["test", scratch.path().to_str().unwrap()]);
    // a link back to its own directory would otherwise loop
    std::os::unix::fs::symlink(scratch.path(), scratch.path().join("again")).unwrap();
    let linked = flow(&["test", scratch.path().to_str().unwrap()]);
    assert!(linked.status.success(), "{}", stdout(&linked));
    assert_eq!(tally(&linked), tally(&alone));
}

#[test]
fn graph_shows_a_flow_even_when_it_is_broken() {
    let shown = flow(&["graph", LIFECYCLE]);
    assert!(shown.status.success(), "{}", stderr(&shown));
    let graph = printed_json(&shown);
    assert_eq!(graph["entries"], json!(["requested"]));
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 8);
    let ask = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["kind"] == "ask")
        .unwrap();
    assert_eq!(ask["ask"], "warehouse.inspect");
    assert_eq!(graph["lets"][0]["expression"], "from + days * 86400");

    let scratch = Scratch::new("graph");
    let path = scratch.file(
        "broken.flow",
        "edition 2026\nflow f\nentry a\nnode a = b()\n",
    );
    let broken = flow(&["graph", &path]);
    assert_eq!(broken.status.code(), Some(1));
    let (out, errors) = (stdout(&broken), stderr(&broken));
    assert!(out.contains("\"name\": \"a\""), "{out}");
    assert!(errors.contains("unknown node `b`"), "{errors}");
}

#[test]
fn hash_prints_the_flow_hash() {
    let hashed = flow(&["hash", LIFECYCLE]);
    assert_eq!(stdout(&hashed).trim(), load("return-lifecycle").0.hash());
}

#[test]
fn usage() {
    assert_eq!(flow(&[]).status.code(), Some(2));
    assert_eq!(flow(&["frobnicate"]).status.code(), Some(2));
    assert_eq!(flow(&["exec", LIFECYCLE]).status.code(), Some(2));
    assert!(flow(&["--help"]).status.success());
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_a_usage_error() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let output = Command::new(env!("CARGO_BIN_EXE_flow"))
        .args([OsStr::new("check"), OsStr::from_bytes(b"\xff.flow")])
        .output()
        .expect("run flow");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("not valid UTF-8"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn test_files_take_null_answers_and_compare_numbers_exactly() {
    let scratch = Scratch::new("exact");
    let flow_src = "edition 2026\nflow t\nentry go\n\nnode go = ask svc.call() -> got()\nnode got = done(input.result)\nnode done(r) = { \"null\": {{ r == null }}, \"n\": 9007199254740993 }\n";
    scratch.file("exact.flow", flow_src);
    let good = "tests:\n  - name: null answer\n    answers:\n      svc.call: { result: null }\n    steps:\n      - entry: go\n        expect: { node: done, value: { \"null\": true, n: 9007199254740993 } }\n";
    let path = scratch.file("exact.flow.test.yaml", good);
    let tested = flow(&["test", &path]);
    let out = stdout(&tested);
    assert!(tested.status.success(), "{out}\n{}", stderr(&tested));

    // one off in the last digit fails, though both round to the same float
    scratch.file(
        "exact.flow.test.yaml",
        &good.replace("n: 9007199254740993", "n: 9007199254740992"),
    );
    let tested = flow(&["test", &path]);
    let out = stdout(&tested);
    assert_eq!(tested.status.code(), Some(1), "{out}");
    assert!(out.contains("value.n"), "{out}");
}

#[test]
fn a_test_step_with_input_it_would_ignore_fails() {
    let scratch = Scratch::new("ignored-input");
    scratch.copy(LIFECYCLE);
    let path = scratch.file(
        "return-lifecycle.flow.test.yaml",
        "tests:\n  - name: payload alone\n    steps:\n      - entry: requested\n        data: { evaluatedAt: 1 }\n      - payload: { x: 1 }\n  - name: an answer to a start\n    steps:\n      - entry: requested\n        result: { x: 1 }\n  - name: wake as a decimal\n    steps:\n      - entry: requested\n        data: { evaluatedAt: 0 }\n      - event: approve\n        expect: { wakeAt: 604800.0 }\n",
    );
    let out = stdout(&flow(&["test", &path]));
    assert!(out.contains("`payload` goes with an `event`"), "{out}");
    assert!(
        out.contains("a step with `entry` starts an instance"),
        "{out}"
    );
    assert!(out.contains("ok    wake as a decimal"), "{out}");
    assert!(out.contains("1 passed, 2 failed"), "{out}");
}

#[test]
fn only_location_lines_get_the_file_name() {
    let scratch = Scratch::new("arrow");
    let path = scratch.file(
        "arrow.flow",
        "edition 2026\nflow f\nentry a\nnode a = { \"note\": \"a --> b\", \"x\": {{ nope }} }\n",
    );
    let out = stdout(&flow(&["check", &path]));
    assert!(out.contains("\"a --> b\""), "{out}");
    assert!(out.contains(&format!("--> {path}:4:")), "{out}");
}
