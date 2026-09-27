//! `flow run`: run a flow once and print the decision as JSON.

use super::{decision_json, input, load, located, read, Begin, CliResult, Event, Failure, USAGE};
use flow_runner::{Decision, Flow, Limits, Token};
use serde_json::{json, Value as Json};
use std::borrow::Cow;

/// The options `flow run` takes, as given on the command line.
#[derive(Default)]
struct Options<'a> {
    entry: Option<&'a str>,
    data: Option<&'a str>,
    params: Vec<(String, Json)>,
    event: Option<&'a str>,
    payload: Option<&'a str>,
    token: Option<&'a str>,
    result: Option<&'a str>,
    failure: Option<&'a str>,
    trace: bool,
}

impl<'a> Options<'a> {
    fn parse(args: &'a [String]) -> Result<Options<'a>, String> {
        let mut options = Options::default();
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let flag = flag.as_str();
            let mut value = || {
                args.next()
                    .map(String::as_str)
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag {
                "--trace" => options.trace = true,
                "--entry" => options.entry = Some(value()?),
                "--data" => options.data = Some(value()?),
                "--param" => options.params.push(param(value()?)?),
                "--event" => options.event = Some(value()?),
                "--payload" => options.payload = Some(value()?),
                "--token" => options.token = Some(value()?),
                "--result" => options.result = Some(value()?),
                "--failure" => options.failure = Some(value()?),
                other => return Err(format!("unknown option {other}\n\n{USAGE}")),
            }
        }
        if options.payload.is_some() && options.event.is_none() {
            return Err("--payload goes with an --event".into());
        }
        Ok(options)
    }
}

/// `<name>=<json>`; text that isn't JSON is a string, so
/// `--param carrier=bluedart` works.
fn param(assignment: &str) -> Result<(String, Json), String> {
    let (name, value) = assignment
        .split_once('=')
        .ok_or("--param needs <name>=<json>")?;
    let value = serde_json::from_str(value).unwrap_or_else(|_| Json::from(value));
    Ok((name.to_string(), value))
}

/// A JSON option's value, inline or `@path`; `None` when it wasn't given.
fn json_option(flag: &str, text: Option<&str>) -> Result<Option<Json>, String> {
    let Some(text) = text else {
        return Ok(None);
    };
    let text: Cow<str> = match text.strip_prefix('@') {
        Some(path) => read(path)?.into(),
        None => text.into(),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{flag}: not valid JSON: {error}"))
}

pub(super) fn command(path: &str, args: &[String]) -> CliResult<()> {
    let options = Options::parse(args)?;
    let flow = load(path)?;
    let begin = begin(&flow, &options)?;
    let data = json_option("--data", options.data)?.unwrap_or_else(|| json!({}));
    let event = match options.event {
        Some(name) => Some(Event {
            name: name.to_string(),
            payload: json_option("--payload", options.payload)?.unwrap_or(Json::Null),
        }),
        None => None,
    };
    let (decision, trace) = flow
        .run_traced(input(begin, data, event), &Limits::default())
        .map_err(|error| error.to_string())?;
    let mut out = decision_json(&decision);
    if options.trace {
        out["trace"] = trace
            .branches
            .iter()
            .map(|taken| json!({ "node": taken.node, "branch": taken.branch, "kind": taken.kind.as_str() }))
            .collect();
        out["steps"] = json!(trace.steps_used);
    }
    println!("{out:#}");
    if let Decision::Error { error, .. } = &decision {
        eprintln!("{}", located(path, &error.report(flow.source())));
        return Err(Failure::Flow);
    }
    Ok(())
}

/// A start at an entry, or a resume of `--token`.
fn begin(flow: &Flow, options: &Options) -> Result<Begin, String> {
    let Some(token) = options.token else {
        if options.result.is_some() || options.failure.is_some() {
            return Err("--result and --failure answer an ask; pass the --token that asked".into());
        }
        return Ok(Begin::Start {
            entry: entry(flow, options.entry)?,
            params: options.params.clone(),
        });
    };
    if options.entry.is_some() || !options.params.is_empty() {
        return Err("--entry and --param only apply when starting, not with --token".into());
    }
    Ok(Begin::Resume {
        token: Token::from_base64(token.trim()).map_err(|error| error.to_string())?,
        result: json_option("--result", options.result)?,
        failure: json_option("--failure", options.failure)?,
    })
}

/// `--entry`, or the flow's only entry.
fn entry(flow: &Flow, given: Option<&str>) -> Result<String, String> {
    if let Some(entry) = given {
        return Ok(entry.to_string());
    }
    let entries: Vec<&str> = flow.entries().collect();
    match entries.as_slice() {
        [only] => Ok(only.to_string()),
        many => Err(format!("pick an entry with --entry: {}", many.join(", "))),
    }
}
