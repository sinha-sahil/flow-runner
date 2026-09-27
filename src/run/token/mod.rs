//! [`Token`]: where an instance is, as bytes the host stores and passes back
//! to resume.
//!
//! A token is the signature `FTOK`, the token version, its fields as a CBOR
//! array, and a checksum of all that.

mod base64;
pub(super) mod stored;

use self::stored::{Stored, Text};
use crate::common::checksum;
use crate::common::error::{twice, InputError, RebindError};
use crate::common::limits::{MAX_TOKEN_BYTES, MAX_TOKEN_NESTING, MAX_TOKEN_STATE};
use crate::common::versions::{SEMANTICS_VERSION, TOKEN_VERSION};
use crate::flow::{hex, Flow};
use serde::de::{DeserializeOwned, IgnoredAny};
use smol_str::SmolStr;
use std::collections::HashMap;
use std::fmt;

/// Where an instance is: its node and the arguments it came with, its
/// params and counters, and what it waits for. Store it with the instance,
/// and pass it back to resume.
///
/// A token holds state, not identity: instances in the same state have equal
/// tokens, so key instances by an id of your own. And it isn't signed: its
/// checksum catches damage, not tampering. Keep tokens where only the host
/// can change them, like any other state it trusts.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(Vec<u8>);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token({})", self.to_base64())
    }
}

impl Token {
    /// A token from the bytes [`as_bytes`](Self::as_bytes) gave. It is checked
    /// when used, not here.
    pub fn from_bytes(bytes: Vec<u8>) -> Token {
        Token(bytes)
    }

    /// The bytes to store.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The bytes to store, without copying them.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// URL-safe base64 without padding, for storing as text.
    pub fn to_base64(&self) -> String {
        base64::encode(&self.0)
    }

    /// A token from the text [`to_base64`](Self::to_base64) gave. Padding
    /// and the standard alphabet's `+` and `/` are read too.
    pub fn from_base64(text: &str) -> Result<Token, InputError> {
        base64::decode(text)
            .map(Token)
            .ok_or_else(|| InputError::corrupt("not valid base64"))
    }

    /// What the token says, read without a flow.
    pub fn inspect(&self) -> Result<TokenInfo, InputError> {
        let (_, hash, flow, node, _, _, sequence, asks, wake_at, ask): Fields<
            IgnoredAny,
            IgnoredAny,
        > = fields(&self.0)?;
        Ok(TokenInfo {
            flow: flow.to_string(),
            hash: hex(&hash),
            node: node.to_string(),
            sequence,
            asks,
            ask: ask.map(|ask| ask.to_string()),
            wake_at,
        })
    }
}

/// What a [`Token`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    /// The flow's name.
    pub flow: String,
    /// The version of the flow that made it, as [`Flow::hash`] shows it.
    pub hash: String,
    /// The node the instance is at.
    pub node: String,
    /// Goes up by one each time a run moves the instance: to another node or
    /// other arguments, to another wake time, or into an ask. A run that
    /// leaves the instance where it was hands back the same token, so use
    /// this as a compare-and-swap version when saving.
    pub sequence: u64,
    /// Asks the instance has made so far.
    pub asks: u64,
    /// The ask the instance waits on an answer to.
    pub ask: Option<String>,
    /// When the instance wants a `tick`, in epoch seconds.
    pub wake_at: Option<i64>,
}

const SIGNATURE: [u8; 4] = *b"FTOK";
const VERSION_AT: usize = SIGNATURE.len();
const FIELDS_AT: usize = VERSION_AT + 1;

/// Room a new token starts with: typical tokens are about a hundred bytes.
const TYPICAL_TOKEN_BYTES: usize = 128;

/// What a token holds, in the order it stores it.
#[derive(Debug, Clone)]
pub(super) struct Data {
    /// The [`SEMANTICS_VERSION`] it was made under.
    pub semantics: u32,
    /// The hash of the flow version that made it.
    pub hash: Vec<u8>,
    pub flow: Text,
    pub node: Text,
    pub args: Vec<Stored>,
    pub params: Vec<(Text, Stored)>,
    pub sequence: u64,
    pub asks: u64,
    pub wake_at: Option<i64>,
    /// The ask the instance waits on an answer to.
    pub ask: Option<Text>,
}

/// A token's fields on the wire: an array, not a map, so no field names,
/// which halves a token's size. `inspect` skips the arguments and params.
type Fields<Args, Params> = (
    u32,
    serde_bytes::ByteBuf,
    Text,
    Text,
    Args,
    Params,
    u64,
    u64,
    Option<i64>,
    Option<Text>,
);

