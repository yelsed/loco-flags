//! Telling `cargo loco doctor` whether the flags are reachable.
//!
//! **This does not load anything into memory, and that is the point.** An initializer that read
//! every flag at boot would be a cache, and a cache is a promise that N processes agree about a
//! value. They would not: a flag switched off would reach one process at a time, in whatever order
//! they happened to restart, and nothing would say which ones were still serving the old answer.
//! Solving that properly, with no query in the request path, is the interesting problem and it is
//! deliberately not solved here.
//!
//! So what the initializer earns its place with is a health check: the tables are there, or
//! `doctor` says so before an incident does.

use loco_rs::{
    Result,
    app::{AppContext, Initializer as LocoInitializer},
    doctor::{Check, CheckStatus},
};
use sea_orm::EntityTrait;

use crate::entities::feature_flags;

/// Registers the flag health check.
///
/// ```ignore
/// async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
///     Ok(vec![Box::new(loco_flags::Initializer)])
/// }
/// ```
pub struct Initializer;

#[async_trait::async_trait]
impl LocoInitializer for Initializer {
    fn name(&self) -> String {
        "loco-flags".to_string()
    }

    async fn check(&self, ctx: &AppContext) -> Result<Option<Check>> {
        match feature_flags::Entity::find().all(&ctx.db).await {
            Ok(flags) => {
                let live = flags.iter().filter(|flag| flag.enabled).count();
                Ok(Some(Check {
                    status: CheckStatus::Ok,
                    message: format!("feature flags: {} defined, {live} switched on", flags.len()),
                    description: None,
                }))
            }
            Err(error) => Ok(Some(Check {
                status: CheckStatus::NotOk,
                message: "feature flags: the tables could not be read".to_string(),
                description: Some(format!(
                    "{error}\n\nAdd `loco_flags::migrations::CreateFeatureFlags` to your Migrator \
                     and run `cargo loco db migrate`. Until then every flag answers false."
                )),
            })),
        }
    }
}
