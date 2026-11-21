//! The physical supervised-unit identity.
//!
//! The identity exists so a nested unit's containment anchor can be
//! offered to, acknowledged by, and released from the top-level parent by
//! exact typed correlation rather than by ordering or by an approximate
//! pgid match. It is allocated by [`crate::nested_containment`] as
//! `unit:{process id}:{monotonic ordinal}`, so two units of one process and
//! two units of two sibling children can never collide.

/// The identity of one supervised process unit.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ProcessUnitId(std::sync::Arc<str>);

impl ProcessUnitId {
    /// Creates a new identifier from a string value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(std::sync::Arc::from(value.into()))
    }

    /// Returns the identifier value as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the identifier and returns its string value.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0.to_string()
    }
}

impl core::fmt::Display for ProcessUnitId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ProcessUnitId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
