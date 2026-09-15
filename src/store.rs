//! Every write, in one place.
//!
//! The tasks are a thin skin over these: parse arguments, call one of these, print a sentence. A
//! button in an application's own admin screen sits on exactly the same functions, which is what
//! keeps "there is no admin screen yet" from being a decision that has to be unmade later.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, TransactionSession, TransactionTrait,
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
        bucket_group: ActiveValue::NotSet,
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
/// ordering, `flag:off` after `flag:rollout`, is available and means what it says. Note that this
/// means a flag killed during an incident comes back on if somebody sets a percentage on it.
///
/// Anything above a hundred is **refused rather than clamped**. Clamping silently turned a
/// mistyped `1000` into "release it to everybody", and it disagreed with the task, which refused
/// the same number: two ways in, opposite answers, and the quiet one was the dangerous one.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, [`FlagError::NotAPercentage`] above a hundred, or
/// when the database will not answer.
pub async fn set_rollout(
    db: &impl ConnectionTrait,
    key: &str,
    percent: u8,
) -> Result<feature_flags::Model> {
    if percent > 100 {
        return Err(FlagError::NotAPercentage(percent));
    }
    let flag = require(db, key).await?;
    let mut editing: feature_flags::ActiveModel = flag.into();
    editing.rollout_percent = ActiveValue::Set(Some(i16::from(percent)));
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

/// Draw this flag's rollout from a named audience rather than from its own name.
///
/// **What this is for.** Two flags at ten percent normally reach two different tenths, which is
/// deliberate: it stops one unlucky tenth of your users meeting every experiment you run. When you
/// want the opposite, because three switches belong to one feature and each needs its own kill
/// switch while the audience stays put, give them all the same group.
///
/// Changing or clearing a group **reshuffles who is inside this flag's rollout**, because the
/// audience is what the digest is over. Set it before the rollout starts, not during one.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_bucket_group(
    db: &impl ConnectionTrait,
    key: &str,
    group: Option<&str>,
) -> Result<feature_flags::Model> {
    let flag = require(db, key).await?;
    let mut editing: feature_flags::ActiveModel = flag.into();
    editing.bucket_group = ActiveValue::Set(group.map(ToOwned::to_owned));
    editing.updated_at = ActiveValue::Set(chrono::Utc::now().into());
    Ok(editing.update(db).await?)
}

/// Decide about one subject, whatever the flag says.
///
/// Written as delete-then-insert rather than an upsert so the behaviour is the same on every
/// backend sea-orm supports, and because the unique index is what actually holds the invariant.
///
/// Takes anything that can begin a transaction, which includes a transaction, so a caller already
/// inside one gets a savepoint rather than a refusal.
///
/// **In one transaction**, which is not a detail. Between the delete and the insert the subject has
/// no decision at all, so a failure in the gap, or a concurrent reader arriving in it, would see
/// them fall back to whatever the flag says. For a subject deliberately excluded from something
/// that is sold, that gap is the feature being given away; for one deliberately included, it is
/// being taken back. Either way it is a window nobody asked for.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_override<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    key: &str,
    scope: impl FlagScope,
    enabled: bool,
) -> Result<feature_flag_overrides::Model> {
    set_override_for(db, key, scope.scope_type(), &scope.scope_id(), enabled).await
}

/// [`set_override`], for a subject whose kind is only known at runtime.
///
/// See [`crate::Flags::load_for`] for why this exists beside the trait-shaped one.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn set_override_for<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    key: &str,
    scope_type: &str,
    scope_id: &str,
    enabled: bool,
) -> Result<feature_flag_overrides::Model> {
    let scope_type = scope_type.to_owned();
    let scope_id = scope_id.to_owned();

    let writing = db.begin().await?;
    require(&writing, key).await?;
    clear_override_rows(&writing, key, &scope_type, &scope_id).await?;

    let decision = feature_flag_overrides::ActiveModel {
        id: ActiveValue::NotSet,
        flag_key: ActiveValue::Set(key.to_owned()),
        scope_type: ActiveValue::Set(scope_type),
        scope_id: ActiveValue::Set(scope_id),
        enabled: ActiveValue::Set(enabled),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&writing)
    .await?;

    writing.commit().await?;
    Ok(decision)
}

/// Forget a decision about one subject, putting them back under whatever the flag says.
///
/// Goes through [`require`] like every other write, so a mistyped key says so rather than reporting
/// the honest-looking "nothing changed" that a delete of no rows would otherwise produce.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn clear_override(
    db: &impl ConnectionTrait,
    key: &str,
    scope: impl FlagScope,
) -> Result<u64> {
    clear_override_for(db, key, scope.scope_type(), &scope.scope_id()).await
}

/// [`clear_override`], for a subject whose kind is only known at runtime.
///
/// # Errors
/// [`FlagError::UnknownFlag`] when no such key, or when the database will not answer.
pub async fn clear_override_for(
    db: &impl ConnectionTrait,
    key: &str,
    scope_type: &str,
    scope_id: &str,
) -> Result<u64> {
    require(db, key).await?;
    clear_override_rows(db, key, scope_type, scope_id).await
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
