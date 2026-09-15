//! One human decision about one subject.
//!
//! **Not a cache.** Pennant has a table shaped a little like this one and it holds *resolved
//! values*, because a PHP closure can answer differently on every call and its answers therefore
//! have to be frozen. There are no closures here, so the rollout is recomputed every time and
//! agrees with itself for ever. What is left in this table is only the thing that cannot be
//! computed: somebody deciding that this particular subject is in or out, whatever the rollout
//! says.
//!
//! That distinction is why nothing ever needs to purge this table.

use sea_orm::entity::prelude::*;

/// One decision about one subject.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
#[sea_orm(table_name = "feature_flag_overrides")]
pub struct Model {
    /// A surrogate key, because the meaningful one is the triple below and a composite primary key
    /// would make every relation in a consumer's own screens carry three columns.
    #[sea_orm(primary_key)]
    pub id: i32,

    /// Which flag this is a decision about.
    pub flag_key: String,

    /// What kind of subject, from [`crate::FlagScope::scope_type`].
    pub scope_type: String,

    /// Which one, from [`crate::FlagScope::scope_id`].
    pub scope_id: String,

    /// In or out. There is no third state: a subject with no row here simply has no override, and
    /// a null here would be a second way of saying the same thing.
    pub enabled: bool,

    /// When the decision was made.
    pub created_at: DateTimeWithTimeZone,
    /// When it was last changed.
    pub updated_at: DateTimeWithTimeZone,
}

/// What a decision belongs to.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The flag it is about.
    #[sea_orm(
        belongs_to = "super::feature_flags::Entity",
        from = "Column::FlagKey",
        to = "super::feature_flags::Column::Key",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Flag,
}

impl Related<super::feature_flags::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Flag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
