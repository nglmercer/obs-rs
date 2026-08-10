/// A stable runtime handle for an owned source instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(pub(crate) u64);

impl SourceId {
    /// Returns the numeric value for logs and deterministic fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}
