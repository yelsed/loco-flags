//! Who a flag is being asked about.
//!
//! **No type from any application appears in this crate.** An application teaches its own types to
//! be subjects by implementing one trait on them, which is what lets this be published without
//! knowing anything about hosts, parties, tenants or accounts.

/// A subject a flag can be resolved for.
///
/// ```ignore
/// impl FlagScope for Host {
///     fn scope_type(&self) -> &'static str { "host" }
///     fn scope_id(&self) -> String { self.id.to_string() }
/// }
/// ```
///
/// Both halves are stored on an override row and both go into the rollout digest, so a `Host` 42
/// and a `Party` 42 are different subjects and are never confused for one another.
pub trait FlagScope {
    /// What kind of thing this is. A short, stable, lowercase word.
    ///
    /// **Changing it later moves every subject of that kind to a different place in every
    /// rollout**, and orphans every override row already written against the old spelling. Pick it
    /// once.
    ///
    /// **`&'static str`, because for every real implementor this is a literal.** `&str` was tried
    /// and reverted: clippy's `unnecessary_literal_bound` fires on any implementation that returns
    /// a literal, so the looser type moved friction out of this crate and into every consumer that
    /// lints. A kind chosen at runtime does not go through this trait at all, and uses
    /// [`crate::Flags::load_for`] and the `_for` functions in [`crate::store`] instead.
    fn scope_type(&self) -> &'static str;

    /// Which one it is, as text.
    ///
    /// Text rather than an integer because a subject is as likely to be a uuid, a slug or a tenant
    /// name as a row id, and a crate that insisted on `i64` would be telling applications how to
    /// key their own tables.
    fn scope_id(&self) -> String;
}

/// A subject named directly, for when there is no type to hang the trait on yet.
///
/// Useful in a task, in a test, or the first time a flag is tried out before the application has
/// decided which of its types is the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    kind: &'static str,
    id: String,
}

impl Subject {
    /// Name a subject by its kind and its identifier.
    pub fn new(kind: &'static str, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

impl FlagScope for Subject {
    fn scope_type(&self) -> &'static str {
        self.kind
    }

    fn scope_id(&self) -> String {
        self.id.clone()
    }
}

/// So `&Host` works anywhere `Host` does, which is what a caller writes without thinking about it.
impl<T: FlagScope + ?Sized> FlagScope for &T {
    fn scope_type(&self) -> &'static str {
        (**self).scope_type()
    }

    fn scope_id(&self) -> String {
        (**self).scope_id()
    }
}
