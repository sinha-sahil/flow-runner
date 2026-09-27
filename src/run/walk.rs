//! One run, from a start or a token to a decision.

use super::token::stored::{self, Refused, Stored, Text};
use super::token::Data;
use super::{Begin, BranchKind, BranchTaken, Decision, Event, Input, Token, Trace};
use crate::common::error::{describe, kind_phrase, twice, InputError, RunError};
use crate::common::limits::{Limits, MAX_TOKEN_STATE};
use crate::common::versions::SEMANTICS_VERSION;
use crate::flow::{Flow, Node, NodeId, Outcome};
use crate::syntax::ast::TICK;
use indexmap::IndexMap;
use smol_str::SmolStr;
use temple_dsl::{Budget, Env, EvalError, ExprUnit, Value};

/// One run in progress.
pub(super) struct Run<'f, 't> {
    flow: &'f Flow,
    limits: &'f Limits,
    budget: Budget,
    /// The `input` object every expression sees.
    input: Value,
    /// Params, flow-level lets, and the parameters and lets of each node
    /// entered so far.
    env: Env<'f>,
    /// The params the instance started with, which every token carries.
    params: Vec<Value>,
    asks: u64,
    resumed: Option<Resumed>,
    /// The node being run, and the arguments it was entered with.
    at: NodeId,
    args: Vec<Value>,
    jumps: u64,
    trace: Option<&'t mut Trace>,
}

/// The token a run resumed, and where it had the instance. A run that
/// leaves the instance there hands this token back as it was.
struct Resumed {
    token: Token,
    node: NodeId,
    args: Vec<Stored>,
    sequence: u64,
    wake_at: Option<i64>,
    /// The instance waited on an ask, which this run answers.
    answering: bool,
}

impl Resumed {
    /// Whether an instance going on as `next`, with `args` stored, stays
    /// where this token has it.
    fn holds(&self, next: &Next, args: &[Stored]) -> bool {
        !self.answering
            && next.ask.is_none()
            && next.node == self.node
            && next.wake_at == self.wake_at
            && args == self.args
    }
}

/// Where a run begins, worked out from what the host sent.
struct Place {
    node: NodeId,
    args: Vec<Value>,
    params: Vec<Value>,
    asks: u64,
    resumed: Option<Resumed>,
}

/// Where an instance goes on, as its next token will say.
struct Next<'v> {
    node: NodeId,
    args: &'v [Value],
    asks: u64,
    wake_at: Option<i64>,
    /// The ask it waits on an answer to.
    ask: Option<&'v SmolStr>,
}

impl<'f, 't> Run<'f, 't> {
    pub(super) fn begin(
        flow: &'f Flow,
        input: Input,
        limits: &'f Limits,
        trace: Option<&'t mut Trace>,
    ) -> Result<Self, InputError> {
        check_input(flow, &input)?;
        let answered = input.answers();
        let Input {
            begin,
            event,
            data,
            params,
            result,
            failure,
        } = input;
        let arrived = matches!(begin, Begin::Entry(_));
        let place = match begin {
            Begin::Entry(entry) => start(flow, entry, &params)?,
            Begin::Token(token) => resume(flow, token, answered, event.is_some())?,
        };
        let mut env = Env::with_functions(&flow.functions);
        for (param, value) in flow.params.iter().zip(&place.params) {
            env.insert(param.name.as_str(), value.clone());
        }
        let input = Arrival {
            data,
            event,
            arrived,
            result,
            failure,
        }
        .into_input();
        Ok(Run {
            flow,
            limits,
            budget: Budget::new(limits.steps),
            input,
            env,
            params: place.params,
            asks: place.asks,
            resumed: place.resumed,
            at: place.node,
            args: place.args,
            jumps: 0,
            trace,
        })
    }

