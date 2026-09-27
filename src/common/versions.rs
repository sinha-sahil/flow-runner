//! The versions stored bytes carry, and when each one changes. Blobs and
//! tokens outlive the flow-runner that wrote them, so these are promises.

/// Layout of [`Flow::to_bytes`](crate::Flow::to_bytes) blobs. Bump it when
/// the layout changes. A blob of another version is refused with
/// [`LoadError::IncompatibleVersion`](crate::LoadError::IncompatibleVersion):
/// blobs are rebuilt from source, so no old layout is kept.
pub const BLOB_VERSION: u32 = 1;

/// Layout of a token. Bump it when the layout changes, and keep reading
/// every older layout: tokens in flight outlive deployments.
pub(crate) const TOKEN_VERSION: u8 = 1;

/// What a compiled flow means. Bump it when a flow that compiles the same
/// would run differently. It is part of every flow's hash, so a bump
/// changes every hash, and tokens then need
/// [`Flow::rebind`](crate::Flow::rebind).
pub(crate) const SEMANTICS_VERSION: u32 = 1;
