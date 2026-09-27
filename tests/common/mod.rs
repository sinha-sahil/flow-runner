//! Helpers the test files share. Each file uses some of them.
#![allow(dead_code, unused_imports)]

use flow_runner::{Decision, Flow, Input, Limits};

/// The example flows, which are also the tests' fixtures.
pub const EXAMPLES: [&str; 4] = [
    "return-eligibility",
    "return-lifecycle",
    "spend-approval",
    "carrier-quote",
];

/// When the tests' runs happen, in epoch seconds.
pub const NOW: i64 = 1_774_166_400;
pub const DAY: i64 = 86_400;

// Limits from `src/common/limits.rs`, which the crate keeps private.
pub const MAX_NAME_BYTES: usize = 256;
pub const MAX_LITERAL_DEPTH: usize = 63;
pub const MAX_TOKEN_STATE: usize = 1 << 20;

/// An example flow, compiled, and its source.
pub fn load(name: &str) -> (Flow, String) {
    let path = format!("{}/examples/flows/{name}.flow", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    (compile(&src), src)
}

/// `src` compiled; a report of what's wrong fails the test.
#[track_caller]
pub fn compile(src: &str) -> Flow {
    match Flow::compile(src) {
        Ok(flow) => flow,
        Err(errors) => panic!("{}", errors.report(src)),
    }
}

pub fn run(flow: &Flow, input: Input) -> Decision {
    flow.run(input, &Limits::default()).unwrap()
}

/// Run `f` on a 2 MB thread, or on WebAssembly, which has no threads, on its 1 MB main stack.
pub fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    if cfg!(target_family = "wasm") {
        return f();
    }
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[cfg(feature = "json")]
pub use self::json::*;

/// Helpers that pass values as JSON, which needs the `json` feature.
#[cfg(feature = "json")]
mod json {
    use super::{run, NOW};
    use flow_runner::{Decision, Flow, Input, Token, Value};
    use serde_json::json;

    pub fn from_json(json: serde_json::Value) -> Value {
        Value::from(json)
    }

    pub fn to_json(value: &Value) -> serde_json::Value {
        serde_json::Value::from(value.clone())
    }

    /// The data every run of the lifecycle flow gets.
    pub fn lifecycle_data() -> Value {
        from_json(json!({ "evaluatedAt": NOW, "returnId": "R-1" }))
    }

    /// The data every run of the carrier flow gets.
    pub fn quote_data() -> Value {
        from_json(json!({ "pincode": "560001", "weightGrams": 800 }))
    }

    /// What a value decision holds, with the value as JSON.
    pub struct Parked {
        pub value: serde_json::Value,
        pub node: String,
        pub token: Token,
        pub wake_at: Option<i64>,
    }

    #[track_caller]
    pub fn parked(decision: Decision) -> Parked {
        match decision {
            Decision::Value {
                value,
                node,
                token,
                wake_at,
            } => Parked {
                value: to_json(&value),
                node,
                token,
                wake_at,
            },
            other => panic!("expected a value, got {other:?}"),
        }
    }

    /// What an ask decision holds, with the payload as JSON.
    pub struct Asked {
        pub name: String,
        pub payload: serde_json::Value,
        pub node: String,
        pub token: Token,
    }

    #[track_caller]
    pub fn asked(decision: Decision) -> Asked {
        match decision {
            Decision::Ask {
                name,
                payload,
                node,
                token,
            } => Asked {
                name,
                payload: to_json(&payload),
                node,
                token,
            },
            other => panic!("expected an ask, got {other:?}"),
        }
    }

    /// Answer each ask with the next of `answers`, `Ok` a result and `Err` a failure, until the
    /// run ends in something else.
    pub fn drive(
        flow: &Flow,
        mut input: Input,
        answers: Vec<Result<serde_json::Value, serde_json::Value>>,
        data: &Value,
    ) -> (Decision, Vec<(String, serde_json::Value)>) {
        let mut answers = answers.into_iter();
        let mut asked = Vec::new();
        loop {
            match run(flow, input) {
                Decision::Ask {
                    name,
                    payload,
                    token,
                    ..
                } => {
                    asked.push((name, to_json(&payload)));
                    let resume = Input::resume(token, data.clone());
                    input = match answers.next() {
                        Some(Ok(result)) => resume.result(from_json(result)),
                        Some(Err(failure)) => resume.failure(from_json(failure)),
                        None => panic!("ran out of answers after {asked:?}"),
                    };
                }
                other => return (other, asked),
            }
        }
    }
}

/// Tokens taken apart and sealed again with a valid checksum, as a token made by hand would be.
pub mod forge {
    use ciborium::value::Value as Cbor;
    use flow_runner::Token;
    use sha2::{Digest, Sha256};

    // Versions from `src/common/versions.rs`, which the crate keeps private.
    pub const TOKEN_VERSION: u8 = 1;
    pub const SEMANTICS_VERSION: u32 = 1;

    /// Where the fields start: after the signature and the version.
    const FIELDS_AT: usize = 5;
    const CHECKSUM_LEN: usize = 8;

    /// The token's fields as CBOR bytes, between the version and the checksum.
    pub fn body(token: &Token) -> Vec<u8> {
        let bytes = token.as_bytes();
        bytes[FIELDS_AT..bytes.len() - CHECKSUM_LEN].to_vec()
    }

    /// The token's fields, in order.
    pub fn fields(token: &Token) -> Vec<Cbor> {
        match ciborium::de::from_reader(&body(token)[..]).unwrap() {
            Cbor::Array(fields) => fields,
            other => panic!("expected the fields as an array, got {other:?}"),
        }
    }

    /// A token holding `fields`, with a valid checksum.
    pub fn seal(fields: Vec<Cbor>) -> Token {
        let mut body = Vec::new();
        ciborium::ser::into_writer(&Cbor::Array(fields), &mut body).unwrap();
        seal_raw(TOKEN_VERSION, &body)
    }

    /// A token of `version` holding `body`, with a valid checksum.
    pub fn seal_raw(version: u8, body: &[u8]) -> Token {
        let mut out = b"FTOK".to_vec();
        out.push(version);
        out.extend_from_slice(body);
        let sum = Sha256::digest(&out);
        out.extend_from_slice(&sum[..CHECKSUM_LEN]);
        Token::from_bytes(out)
    }

    /// `token` with field `index` replaced.
    pub fn with(token: &Token, index: usize, value: Cbor) -> Token {
        let mut fields = fields(token);
        fields[index] = value;
        seal(fields)
    }

    // The field numbers, in order.
    pub const SEMANTICS: usize = 0;
    pub const HASH: usize = 1;
    pub const FLOW: usize = 2;
    pub const NODE: usize = 3;
    pub const ARGS: usize = 4;
    pub const PARAMS: usize = 5;
    pub const SEQUENCE: usize = 6;
    pub const ASKS: usize = 7;
    pub const WAKE_AT: usize = 8;
    pub const ASK: usize = 9;
}