    /// Follow the flow from the starting node until something ends the run.
    pub(super) fn walk(mut self) -> Decision {
        if let Err(error) = self.bind_flow_lets() {
            return self.fail(error);
        }
        loop {
            let flow = self.flow;
            let node = &flow.nodes[self.at.index()];
            if let Err(error) = self.enter(node) {
                return self.fail(error);
            }
            let branch = match self.choose_branch(node) {
                Ok(Some(branch)) => branch,
                Ok(None) => return self.ignore(node),
                Err((branch, error)) => {
                    self.record(node, branch, BranchKind::Error);
                    return self.fail(error);
                }
            };
            match self.follow(node, branch) {
                Ok(Some(decision)) => return decision,
                Ok(None) => {
                    // the first jump uses up the event and any answer, which
                    // flow-level lets may have read
                    if self.jumps == 1 && self.arrive_by_jump() {
                        if let Err(error) = self.bind_flow_lets() {
                            return self.fail(error);
                        }
                    }
                }
                Err(error) => {
                    self.record(node, branch, BranchKind::Error);
                    return self.fail(error);
                }
            }
        }
    }

    /// Flow-level lets see the params and each other, in order.
    fn bind_flow_lets(&mut self) -> Result<(), RunError> {
        for binding in &self.flow.lets {
            let value = binding
                .expression
                .eval(&self.input, &self.env, &self.budget)
                .map_err(|error| RunError::Expression {
                    node: None,
                    error: error.into(),
                })?;
            self.env.insert(binding.name.clone(), value);
        }
        Ok(())
    }

    /// Bind a node's parameters and lets. Names of nodes entered before stay
    /// bound, but checking made sure this node never reads them.
    fn enter(&mut self, node: &Node) -> Result<(), RunError> {
        for (param, value) in node.params.iter().zip(&self.args) {
            self.env.insert(param.clone(), value.clone());
        }
        for binding in &node.lets {
            let value = binding
                .expression
                .eval(&self.input, &self.env, &self.budget)
                .map_err(|error| expression_error(node, error))?;
            self.env.insert(binding.name.clone(), value);
        }
        Ok(())
    }

    /// The first branch whose guard holds; `else` always does. A guard that
    /// fails fails its branch.
    fn choose_branch(&self, node: &Node) -> Result<Option<usize>, (usize, RunError)> {
        for (index, branch) in node.branches.iter().enumerate() {
            let Some(guard) = &branch.guard else {
                return Ok(Some(index));
            };
            match guard.eval(&self.input, &self.env, &self.budget) {
                Ok(Value::Bool(true)) => return Ok(Some(index)),
                Ok(Value::Bool(false)) => {}
                Ok(other) => {
                    let error = RunError::GuardNotBool {
                        node: node.name.to_string(),
                        got: describe(&other),
                        span: guard.span(),
                    };
                    return Err((index, error));
                }
                Err(error) => return Err((index, expression_error(node, error))),
            }
        }
        Ok(None)
    }

    /// Take a branch: `None` when it jumped and the walk goes on. A branch
    /// that fails is recorded by the caller.
    fn follow(&mut self, node: &Node, branch: usize) -> Result<Option<Decision>, RunError> {
        let decision = match &node.branches[branch].outcome {
            Outcome::Ignore => {
                self.record(node, branch, BranchKind::Ignore);
                self.ignore(node)
            }
            Outcome::Jump { node: target, args } => {
                let args = self.evaluate_all(node, args)?;
                if self.jumps >= self.limits.jumps {
                    return Err(RunError::TooManyJumps {
                        limit: self.limits.jumps,
                        node: node.name.to_string(),
                    });
                }
                self.jumps += 1;
                self.record(node, branch, BranchKind::Jump);
                self.at = *target;
                self.args = args;
                return Ok(None);
            }
            Outcome::Value { template, wake } => {
                let value = template
                    .render(&self.input, &self.env, &self.budget)
                    .map_err(|error| expression_error(node, error))?;
                let wake_at = self.wake_time(node, wake.as_ref())?;
                // the instance stays at this node, with the arguments it came with
                let args = std::mem::take(&mut self.args);
                let next = Next {
                    node: self.at,
                    args: &args,
                    asks: self.asks,
                    wake_at,
                    ask: None,
                };
                let token = self.mint(node, next)?;
                self.record(node, branch, BranchKind::Value);
                Decision::Value {
                    value,
                    node: node.name.to_string(),
                    token,
                    wake_at,
                }
            }
            Outcome::Ask {
                name,
                payload,
                node: target,
                args,
            } => {
                let payload = match payload {
                    Some(payload) => payload
                        .eval(&self.input, &self.env, &self.budget)
                        .map_err(|error| expression_error(node, error))?,
                    None => Value::Null,
                };
                let args = self.evaluate_all(node, args)?;
                if self.asks >= self.limits.asks {
                    return Err(RunError::TooManyAsks {
                        limit: self.limits.asks,
                        node: node.name.to_string(),
                    });
                }
                let next = Next {
                    node: *target,
                    args: &args,
                    asks: self.asks + 1,
                    wake_at: None,
                    ask: Some(name),
                };
                let token = self.mint(node, next)?;
                self.record(node, branch, BranchKind::Ask);
                Decision::Ask {
                    name: name.to_string(),
                    payload,
                    node: self.flow.node_name(*target).to_string(),
                    token,
                }
            }
        };
        Ok(Some(decision))
    }

