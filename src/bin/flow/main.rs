//! The `flow` command: check, run and test `.flow` files, and print their
//! graph or hash.

mod run;
mod test;

use flow_runner::{Decision, Diagnostics, Document, Flow, Input, Token, Value};
use serde_json::{json, Value as Json};
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  flow check <file.flow>...        print errors and warnings
  flow run <file.flow> [options]   run once and print the decision as JSON
  flow test [<path>...]            run *.flow.test.yaml files (default: all below .)
  flow graph <file.flow>           print the flow as a graph, as JSON
  flow hash <file.flow>            print the flow's version hash

run options:
  --entry <node>          entry to start at (default: the only entry)
  --data <json|@file>     input.data (default: {})
  --param <name>=<json>   set a param (repeatable)
  --event <name>          deliver an event
  --payload <json|@file>  the event's payload
  --token <token>         resume this token instead of starting
  --result <json|@file>   answer the ask the token waits on
  --failure <json|@file>  or say why it failed
  --trace                 include the path the run took

exit status: 0 when all is well, 1 when a flow fails (it doesn't compile, a run
ends in an error, a test fails), 2 when the command can't do what it was asked";

/// Exit status when a flow fails.
const EXIT_FLOW_FAILED: u8 = 1;

/// Exit status when the command can't do what it was asked.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(OsString::into_string)
        .collect()
    {
        Ok(args) => args,
        Err(arg) => {
            eprintln!("error: {} is not valid UTF-8", arg.to_string_lossy());
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("check") if args.len() > 1 => check(&args[1..]),
        Some("run") if args.len() > 1 => run::command(&args[1], &args[2..]),
        Some("test") => test::command(&args[1..]),
        Some("graph") if args.len() == 2 => graph(&args[1]),
        Some("hash") if args.len() == 2 => hash(&args[1]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Flow) => ExitCode::from(EXIT_FLOW_FAILED),
        Err(Failure::Usage(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// Why a command failed, which sets the exit status.
enum Failure {
    /// A flow failed: it doesn't compile, a run ended in an error, or a test
    /// failed. The command has printed why.
    Flow,
    /// The command can't do what it was asked: a bad option, a file it
    /// can't read, input that isn't JSON or that the flow refuses.
    Usage(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Usage(message)
    }
}

type CliResult<T> = Result<T, Failure>;

fn read(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Compile a file, printing its diagnostics when it doesn't compile.
fn load(path: &str) -> CliResult<Flow> {
    let src = read(path)?;
    Flow::compile(&src).map_err(|errors| {
        eprintln!("{}", compile_failure(path, &src, &errors));
        Failure::Flow
    })
}

/// Why the flow at `path` doesn't compile.
fn compile_failure(path: &str, src: &str, errors: &Diagnostics) -> String {
    format!(
        "{path} doesn't compile\n\n{}",
        located(path, &errors.report(src))
    )
}

/// Put the file name into each `-->` location line of a report.
fn located(path: &str, report: &str) -> String {
    report
        .split('\n')
        .map(|line| {
            let text = line.trim_start();
            match text.strip_prefix("--> ") {
                Some(location) => {
                    let indent = &line[..line.len() - text.len()];
                    format!("{indent}--> {path}:{location}")
                }
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn check(paths: &[String]) -> CliResult<()> {
    let mut clean = true;
    for path in paths {
        let src = read(path)?;
        let diagnostics = Flow::check(&src);
        if diagnostics.is_empty() {
            println!("{path}: ok");
            continue;
        }
        println!("{}\n", located(path, &diagnostics.report(&src)));
        let errors = diagnostics.errors().count();
        let warnings = diagnostics.len() - errors;
        println!(
            "{path}: {errors} error{}, {warnings} warning{}",
            plural(errors),
            plural(warnings)
        );
        clean &= errors == 0;
    }
    if clean {
        Ok(())
    } else {
        Err(Failure::Flow)
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn graph(path: &str) -> CliResult<()> {
    let document = Document::parse(read(path)?);
    println!("{:#}", document.graph().to_json());
    let diagnostics = document.diagnostics();
    if !diagnostics.is_empty() {
        eprintln!("{}", located(path, &diagnostics.report(document.text())));
    }
    if diagnostics.has_errors() {
        Err(Failure::Flow)
    } else {
        Ok(())
    }
}

fn hash(path: &str) -> CliResult<()> {
    let flow = load(path)?;
    println!("{}", flow.hash());
    Ok(())
}

/// How a run begins, as a command gives it.
enum Begin {
    Start {
        entry: String,
        params: Vec<(String, Json)>,
    },
    Resume {
        token: Token,
        result: Option<Json>,
        failure: Option<Json>,
    },
}

/// An event to deliver, and what comes with it.
struct Event {
    name: String,
    payload: Json,
}

/// The input for one run.
fn input(begin: Begin, data: Json, event: Option<Event>) -> Input {
    let data = Value::from(data);
    let mut input = match begin {
        Begin::Start { entry, params } => params
            .into_iter()
            .fold(Input::start(entry, data), |input, (name, value)| {
                input.param(name, Value::from(value))
            }),
        Begin::Resume {
            token,
            result,
            failure,
        } => {
            let mut input = Input::resume(token, data);
            if let Some(result) = result {
                input = input.result(Value::from(result));
            }
            if let Some(failure) = failure {
                input = input.failure(Value::from(failure));
            }
            input
        }
    };
    if let Some(Event { name, payload }) = event {
        input = input.event(name, Value::from(payload));
    }
    input
}

/// A decision as JSON, as `flow run` prints it and tests compare it.
fn decision_json(decision: &Decision) -> Json {
    match decision {
        Decision::Value {
            value,
            node,
            token,
            wake_at,
        } => json!({
            "decision": "value", "node": node, "value": Json::from(value.clone()),
            "wakeAt": wake_at, "token": token.to_base64(),
        }),
        Decision::Ask {
            name,
            payload,
            node,
            token,
        } => json!({
            "decision": "ask", "ask": name, "payload": Json::from(payload.clone()),
            "node": node, "token": token.to_base64(),
        }),
        Decision::Ignored => json!({ "decision": "ignored" }),
        Decision::Error { error, token } => json!({
            "decision": "error", "error": error.to_string(),
            "token": token.as_ref().map(Token::to_base64),
        }),
        _ => json!({ "decision": "unknown" }),
    }
}
