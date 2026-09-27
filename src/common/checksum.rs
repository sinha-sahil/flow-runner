//! The checksum that ends every blob and token: the first bytes of a
//! SHA-256 of everything before it. It catches damage, not tampering.
//! Callers check it before reading the version, so a damaged version byte
//! reads as damage, not as another version.

use sha2::{Digest, Sha256};

/// Bytes of checksum at the end.
pub(crate) const CHECKSUM_LEN: usize = 8;

/// Append the checksum of `bytes`.
pub(crate) fn append(bytes: &mut Vec<u8>) {
    let sum = Sha256::digest(&bytes[..]);
    bytes.extend_from_slice(&sum[..CHECKSUM_LEN]);
}

/// `bytes` without their checksum, if it matches.
pub(crate) fn strip(bytes: &[u8]) -> Option<&[u8]> {
    let body_len = bytes.len().checked_sub(CHECKSUM_LEN)?;
    let (body, sum) = bytes.split_at(body_len);
    (Sha256::digest(body)[..CHECKSUM_LEN] == *sum).then_some(body)
}