    /// Nothing happens at `node`. That is an error after an answer, which
    /// would be lost, and after a jump, which would drop the event behind it.
    fn ignore(&mut self, node: &Node) -> Decision {
        if self
            .resumed
            .as_ref()
            .is_some_and(|resumed| resumed.answering)
        {
            self.fail(RunError::AnswerIgnored {
                node: node.name.to_string(),
            })
        } else if self.jumps > 0 {
            self.fail(RunError::JumpIgnored {
                node: node.name.to_string(),
            })
        } else {
            Decision::Ignored
        }
    }

    fn wake_time(&self, node: &Node, wake: Option<&ExprUnit>) -> Result<Option<i64>, RunError> {
        let Some(wake) = wake else {
            return Ok(None);
        };
        let value = wake
            .eval(&self.input, &self.env, &self.budget)
            .map_err(|error| expression_error(node, error))?;
        let seconds = match &value {
            Value::Null => return Ok(None),
            Value::Int(seconds) => Some(*seconds),
            // `2.0` is whole too, as a decimal param override can make it
            Value::Decimal(seconds) if seconds.is_integer() => i64::try_from(*seconds).ok(),
            _ => None,
        };
        match seconds {
            Some(seconds) => Ok(Some(seconds)),
            None => Err(RunError::WakeNotTime {
                node: node.name.to_string(),
                got: describe(&value),
                span: wake.span(),
            }),
        }
    }

    fn evaluate_all(&self, node: &Node, units: &[ExprUnit]) -> Result<Vec<Value>, RunError> {
        units
            .iter()
            .map(|unit| unit.eval(&self.input, &self.env, &self.budget))
            .collect::<Result<_, EvalError>>()
            .map_err(|error| expression_error(node, error))
    }

    /// Make `input` what a node reached by a jump sees: no event, no answer,
    /// and `arrived` true. Returns whether that changed anything.
    fn arrive_by_jump(&mut self) -> bool {
        let Value::Obj(fields) = &mut self.input else {
            return false;
        };
        let mut changed = false;
        // one pass over the fields is cheaper than a lookup for each
        for (key, field) in fields.iter_mut() {
            let fresh = match key.as_str() {
                "event" | "payload" | "result" | "failure" => Value::Null,
                "arrived" => Value::Bool(true),
                _ => continue,
            };
            if *field != fresh {
                *field = fresh;
                changed = true;
            }
        }
        changed
    }

    /// The token for the instance going on as `next` says. `from` is the
    /// node that decided it, for errors.
    fn mint(&mut self, from: &Node, next: Next) -> Result<Token, RunError> {
        let mut room = MAX_TOKEN_STATE;
        let refused = |why: Refused| match why {
            Refused::TooDeep => RunError::ValueTooDeep {
                node: from.name.to_string(),
            },
            Refused::TooLarge => RunError::TokenTooLarge {
                node: from.name.to_string(),
                limit: MAX_TOKEN_STATE as u64,
            },
        };
        let args = next
            .args
            .iter()
            .map(|value| stored::encode_within(value, &mut room))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        if let Some(resumed) = self.resumed.take_if(|resumed| resumed.holds(&next, &args)) {
            return Ok(resumed.token);
        }
        let params = self
            .flow
            .params
            .iter()
            .zip(&self.params)
            .map(|(param, value)| stored::encode_entry(SmolStr::new(&param.name), value, &mut room))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        let flow = self.flow;
        Ok(Data {
            semantics: SEMANTICS_VERSION,
            hash: flow.hash.to_vec(),
            flow: Text(flow.name.clone()),
            node: Text(flow.nodes[next.node.index()].name.clone()),
            args,
            params,
            sequence: self
                .resumed
                .as_ref()
                .map_or(1, |resumed| resumed.sequence + 1),
            asks: next.asks,
            wake_at: next.wake_at,
            ask: next.ask.cloned().map(Text),
        }
        .encode())
    }

