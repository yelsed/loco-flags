//! Runtime feature flags for [loco.rs](https://loco.rs), kept in the database.
//!
//! Loco's own "feature flags" are Cargo features, which are decided when you compile. These are
//! decided while the application is running: switching one on, off, or up to a percentage is a
//! task or a button, and never a deploy.
//!
//! # The whole of it
//!
//! ```ignore
//! let flags = Flags::load(&ctx, &host).await?;   // two queries, once per request
//!
//! if flags.active("occasions") { /* ... */ }     // synchronous, returns bool
//! if flags.active("paywall")   { /* ... */ }     // a global flag answers the same call
//! ```
//!
//! Everything is read up front so that no `.await?` appears between the lines of business logic
//! that ask about it.
//!
//! # Wiring it in
//!
//! Three lines, and no fork of loco.
//!
//! ```ignore
//! // src/app.rs
//! async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
//!     Ok(vec![Box::new(loco_flags::Initializer)])
//! }
//!
//! fn register_tasks(tasks: &mut Tasks) {
//!     loco_flags::register_tasks(tasks);
//! }
//!
//! // migration/src/lib.rs
//! Box::new(loco_flags::migrations::CreateFeatureFlags),
//! ```
//!
//! Then teach one of your own types to be a subject:
//!
//! ```ignore
//! impl FlagScope for Host {
//!     fn scope_type(&self) -> &'static str { "host" }
//!     fn scope_id(&self) -> String { self.id.to_string() }
//! }
//! ```
//!
//! # How an answer is reached
//!
//! In this order, and the order is the design:
//!
//! 1. an override row for this subject, which is a human decision and beats everything;
//! 2. `enabled = false`, so the kill switch beats the rollout;
//! 3. no rollout percentage, so being on is the whole answer;
//! 4. otherwise, whether this subject's bucket falls under the percentage.
//!
//! Step 2 is what makes an operations toggle worth having at three in the morning: off means off,
//! not "off for the ninety percent who were not chosen".
//!
//! # What this is not
//!
//! **Two tables, not three, and no way to define a flag as a rule in code.** Those are the same
//! decision. A rule can answer differently on every call, so its answers have to be frozen the
//! moment they are first given, and that store then needs invalidating and purging. Here the answer
//! is arithmetic over the flag and the subject, recomputed every time and identical for ever, so
//! there is nothing to store and a read is not secretly a write.
//!
//! And deliberately, in this version: no process-wide cache, so two queries per request; flag names
//! are strings, so a typo is a warning in the log rather than a compile error; booleans only, so no
//! A/B/C variants; and no admin screen. Each of those is a seam rather than an oversight.

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::pedantic, clippy::nursery, rust_2018_idioms)]

pub mod bucket;
pub mod entities;
pub mod error;
pub mod flags;
pub mod migrations;
pub mod scope;
pub mod store;
pub mod tasks;

mod initializer;

pub use error::{FlagError, Result};
pub use flags::Flags;
pub use initializer::Initializer;
pub use scope::{FlagScope, Subject};

/// Add every flag task to an application's registry.
///
/// ```ignore
/// fn register_tasks(tasks: &mut Tasks) {
///     loco_flags::register_tasks(tasks);
/// }
/// ```
pub fn register_tasks(tasks: &mut loco_rs::task::Tasks) {
    tasks::register(tasks);
}