impl Data {
    pub(super) fn encode(&self) -> Token {
        let fields = (
            self.semantics,
            serde_bytes::Bytes::new(&self.hash),
            &self.flow,
            &self.node,
            &self.args,
            &self.params,
            self.sequence,
            self.asks,
            self.wake_at,
            &self.ask,
        );
        let mut out = Vec::with_capacity(TYPICAL_TOKEN_BYTES);
        out.extend_from_slice(&SIGNATURE);
        out.push(TOKEN_VERSION);
        ciborium::ser::into_writer(&fields, &mut out).expect("writing to a Vec can't fail");
        checksum::append(&mut out);
        Token(out)
    }

    /// The fields of a token for `flow`, made by this flow-runner or an
    /// older one.
    pub(super) fn decode(bytes: &[u8], flow: &Flow) -> Result<Data, InputError> {
        let (semantics, hash, name, node, args, params, sequence, asks, wake_at, ask) =
            fields(bytes)?;
        if *name != *flow.name {
            return Err(InputError::OtherFlow(name.to_string()));
        }
        if semantics > SEMANTICS_VERSION {
            return Err(InputError::NewerToken);
        }
        Ok(Data {
            semantics,
            hash: hash.into_vec(),
            flow: name,
            node,
            args,
            params,
            sequence,
            asks,
            wake_at,
            ask,
        })
    }
}

/// A token's fields, once its signature, checksum and version check out.
fn fields<Args, Params>(bytes: &[u8]) -> Result<Fields<Args, Params>, InputError>
where
    Args: DeserializeOwned,
    Params: DeserializeOwned,
{
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(InputError::corrupt("longer than any token"));
    }
    if !bytes.starts_with(&SIGNATURE) {
        return Err(InputError::corrupt("not a flow token"));
    }
    let body = checksum::strip(bytes).ok_or_else(|| InputError::corrupt("checksum mismatch"))?;
    match body.get(VERSION_AT) {
        Some(&TOKEN_VERSION) => {}
        Some(&version) if version > TOKEN_VERSION => return Err(InputError::NewerToken),
        _ => return Err(InputError::corrupt("no known token version")),
    }
    let mut rest = &body[FIELDS_AT..];
    let fields = ciborium::de::from_reader_with_recursion_limit(&mut rest, MAX_TOKEN_NESTING)
        .map_err(|error| InputError::corrupt(format!("malformed: {error}")))?;
    if !rest.is_empty() {
        return Err(InputError::corrupt("bytes after its end"));
    }
    Ok(fields)
}

impl Flow {
    /// Move a token to this version of the flow: its node must still exist,
    /// take as many arguments, and continue an ask exactly when the token
    /// waits on one, and each param it carries must still exist. The
    /// instance stays where it was, with the same sequence number.
    ///
    /// Arguments travel by position, so a node whose parameters were renamed
    /// or reordered takes them as they were. The host decides when rebinding
    /// is safe, e.g. after fixing a flow that failed mid-instance.
    pub fn rebind(&self, token: &Token) -> Result<Token, RebindError> {
        let data = Data::decode(&token.0, self).map_err(RebindError::Token)?;
        let Some(id) = self.node_by_name(&data.node) else {
            return Err(RebindError::NodeGone(data.node.to_string()));
        };
        let node = &self.nodes[id.index()];
        // a waiting token needs a node that takes the answer, and a parked
        // token can't land on one, which only an answer may enter
        if node.params.len() != data.args.len() || node.resumes_ask != data.ask.is_some() {
            return Err(RebindError::NodeChanged(data.node.to_string()));
        }
        let mut carried: HashMap<SmolStr, Stored> = HashMap::with_capacity(data.params.len());
        for (Text(name), value) in data.params {
            if carried.contains_key(&name) {
                let error = InputError::corrupt(twice("param", &name));
                return Err(RebindError::Token(error));
            }
            if !self.param_positions.contains_key(&name) {
                return Err(RebindError::ParamGone(name.to_string()));
            }
            carried.insert(name, value);
        }
        let params: Vec<(Text, Stored)> = self
            .params
            .iter()
            .map(|param| {
                // a param added since the token was made starts at its
                // default, a literal, which is never too deep to store
                let value = carried
                    .remove(param.name.as_str())
                    .unwrap_or_else(|| stored::encode(&param.default).unwrap_or(Stored::Null));
                (Text(SmolStr::new(&param.name)), value)
            })
            .collect();
        if stored::state_len(&data.args, &params) > MAX_TOKEN_STATE {
            return Err(RebindError::TooLarge);
        }
        Ok(Data {
            semantics: SEMANTICS_VERSION,
            hash: self.hash.to_vec(),
            params,
            ..data
        }
        .encode())
    }
}
