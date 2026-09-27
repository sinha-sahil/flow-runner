//! `flow test`: run the `<name>.flow.test.yaml` files beside flows.

use super::{compile_failure, decision_json, input, read, Begin, CliResult, Event, Failure};
use flow_runner::{Decision, Flow, Input, Limits, Token};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `<name>.flow.test.yaml`: tests for one flow.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFile {
    /// The flow, relative to this file. Defaults to `<name>.flow` beside it.
    flow: Option<String>,
    tests: Vec<TestCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestCase {
    name: String,
    #[serde(default)]
    params: serde_json::Map<String, Json>,
    /// Stubbed answers by ask name: one answer given every time, or a list
    /// used in order.
    #[serde(default)]
    answers: BTreeMap<String, Answers>,
    steps: Vec<Step>,
}

// `Many` first: serde would also read a list into `Answer`'s two fields.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answers {
    Many(Vec<Answer>),
    One(Answer),
}

/// A stubbed answer to an ask. `result: null` is an answer of null.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    #[serde(default, deserialize_with = "present")]
    result: Option<Json>,
    #[serde(default, deserialize_with = "present")]
    failure: Option<Json>,
}

/// One run: a step with `entry` starts an instance, one without resumes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    entry: Option<String>,
    /// `input.data`; later steps reuse the last one when this is absent.
    data: Option<Json>,
    event: Option<String>,
    payload: Option<Json>,
    #[serde(default, deserialize_with = "present")]
    result: Option<Json>,
    #[serde(default, deserialize_with = "present")]
    failure: Option<Json>,
    expect: Option<Expect>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    node: Option<String>,
    /// Keys listed must match; keys not listed are ignored.
    value: Option<Json>,
    /// `null` means no wake.
    #[serde(default, deserialize_with = "present")]
    wake_at: Option<Json>,
    ask: Option<String>,
    payload: Option<Json>,
    ignored: Option<bool>,
    /// A piece of the error message.
    error: Option<String>,
}

/// Keeps `key: null` apart from a missing key.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Json>, D::Error> {
    Json::deserialize(deserializer).map(Some)
}

