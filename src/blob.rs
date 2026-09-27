//! A compiled flow as bytes to store, and back.
//!
//! A blob is the signature `FLOW`, the blob version, the flow's hash, the
//! source, and a checksum of all that.

use crate::common::checksum::{self, CHECKSUM_LEN};
use crate::common::error::LoadError;
use crate::common::limits::MAX_SOURCE_BYTES;
use crate::common::versions::BLOB_VERSION;
use crate::flow::{Flow, HASH_LEN};

const SIGNATURE: [u8; 4] = *b"FLOW";
const VERSION_AT: usize = SIGNATURE.len();
const HASH_AT: usize = VERSION_AT + std::mem::size_of::<u32>();
const SOURCE_AT: usize = HASH_AT + HASH_LEN;

impl Flow {
    /// The flow as bytes to store. They hold the source, which
    /// [`from_bytes`](Self::from_bytes) compiles again.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SOURCE_AT + self.source.len() + CHECKSUM_LEN);
        out.extend_from_slice(&SIGNATURE);
        out.extend_from_slice(&BLOB_VERSION.to_le_bytes());
        out.extend_from_slice(&self.hash);
        out.extend_from_slice(self.source.as_bytes());
        checksum::append(&mut out);
        out
    }

    /// Load a flow saved with [`to_bytes`](Self::to_bytes). Any damaged byte
    /// is refused, and the source must compile to the hash saved with it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Flow, LoadError> {
        if bytes.len() > SOURCE_AT + MAX_SOURCE_BYTES + CHECKSUM_LEN {
            return Err(LoadError::corrupt("larger than any flow blob"));
        }
        if !bytes.starts_with(&SIGNATURE) {
            return Err(LoadError::corrupt("not a flow blob"));
        }
        let body = checksum::strip(bytes).ok_or_else(|| LoadError::corrupt("checksum mismatch"))?;
        let version = body
            .get(VERSION_AT..HASH_AT)
            .and_then(|version| version.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| LoadError::corrupt("truncated"))?;
        if version != BLOB_VERSION {
            return Err(LoadError::IncompatibleVersion {
                found: version,
                expected: BLOB_VERSION,
            });
        }
        let stored_hash = body
            .get(HASH_AT..SOURCE_AT)
            .ok_or_else(|| LoadError::corrupt("truncated"))?;
        let source = std::str::from_utf8(&body[SOURCE_AT..])
            .map_err(|_| LoadError::corrupt("the source is not valid UTF-8"))?;
        let flow = Flow::compile(source).map_err(LoadError::Invalid)?;
        if flow.hash[..] != *stored_hash {
            return Err(LoadError::HashChanged);
        }
        Ok(flow)
    }
}