    fn record(&mut self, node: &Node, branch: usize, kind: BranchKind) {
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.branches.push(BranchTaken {
                node: node.name.to_string(),
                branch,
                kind,
            });
            trace.steps_used = self.budget.used();
        }
    }

    /// End the run with an error, handing back the token it resumed.
    fn fail(&mut self, error: RunError) -> Decision {
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.steps_used = self.budget.used();
        }
        Decision::Error {
            error,
            token: self.resumed.take().map(|resumed| resumed.token),
        }
    }
}

/// Refuse the host's mistakes that show before the entry or token is read.
fn check_input(flow: &Flow, input: &Input) -> Result<(), InputError> {
    match &input.begin {
        Begin::Entry(_) if input.answers() => return Err(InputError::AnswerUnexpected),
        Begin::Entry(_) => {}
        Begin::Token(_) => {
            if let Some(name) = input.params.keys().next() {
                return Err(InputError::ParamOnResume(name.clone()));
            }
            if input.result.is_some() && input.failure.is_some() {
                return Err(InputError::AnswerAmbiguous);
            }
            if matches!(input.failure, Some(Value::Null)) {
                return Err(InputError::NullFailure);
            }
        }
    }
    if let Some(event) = &input.event {
        if event.name != TICK && !flow.events.contains(event.name.as_str()) {
            return Err(InputError::UnknownEvent(event.name.clone()));
        }
    }
    Ok(())
}

/// A run that starts a new instance at `entry`.
fn start(
    flow: &Flow,
    entry: String,
    overrides: &IndexMap<String, Value>,
) -> Result<Place, InputError> {
    let node = flow
        .node_by_name(&entry)
        .filter(|&id| flow.nodes[id.index()].is_entry)
        .ok_or(InputError::UnknownEntry(entry))?;
    Ok(Place {
        node,
        args: Vec::new(),
        params: merge_params(flow, overrides)?,
        asks: 0,
        resumed: None,
    })
}

/// A run that continues the instance `token` belongs to. `answered` says
/// whether the host gave a result or a failure.
fn resume(flow: &Flow, token: Token, answered: bool, has_event: bool) -> Result<Place, InputError> {
    let data = Data::decode(token.as_bytes(), flow)?;
    if data.hash != flow.hash {
        return Err(InputError::OtherVersion);
    }
    // the hash matches, so the checks below fail only for a token made by hand
    let node = flow
        .node_by_name(&data.node)
        .ok_or_else(|| InputError::corrupt("its node isn't in the flow"))?;
    let waiting = data.ask.is_some();
    if waiting != flow.nodes[node.index()].resumes_ask {
        return Err(InputError::corrupt("its state doesn't match its node"));
    }
    if data.sequence == u64::MAX || data.asks == u64::MAX {
        return Err(InputError::corrupt("its counters are used up"));
    }
    if waiting && !answered {
        return Err(InputError::AnswerMissing);
    }
    if answered && !waiting {
        return Err(InputError::AnswerUnexpected);
    }
    if waiting && has_event {
        return Err(InputError::EventDuringAsk);
    }
    if data.args.len() != flow.nodes[node.index()].params.len() {
        return Err(InputError::corrupt("its arguments don't match its node"));
    }
    let args = data
        .args
        .iter()
        .map(stored::decode)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| InputError::corrupt("an argument can't be read"))?;
    let params = carried_params(flow, &data.params)?;
    Ok(Place {
        node,
        args,
        params,
        asks: data.asks,
        resumed: Some(Resumed {
            token,
            node,
            args: data.args,
            sequence: data.sequence,
            wake_at: data.wake_at,
            answering: waiting,
        }),
    })
}