/// The answers left for one ask name.
enum AnswerQueue<'a> {
    /// The same answer every time.
    Repeat(&'a Answer),
    /// Each answer once, in order.
    InOrder(std::slice::Iter<'a, Answer>),
}

impl<'a> AnswerQueue<'a> {
    fn new(answers: &'a Answers) -> AnswerQueue<'a> {
        match answers {
            Answers::One(answer) => AnswerQueue::Repeat(answer),
            Answers::Many(list) => AnswerQueue::InOrder(list.iter()),
        }
    }

    /// The next answer, while any are left.
    fn next(&mut self) -> Option<&'a Answer> {
        match self {
            AnswerQueue::Repeat(answer) => Some(answer),
            AnswerQueue::InOrder(answers) => answers.next(),
        }
    }
}

/// How many tests passed and failed.
#[derive(Default)]
struct Tally {
    passed: usize,
    failed: usize,
}

pub(super) fn command(paths: &[String]) -> CliResult<()> {
    let mut files = Vec::new();
    if paths.is_empty() {
        collect_tests(Path::new("."), &mut files)?;
    }
    for path in paths {
        collect_tests(Path::new(path), &mut files)?;
    }
    if files.is_empty() {
        return Err(Failure::Usage("no *.flow.test.yaml files found".into()));
    }
    files.sort();
    let mut tally = Tally::default();
    for file in &files {
        run_test_file(file, &mut tally);
    }
    println!("\n{} passed, {} failed", tally.passed, tally.failed);
    if tally.failed == 0 {
        Ok(())
    } else {
        Err(Failure::Flow)
    }
}

/// Every test file at or below `path`. Hidden directories, `target` and
/// symbolic links to directories are skipped, so a link loop can't run the
/// same tests again and again.
fn collect_tests(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    let entries =
        std::fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if kind.is_dir() {
            if !file_name.starts_with('.') && file_name != "target" {
                collect_tests(&entry.path(), files)?;
            }
        } else if file_name.ends_with(".flow.test.yaml") || file_name.ends_with(".flow.test.yml") {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Run one file's tests. A file that can't be read, or whose flow doesn't
/// compile, counts as one failed test.
fn run_test_file(path: &Path, tally: &mut Tally) {
    println!("{}", path.display());
    let (file, flow) = match load_test_file(path) {
        Ok(loaded) => loaded,
        Err(why) => {
            tally.failed += 1;
            println!("  FAIL  (the file)");
            print_indented(&why);
            return;
        }
    };
    for case in &file.tests {
        match run_case(&flow, case) {
            Ok(()) => {
                tally.passed += 1;
                println!("  ok    {}", case.name);
            }
            Err(why) => {
                tally.failed += 1;
                println!("  FAIL  {}", case.name);
                print_indented(&why);
            }
        }
    }
}

fn print_indented(text: &str) {
    for line in text.lines() {
        println!("          {line}");
    }
}

fn load_test_file(path: &Path) -> Result<(TestFile, Flow), String> {
    let file: TestFile = serde_yaml_ng::from_str(&read(path)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let flow_path = match &file.flow {
        Some(flow) => path.parent().unwrap_or(Path::new(".")).join(flow),
        None => {
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            let stem = file_name
                .trim_end_matches(".test.yaml")
                .trim_end_matches(".test.yml");
            path.with_file_name(stem)
        }
    };
    let src = read(&flow_path)?;
    let flow = Flow::compile(&src)
        .map_err(|errors| compile_failure(&flow_path.display().to_string(), &src, &errors))?;
    Ok((file, flow))
}

fn run_case(flow: &Flow, case: &TestCase) -> Result<(), String> {
    let mut answers: BTreeMap<&str, AnswerQueue> = case
        .answers
        .iter()
        .map(|(name, answers)| (name.as_str(), AnswerQueue::new(answers)))
        .collect();
    let mut token: Option<Token> = None;
    let mut data = json!({});
    for (index, step) in case.steps.iter().enumerate() {
        let at = format!("step {}", index + 1);
        if let Some(step_data) = &step.data {
            data = step_data.clone();
        }
        if step.payload.is_some() && step.event.is_none() {
            return Err(format!("{at}: `payload` goes with an `event`"));
        }
        let begin = begin(case, step, token.as_ref()).map_err(|why| format!("{at}: {why}"))?;
        let event = step.event.clone().map(|name| Event {
            name,
            payload: step.payload.clone().unwrap_or(Json::Null),
        });
        let decision = run_answering(flow, input(begin, data.clone(), event), &data, &mut answers)
            .map_err(|why| format!("{at}: {why}"))?;
        if let Some(expect) = &step.expect {
            let problems = compare(expect, &decision);
            if !problems.is_empty() {
                return Err(format!("{at}: {}", problems.join("\n")));
            }
        } else if let Decision::Error { error, .. } = &decision {
            return Err(format!("{at}: {error}"));
        }
        if !decision.is_error() {
            if let Some(next) = decision.token() {
                token = Some(next.clone());
            }
        }
    }
    Ok(())
}

/// A start at the step's entry, or a resume of `token`, the last one made.
fn begin(case: &TestCase, step: &Step, token: Option<&Token>) -> Result<Begin, String> {
    if step.entry.is_some() && (step.result.is_some() || step.failure.is_some()) {
        return Err(
            "`result` and `failure` answer an ask; a step with `entry` starts an instance".into(),
        );
    }
    match (&step.entry, token) {
        (Some(entry), _) => Ok(Begin::Start {
            entry: entry.clone(),
            params: case
                .params
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        }),
        (None, Some(token)) => Ok(Begin::Resume {
            token: token.clone(),
            result: step.result.clone(),
            failure: step.failure.clone(),
        }),
        (None, None) => Err("the first step needs `entry: <node>`".into()),
    }
}

/// Run `next`, answering the asks the test stubs, until a decision the test
/// looks at.
fn run_answering(
    flow: &Flow,
    mut next: Input,
    data: &Json,
    answers: &mut BTreeMap<&str, AnswerQueue>,
) -> Result<Decision, String> {
    let limits = Limits::default();
    loop {
        let decision = flow.run(next, &limits).map_err(|error| error.to_string())?;
        let Decision::Ask { name, token, .. } = &decision else {
            return Ok(decision);
        };
        let Some(answer) = answers.get_mut(name.as_str()).and_then(AnswerQueue::next) else {
            return Ok(decision);
        };
        if answer.result.is_some() == answer.failure.is_some() {
            return Err(format!(
                "the answer for `{name}` needs exactly one of `result` or `failure`"
            ));
        }
        let begin = Begin::Resume {
            token: token.clone(),
            result: answer.result.clone(),
            failure: answer.failure.clone(),
        };
        next = input(begin, data.clone(), None);
    }
}

fn compare(expect: &Expect, decision: &Decision) -> Vec<String> {
    let got = decision_json(decision);
    let kind = got["decision"].as_str().unwrap_or_default();
    let wanted_kind = if expect.error.is_some() {
        "error"
    } else if expect.ask.is_some() || expect.payload.is_some() {
        "ask"
    } else if expect.ignored == Some(true) {
        "ignored"
    } else {
        "value"
    };
    if kind != wanted_kind {
        let detail = match decision {
            Decision::Error { error, .. } => format!(": {error}"),
            Decision::Ask { name, .. } => format!(" `{name}`"),
            _ => String::new(),
        };
        return vec![format!("expected {wanted_kind}, got {kind}{detail}")];
    }
    let mut problems = Vec::new();
    if let Some(node) = &expect.node {
        if got["node"] != *node {
            problems.push(format!("node: expected {node}, got {}", got["node"]));
        }
    }
    if let Some(value) = &expect.value {
        subset("value", value, &got["value"], &mut problems);
    }
    if let Some(wake_at) = &expect.wake_at {
        subset("wakeAt", wake_at, &got["wakeAt"], &mut problems);
    }
    if let Some(ask) = &expect.ask {
        if got["ask"] != *ask {
            problems.push(format!("ask: expected {ask}, got {}", got["ask"]));
        }
    }
    if let Some(payload) = &expect.payload {
        subset("payload", payload, &got["payload"], &mut problems);
    }
    if let Some(message) = &expect.error {
        let text = got["error"].as_str().unwrap_or_default();
        if !text.contains(message.as_str()) {
            problems.push(format!(
                "error: expected it to mention {message:?}, got {text:?}"
            ));
        }
    }
    problems
}

/// Numbers are equal when their exact decimal values are, so `1` matches
/// `1.0` and nothing is lost to floating point.
fn same_number(left: &serde_json::Number, right: &serde_json::Number) -> bool {
    let exact = |number: &serde_json::Number| {
        let text = number.to_string();
        Decimal::from_str_exact(&text)
            .or_else(|_| Decimal::from_scientific(&text))
            .ok()
    };
    match (exact(left), exact(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.to_string() == right.to_string(),
    }
}

/// Adds a problem for each place `got` lacks what `wanted` holds. Objects
/// may hold more keys, lists match item by item, and numbers by exact value.
fn subset(path: &str, wanted: &Json, got: &Json, problems: &mut Vec<String>) {
    match (wanted, got) {
        (Json::Object(wanted_map), Json::Object(got_map)) => {
            for (key, wanted_value) in wanted_map {
                match got_map.get(key) {
                    Some(got_value) => {
                        subset(&format!("{path}.{key}"), wanted_value, got_value, problems)
                    }
                    None => problems.push(format!("{path}.{key}: missing")),
                }
            }
        }
        (Json::Array(wanted_items), Json::Array(got_items))
            if wanted_items.len() == got_items.len() =>
        {
            for (index, (wanted_item, got_item)) in wanted_items.iter().zip(got_items).enumerate() {
                subset(&format!("{path}[{index}]"), wanted_item, got_item, problems);
            }
        }
        (Json::Number(wanted_number), Json::Number(got_number))
            if same_number(wanted_number, got_number) => {}
        _ if wanted == got => {}
        _ => problems.push(format!("{path}: expected {wanted}, got {got}")),
    }
}
