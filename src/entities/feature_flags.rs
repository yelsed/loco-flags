//! What a flag is, as a row.

use sea_orm::entity::prelude::*;

/// One flag.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
#[sea_orm(table_name = "feature_flags")]
pub struct Model {
    /// The name the code asks for. The primary key, because a flag *is* its name: there is no
    /// second identity to keep in step with it, and a numeric id would only be a thing that could
    /// disagree with the name.
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,

    /// The kill switch, and it beats the rollout.
    ///
    /// A flag at ten percent with `enabled = false` is off for everybody. That ordering is the
    /// whole reason an operations toggle is trustworthy at three in the morning: switching it off
    /// means off, not "off for the ninety percent who were not chosen".
    pub enabled: bool,

    /// `None` means this is not a rollout at all and `enabled` is the entire answer.
    ///
    /// `Some(0)` is a real and useful state, which is why this is not "zero means no rollout": it
    /// is a flag that has been prepared, is switched on, and is deliberately reaching nobody yet.
    pub rollout_percent: Option<i16>,

    /// Whose audience this flag's rollout uses, or `None` for its own name.
    ///
    /// Two flags with the same group reach the same people at the same percentage. Two flags
    /// without one reach different people, which is the default because it is what stops one
    /// unlucky tenth of an audience meeting every experiment there is.
    pub bucket_group: Option<String>,

    /// What this flag is for, in a sentence, for whoever finds it in `flag:list` in a year.
    pub description: Option<String>,

    /// When the flag was created.
    pub created_at: DateTimeWithTimeZone,
    /// When it was last changed.
    pub updated_at: DateTimeWithTimeZone,
}

/// What hangs off a flag.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// Every decision made about a particular subject.
    #[sea_orm(has_many = "super::feature_flag_overrides::Entity")]
    Overrides,
}

impl Related<super::feature_flag_overrides::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Overrides.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
