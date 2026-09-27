//! What every stage shares: diagnostics, errors, limits, and the versions
//! and checksum of stored bytes.

pub(crate) mod checksum;
pub(crate) mod diagnostic;
pub(crate) mod error;
pub(crate) mod limits;
pub(crate) mod versions;
