//! Migrations a consumer adds to their own `Migrator`.
//!
//! ```ignore
//! // migration/src/lib.rs
//! vec![
//!     // ... the application's own migrations ...
//!     Box::new(loco_flags::migrations::CreateFeatureFlags),
//! ]
//! ```
//!
//! **Not a `MigratorTrait` of our own.** A second migrator would mean a second
//! `seaql_migrations` table and a second thing to remember to run; putting the migration in the
//! application's own list means `cargo loco db migrate` already covers it and `db status` already
//! reports it.
//!
//! Where in the list is the consumer's choice and it does not matter, because nothing here refers
//! to any of their tables. Appending is the ordinary answer.

mod m20260914_000001_create_feature_flags;

/// The two tables. See the module documentation for where to put it.
pub use m20260914_000001_create_feature_flags::Migration as CreateFeatureFlags;

/// Every migration this crate ships, in order, for a consumer who would rather splice the whole
/// list in than name them one at a time.
#[must_use]
pub fn all() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
    vec![Box::new(CreateFeatureFlags)]
}