/// The params a token carries, in the flow's order. Its hash matches, so it
/// names each of the flow's params once, though maybe in another order: the
/// hash doesn't depend on theirs.
fn carried_params(flow: &Flow, carried: &[(Text, Stored)]) -> Result<Vec<Value>, InputError> {
    let read = |name: &Text, value: &Stored| {
        stored::decode(value)
            .ok_or_else(|| InputError::corrupt(format!("param `{name}` can't be read")))
    };
    // the flow that made the token wrote them in its order: read them so
    // without looking each name up
    let in_order = carried.len() == flow.params.len()
        && carried
            .iter()
            .zip(&flow.params)
            .all(|((name, _), param)| **name == *param.name);
    if in_order {
        return carried
            .iter()
            .map(|(name, value)| read(name, value))
            .collect();
    }
    let mut params: Vec<Option<Value>> = vec![None; flow.params.len()];
    for (name, value) in carried {
        let position = *flow
            .param_positions
            .get(&**name)
            .ok_or_else(|| InputError::corrupt(format!("param `{name}` isn't in the flow")))?;
        if params[position].is_some() {
            return Err(InputError::corrupt(twice("param", name)));
        }
        params[position] = Some(read(name, value)?);
    }
    params
        .into_iter()
        .collect::<Option<_>>()
        .ok_or_else(|| InputError::corrupt("a param is missing"))
}

/// Param values for a new instance: the host's overrides where given,
/// defaults elsewhere, in declaration order.
fn merge_params(
    flow: &Flow,
    overrides: &IndexMap<String, Value>,
) -> Result<Vec<Value>, InputError> {
    for (name, value) in overrides {
        let Some(&position) = flow.param_positions.get(name.as_str()) else {
            return Err(InputError::UnknownParam(name.clone()));
        };
        let param = &flow.params[position];
        if !same_kind(&param.default, value) {
            return Err(InputError::ParamType {
                name: name.clone(),
                expected: kind_phrase(&param.default).to_string(),
                got: kind_phrase(value).to_string(),
            });
        }
        // every token stores the params, which recurses
        if stored::encode(value).is_none() {
            return Err(InputError::ParamTooDeep(name.clone()));
        }
    }
    Ok(flow
        .params
        .iter()
        .map(|param| {
            overrides
                .get(param.name.as_str())
                .unwrap_or(&param.default)
                .clone()
        })
        .collect())
}

/// Whether an override has the same kind of value as a param's default. A
/// null default takes anything.
fn same_kind(default: &Value, given: &Value) -> bool {
    matches!(
        (default, given),
        (Value::Null, _)
            | (Value::Bool(_), Value::Bool(_))
            | (
                Value::Int(_) | Value::Decimal(_),
                Value::Int(_) | Value::Decimal(_)
            )
            | (Value::Str(_), Value::Str(_))
            | (Value::Arr(_), Value::Arr(_))
            | (Value::Obj(_), Value::Obj(_))
    )
}

/// What came with a run, which expressions read as `input`.
struct Arrival {
    data: Value,
    event: Option<Event>,
    /// A new run arrives at its entry; a resumed one was already parked.
    arrived: bool,
    result: Option<Value>,
    failure: Option<Value>,
}

impl Arrival {
    /// The `input` object: `data` first, then the rest.
    fn into_input(self) -> Value {
        let (event_name, payload) = match self.event {
            Some(event) => (Value::Str(event.name.into()), event.payload),
            None => (Value::Null, Value::Null),
        };
        let fields = [
            ("data", self.data),
            ("event", event_name),
            ("payload", payload),
            ("arrived", Value::Bool(self.arrived)),
            ("result", self.result.unwrap_or(Value::Null)),
            ("failure", self.failure.unwrap_or(Value::Null)),
        ];
        Value::Obj(
            fields
                .into_iter()
                .map(|(name, value)| (SmolStr::new_static(name), value))
                .collect(),
        )
    }
}

fn expression_error(node: &Node, error: EvalError) -> RunError {
    RunError::Expression {
        node: Some(node.name.to_string()),
        error: error.into(),
    }
}
