# Changelog

All notable changes to flow-runner are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-09-27

The first version.

### Added

- The `.flow` language: editions, entries, events, params, lets and helpers, nodes with `when`
  branches, and the outcomes jump, value (with `wake`), `ask` and `ignore`. Expressions are
  temple-dsl 1.
- `Flow::compile` and `Flow::check`, with `Diagnostics` that carry a span, a caret and a
  suggestion, and the rules they check: no jump loop without an event, asks continue at nodes
  only their answers enter, events are declared. An unreachable node is a warning.
- `Flow::run` and `Flow::run_traced`: one call from an `Input` to a `Decision`, under `Limits`
  on steps, jumps and asks. Host mistakes are an `InputError`; failures inside a run are a
  `RunError` that hands back the token the run resumed.
- `Token`: the state the host stores between calls, readable with `Token::inspect` as a
  `TokenInfo`. A run that leaves the instance where it was hands back the same token.
  `Flow::rebind` moves a token to a new version of its flow.
- `Flow::to_bytes` and `Flow::from_bytes`, and `Flow::hash`, which changes when what a flow does
  changes and not when its layout, comments or annotations do.
- Reading a flow: `name`, `edition`, `entries`, `events`, `params`, `asks`, `annotations`,
  `warnings`, `nodes` and `node` as `NodeInfo`, and `source`.
- `Document`, `Graph` and `Edit`: editing a flow as text for a visual builder, rewriting only
  the text an edit touches.
- The `flow` command (`cli` feature): `check`, `run`, `test`, `graph` and `hash`, with tests in
  `<name>.flow.test.yaml` beside each flow.
- Builds for `wasm32-wasip1`.
- Examples: programs for one-off decisions, long-running processes, asks, editing and new
  versions, and four complete flows with their tests.
