//! Every write, in one place.
//!
//! The tasks are a thin skin over these: parse arguments, call one of these, print a sentence. A
//! button in an application's own admin screen sits on exactly the same functions, which is what
//! keeps "there is no admin screen yet" from being a decision that has to be unmade later.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    entities::{feature_flag_overrides, feature_flags},
    error::{FlagError, Result},
    scope::FlagScope,
};

/// Every flag, by name.
///
/// # Errors
/// When the database will not answer.
pub async fn list(db: &impl ConnectionTrait) -> Result<Vec<feature_flags::Model>> {
    Ok(feature_flags::Entity::find()
        .order_by_asc(feature_flags::Column::Key)
        .all(db)
        .await?)
}

/// One flag, or nothing.
///
/// # Errors
/// When the database will not answer.
pub async fn find(db: &impl ConnectionTrait, key: &str) -> Result<Option<feature_flags::Model>> {
    Ok(feature_flags::Entity::find_by_id(key.to_owned())
        .one(db)
        .await?)
}

/// Create a flag, switched off.
///
/// Off, always, and not a parameter: a flag that could be born switched on would make adding one a
/// release of whatever it guards, which is the opposite of the reason to have flags.
///
/// # Errors
/// When the database will not answer, including when the key already exists.
pub async fn create(
    db: &impl ConnectionTrait,
    key: &str,
    description: Option<String>,
) -> Result<feature_flags::Model> {
    Ok(feature_flags::ActiveModel {
        key: ActiveValue::Set(key.to_owned()),
        enabled: ActiveValue::Set(false),
        rollout_percent: ActiveValue::NotSet,
        description: ActiveValue::Set(description),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await?)
}

/// Switch a flag on or off, leaving any rollout percentage where it is.
///
/// Leaving it is deliberate. Switching off in an incident and back on afterwards should return the
/// flag to the rollout it was on, not to everybody at once.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_enabled(
    db: &impl ConnectionTrait,
    key: &str,
    enabled: bool,
) -> Result<feature_flags::Model> {
    let flag = require(db, key).await?;
    let mut editing: feature_flags::ActiveModel = flag.into();
    editing.enabled = ActiveValue::Set(enabled);
    editing.updated_at = ActiveValue::Set(chrono::Utc::now().into());
    Ok(editing.update(db).await?)
}

/// Put a flag on a percentage rollout, and switch it on.
///
/// **Switching it on is part of this**, because a rollout on a flag whose kill switch is off
/// reaches nobody, and setting a percentage is nobody's way of saying "still off". The reverse
/// ordering, `flag:off` after `flag:rollout`, is available and means what it says.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_rollout(
    db: &impl ConnectionTrait,
    key: &str,
    percent: u8,
) -> Result<feature_flags::Model> {
    let flag = require(db, key).await?;
    let mut editing: feature_flags::ActiveModel = flag.into();
    editing.rollout_percent = ActiveValue::Set(Some(i16::from(percent.min(100))));
    editing.enabled = ActiveValue::Set(true);
    editing.updated_at = ActiveValue::Set(chrono::Utc::now().into());
    Ok(editing.update(db).await?)
}

/// Take a flag off its rollout, so `enabled` is the whole answer again.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn clear_rollout(db: &impl ConnectionTrait, key: &str) -> Result<feature_flags::Model> {
    let flag = require(db, key).await?;
    let mut editing: feature_flags::ActiveModel = flag.into();
    editing.rollout_percent = ActiveValue::Set(None);
    editing.updated_at = ActiveValue::Set(chrono::Utc::now().into());
    Ok(editing.update(db).await?)
}

/// Decide about one subject, whatever the flag says.
///
/// Written as delete-then-insert rather than an upsert so that the behaviour is the same on every
/// backend sea-orm supports, and because the unique index is what actually holds the invariant.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_override(
    db: &impl ConnectionTrait,
    key: &str,
    scope: impl FlagScope,
    enabled: bool,
) -> Result<feature_flag_overrides::Model> {
    require(db, key).await?;

    let scope_type = scope.scope_type().to_owned();
    let scope_id = scope.scope_id();

    clear_override_rows(db, key, &scope_type, &scope_id).await?;

    Ok(feature_flag_overrides::ActiveModel {
        id: ActiveValue::NotSet,
        flag_key: ActiveValue::Set(key.to_owned()),
        scope_type: ActiveValue::Set(scope_type),
        scope_id: ActiveValue::Set(scope_id),
        enabled: ActiveValue::Set(enabled),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await?)
}

/// Forget a decision about one subject, putting them back under whatever the flag says.
///
/// # Errors
/// When the database will not answer.
pub async fn clear_override(
    db: &impl ConnectionTrait,
    key: &str,
    scope: impl FlagScope,
) -> Result<u64> {
    let scope_type = scope.scope_type().to_owned();
    let scope_id = scope.scope_id();
    clear_override_rows(db, key, &scope_type, &scope_id).await
}

/// Every decision made about this flag.
///
/// # Errors
/// When the database will not answer.
pub async fn overrides_of(
    db: &impl ConnectionTrait,
    key: &str,
) -> Result<Vec<feature_flag_overrides::Model>> {
    Ok(feature_flag_overrides::Entity::find()
        .filter(feature_flag_overrides::Column::FlagKey.eq(key.to_owned()))
        .order_by_asc(feature_flag_overrides::Column::ScopeType)
        .order_by_asc(feature_flag_overrides::Column::ScopeId)
        .all(db)
        .await?)
}

/// Delete a flag, and with it every decision anybody made about it.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn delete(db: &impl ConnectionTrait, key: &str) -> Result<()> {
    require(db, key).await?;
    feature_flags::Entity::delete_by_id(key.to_owned())
        .exec(db)
        .await?;
    Ok(())
}

/// The flag, or [`FlagError::UnknownFlag`].
///
/// Every write goes through this. `flag:on key:typo` saying "no flag is called typo" and changing
/// nothing is the whole reason: the alternative, creating what was asked for, turns a misspelling
/// into a second flag that the code will never ask about and nobody will ever find.
async fn require(db: &impl ConnectionTrait, key: &str) -> Result<feature_flags::Model> {
    find(db, key)
        .await?
        .ok_or_else(|| FlagError::UnknownFlag(key.to_owned()))
}

async fn clear_override_rows(
    db: &impl ConnectionTrait,
    key: &str,
    scope_type: &str,
    scope_id: &str,
) -> Result<u64> {
    Ok(feature_flag_overrides::Entity::delete_many()
        .filter(feature_flag_overrides::Column::FlagKey.eq(key.to_owned()))
        .filter(feature_flag_overrides::Column::ScopeType.eq(scope_type.to_owned()))
        .filter(feature_flag_overrides::Column::ScopeId.eq(scope_id.to_owned()))
        .exec(db)
        .await?
        .rows_affected)
}
