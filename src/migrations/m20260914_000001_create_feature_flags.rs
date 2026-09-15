//! The two tables, created in the consumer's own migration run.
//!
//! **Named by hand rather than by `DeriveMigrationName`.** That macro takes the name from the
//! module path, which for a library is this crate's path and not the consumer's, and the name is
//! what lands in their `seaql_migrations` table for ever. Writing it out means the row says
//! something a person reading their migration history can recognise, and means renaming this file
//! later cannot quietly ask every deployment to run it again.

// `MigrationTrait` declares `&SchemaManager` with the lifetime elided, so an implementation has to
// elide it too or the signatures do not match. That disagrees with `rust_2018_idioms`, which this
// crate turns on everywhere else, so the exception is named here rather than switched off globally.
#![allow(elided_lifetimes_in_paths)]

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveIden)]
enum FeatureFlags {
    Table,
    Key,
    Enabled,
    RolloutPercent,
    Description,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum FeatureFlagOverrides {
    Table,
    Id,
    FlagKey,
    ScopeType,
    ScopeId,
    Enabled,
    CreatedAt,
    UpdatedAt,
}

/// Creates `feature_flags` and `feature_flag_overrides`.
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &'static str {
        "m20260914_000001_create_feature_flags"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FeatureFlags::Table)
                    .if_not_exists()
                    // The name is the identity. A surrogate id would be a second thing that could
                    // disagree with it, and nothing would ever join on it.
                    .col(text(FeatureFlags::Key).primary_key())
                    // Off until somebody says otherwise. A flag that arrived switched on would
                    // make deploying the migration a release of the feature.
                    .col(boolean(FeatureFlags::Enabled).default(false))
                    // Null means "not a rollout at all", which is a different thing from nought
                    // percent. Nought is a prepared flag reaching nobody yet.
                    .col(small_integer_null(FeatureFlags::RolloutPercent))
                    .col(text_null(FeatureFlags::Description))
                    .col(
                        timestamp_with_time_zone(FeatureFlags::CreatedAt)
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        timestamp_with_time_zone(FeatureFlags::UpdatedAt)
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FeatureFlagOverrides::Table)
                    .if_not_exists()
                    .col(pk_auto(FeatureFlagOverrides::Id))
                    .col(text(FeatureFlagOverrides::FlagKey))
                    .col(text(FeatureFlagOverrides::ScopeType))
                    .col(text(FeatureFlagOverrides::ScopeId))
                    .col(boolean(FeatureFlagOverrides::Enabled))
                    .col(
                        timestamp_with_time_zone(FeatureFlagOverrides::CreatedAt)
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        timestamp_with_time_zone(FeatureFlagOverrides::UpdatedAt)
                            .default(Expr::current_timestamp()),
                    )
                    // Deleting a flag takes its overrides with it. An override for a flag that no
                    // longer exists is a decision about nothing, and leaving them behind would
                    // make recreating a key inherit somebody's forgotten exceptions.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-feature_flag_overrides-flag_key-to-feature_flags")
                            .from(FeatureFlagOverrides::Table, FeatureFlagOverrides::FlagKey)
                            .to(FeatureFlags::Table, FeatureFlags::Key)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // **One decision per subject per flag, held by the database.** Two operators answering the
        // same question at once is exactly the read-then-write that a check in the task layer
        // loses, and the loser would leave two contradictory rows with nothing to say which wins.
        manager
            .create_index(
                Index::create()
                    .name("one_override_per_scope_per_flag")
                    .table(FeatureFlagOverrides::Table)
                    .col(FeatureFlagOverrides::FlagKey)
                    .col(FeatureFlagOverrides::ScopeType)
                    .col(FeatureFlagOverrides::ScopeId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Every request with a subject runs exactly this lookup.
        manager
            .create_index(
                Index::create()
                    .name("overrides_by_subject")
                    .table(FeatureFlagOverrides::Table)
                    .col(FeatureFlagOverrides::ScopeType)
                    .col(FeatureFlagOverrides::ScopeId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(FeatureFlagOverrides::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(FeatureFlags::Table).if_exists().to_owned())
            .await?;
        Ok(())
    }
}
