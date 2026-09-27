//! Every limit: the fixed ones, and [`Limits`], which a host sets per run.

/// Largest `.flow` source accepted, in bytes.
pub const MAX_SOURCE_BYTES: usize = 4 << 20;

/// Longest name, in bytes: of a flow, node, param, let, event, ask or
/// annotation. Names travel in tokens, so this bounds a token's size.
pub(crate) const MAX_NAME_BYTES: usize = 256;

/// Deepest value a token stores or a param takes. Storing, copying and
/// comparing a value recurse once per level.
pub(crate) const MAX_VALUE_DEPTH: usize = 128;

/// Deepest value an edit writes as a literal: temple parses no deeper.
pub(crate) const MAX_LITERAL_DEPTH: usize = 63;

/// Longest expression or value, in bytes. The flow hash formats each with
/// temple, which formats templates of up to 1 MiB, the unit included.
pub(crate) const MAX_UNIT_BYTES: usize = 512 << 10;

/// Most bytes of node arguments and params, with their names, that a token
/// carries. Typical tokens hold about a hundred bytes; instance data belongs
/// with the host.
pub(crate) const MAX_TOKEN_STATE: usize = 1 << 20;

/// Longest token read at all: the state, the flow, node and ask names, and
/// room for the fixed fields.
pub(crate) const MAX_TOKEN_BYTES: usize = MAX_TOKEN_STATE + 3 * MAX_NAME_BYTES + 128;

/// Deepest nesting a token's CBOR may have. Each level of a stored object
/// takes three (its tag, its entries, and the entry), and the token's own
/// fields a few more.
pub(crate) const MAX_TOKEN_NESTING: usize = 3 * MAX_VALUE_DEPTH + 16;

/// Longest decimal a token stores, as text: a sign, 29 digits and a point.
/// Longer text is refused unread, since parsing it recurses per digit.
pub(crate) const MAX_DECIMAL_TEXT: usize = 31;

/// Did-you-mean searches per compile, and the edit-distance table cells they
/// may fill between them. A search runs only if the most it could fill still
/// fits, so a file full of unknown names, or of long and alike ones, gets
/// hints on its first ones only.
pub(crate) const MAX_HINTS: usize = 100;
pub(crate) const MAX_HINT_CELLS: u64 = 10_000_000;

/// Node names a diagnostic lists before it says "and N more".
pub(crate) const MAX_LISTED_NAMES: usize = 10;

/// Lines longer than this, in bytes, are shown in a report as a window
/// around the span, so reports stay small however the file is laid out.
pub(crate) const MAX_REPORT_LINE: usize = 160;

/// How much of a long line a report shows before and after the span's start.
pub(crate) const REPORT_BEFORE: usize = 60;
pub(crate) const REPORT_AFTER: usize = 100;

/// Work limits for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Evaluation steps, shared by every expression in the run. A step is one
    /// expression, one element a builtin touches, builds or copies, or 64
    /// bytes of text copied or built, so steps bound memory as well as time:
    /// the default is about 30 ms of work and at most a few hundred
    /// megabytes, whatever the flow does.
    pub steps: u64,
    /// Jumps from node to node within one run.
    pub jumps: u64,
    /// Asks over the whole life of an instance.
    pub asks: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            steps: 2_000_000,
            jumps: 1_000,
            asks: 100_000,
        }
    }
}
