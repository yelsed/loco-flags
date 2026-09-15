//! What can go wrong, and who is expected to care.

/// Everything this crate can fail at.
///
/// Note what is **not** here: "the flag you asked about is off". That is an answer, not a failure,
/// and it comes back as `false`.
#[derive(Debug, thiserror::Error)]
pub enum FlagError {
    /// A percentage rollout was asked about with nothing to roll out *to*.
    ///
    /// Deliberately an error rather than a coin flip. "Twenty five percent" has to mean twenty five
    /// percent of some population of subjects; answered per call it would mean twenty five percent
    /// of *requests*, so one reader would see the feature appear and disappear as they clicked
    /// around. That is not what anybody means, and guessing it would be worse than saying so.
    #[error(
        "the flag `{0}` is a rollout, so it needs a scope: load it with `Flags::load` rather than \
         `Flags::load_global`"
    )]
    RolloutWithoutScope(String),

    /// A task was pointed at a key nobody has created.
    ///
    /// Belongs to the tasks and never to evaluation. `flag:on key:typo` says so and changes
    /// nothing, rather than helpfully creating a flag that the code will never ask about.
    #[error("no flag is called `{0}`. `flag:list` shows the ones that exist")]
    UnknownFlag(String),

    /// The database would not answer.
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// The result of anything in this crate that can fail.
pub type Result<T> = std::result::Result<T, FlagError>;

impl From<FlagError> for loco_rs::Error {
    /// So a handler or a task can use `?` on us without writing a conversion.
    fn from(error: FlagError) -> Self {
        match error {
            FlagError::Db(inner) => Self::DB(inner),
            other => Self::Message(other.to_string()),
        }
    }
}
