//! Reading the flags once, and then answering without touching the database again.

use std::collections::HashMap;

use loco_rs::app::AppContext;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use crate::{
    bucket,
    entities::{feature_flag_overrides, feature_flags},
    error::{FlagError, Result},
    scope::FlagScope,
};

/// What the `feature_flags` row said, with the columns that decide anything.
#[derive(Debug, Clone)]
struct Definition {
    enabled: bool,
    rollout_percent: Option<u8>,
    /// Whose audience the rollout draws from, or `None` for the flag's own name.
    bucket_group: Option<String>,
}

/// Every flag, already resolved as far as it can be without being asked.
///
/// **Loaded once, then asked as often as you like.** Two queries happen in [`Flags::load`]; every
/// [`Flags::active`] after that is a hash lookup and, at most, one sha256. That is what keeps
/// `.await?` out from between the lines of business logic, where a flag check would otherwise turn
/// every branch into an async one.
///
/// **It is a snapshot.** A flag switched off while a request is in flight is still on for the rest
/// of that request. For a request that lasts milliseconds that is the correct trade, and making N
/// processes agree faster than that, without a query in the request path, is the interesting
/// problem this version deliberately does not solve.
#[derive(Debug, Clone)]
pub struct Flags {
    /// `None` when loaded globally.
    scope: Option<(String, String)>,
    definitions: HashMap<String, Definition>,
    /// Only the overrides belonging to this load's scope, so a lookup needs no filtering.
    overrides: HashMap<String, bool>,
}

impl Flags {
    /// Read every flag, and every override belonging to this subject.
    ///
    /// Two queries, whatever the number of flags. The overrides query is narrowed to the one
    /// subject in the database rather than here, because an application with many subjects and few
    /// flags would otherwise drag the whole table across the wire on every request.
    ///
    /// # Errors
    /// When the database will not answer.
    pub async fn load(ctx: &AppContext, scope: impl FlagScope) -> Result<Self> {
        Self::load_on(&ctx.db, scope).await
    }

    /// [`Flags::load`], for anything holding a connection rather than a context.
    ///
    /// A worker, a task, one side of a transaction, or a test with no application around it. The
    /// context-shaped call is the one a handler wants; this is the one everything else wants, and
    /// having both is why neither has to reach for the other's shape.
    ///
    /// # Errors
    /// When the database will not answer.
    pub async fn load_on(db: &impl ConnectionTrait, scope: impl FlagScope) -> Result<Self> {
        Self::load_for(db, scope.scope_type(), &scope.scope_id()).await
    }

    /// [`Flags::load_on`], for a subject whose kind is only known at runtime.
    ///
    /// [`FlagScope`] returns a `&'static str` because every real implementor writes a literal. A
    /// task parsing `host:42` off a command line has no literal to return, and this is the way in
    /// that does not ask it to invent one by leaking a string.
    ///
    /// # Errors
    /// When the database will not answer.
    pub async fn load_for(
        db: &impl ConnectionTrait,
        scope_type: &str,
        scope_id: &str,
    ) -> Result<Self> {
        let scope_type = scope_type.to_owned();
        let scope_id = scope_id.to_owned();

        let definitions = read_definitions(db).await?;

        let overrides = feature_flag_overrides::Entity::find()
            .filter(feature_flag_overrides::Column::ScopeType.eq(scope_type.clone()))
            .filter(feature_flag_overrides::Column::ScopeId.eq(scope_id.clone()))
            .all(db)
            .await?
            .into_iter()
            .map(|row| (row.flag_key, row.enabled))
            .collect();

        Ok(Self {
            scope: Some((scope_type, scope_id)),
            definitions,
            overrides,
        })
    }

    /// Read every flag, with nobody in particular in mind.
    ///
    /// One query rather than two: with no subject there are no overrides that could apply. A
    /// percentage rollout asked of this answers `false` and says so loudly, because the question
    /// has no honest answer. See [`FlagError::RolloutWithoutScope`].
    ///
    /// # Errors
    /// When the database will not answer.
    pub async fn load_global(ctx: &AppContext) -> Result<Self> {
        Self::load_global_on(&ctx.db).await
    }

    /// [`Flags::load_global`], for anything holding a connection rather than a context.
    ///
    /// # Errors
    /// When the database will not answer.
    pub async fn load_global_on(db: &impl ConnectionTrait) -> Result<Self> {
        Ok(Self {
            scope: None,
            definitions: read_definitions(db).await?,
            overrides: HashMap::new(),
        })
    }

    /// Is this feature on?
    ///
    /// **Returns `bool` and never `Result`**, because a handler that has just asked "should I draw
    /// this button" has nothing useful to do with a database error, and making every call site
    /// handle one would push `?` into places that should read like prose.
    ///
    /// A name with no row answers `false` **and writes a warning**. In this version a flag name is
    /// a string, so a typo is indistinguishable from a flag that has not been created yet, and that
    /// log line is the only thing between a misspelling and an hour of confusion. Closing that gap
    /// with a compile-time registry is the next version's job.
    #[must_use]
    pub fn active(&self, key: &str) -> bool {
        match self.resolve(key) {
            Ok(answer) => answer,
            Err(FlagError::UnknownFlag(name)) => {
                tracing::warn!(
                    flag = %name,
                    "asked about a feature flag that has no row, answering false. `flag:list` \
                     shows what exists; if this is a typo it will keep answering false for ever"
                );
                false
            }
            Err(error) => {
                tracing::error!(flag = %key, %error, "could not resolve a feature flag, answering false");
                false
            }
        }
    }

    /// [`Flags::active`], for a caller that does want to be told why.
    ///
    /// Exists so that the two real failures, a name nobody created and a rollout with no subject,
    /// are reachable rather than only loggable. A task uses this. A request handler should not.
    ///
    /// # Errors
    /// [`FlagError::UnknownFlag`] for a name with no row, [`FlagError::RolloutWithoutScope`] for a
    /// percentage rollout asked of a global load.
    pub fn try_active(&self, key: &str) -> Result<bool> {
        self.resolve(key)
    }

    /// A `Flags` with fixed answers, for testing code that reads flags.
    ///
    /// **Here because this crate's own shape would otherwise be contagious.** Every other way to
    /// get a `Flags` needs a database, so without this, an application that puts one branch behind
    /// a flag has just made every test of that branch need Postgres. That is a real cost to impose
    /// on a consumer, and the fix is four lines.
    ///
    /// Everything named is on and everything else is unknown, so it answers `false` and logs the
    /// same warning a real miss would. Scoped, so a flag on a percentage rollout resolves rather
    /// than reporting [`FlagError::RolloutWithoutScope`] - though a fixed `Flags` never reaches the
    /// rollout step, because a named flag is on outright.
    ///
    /// ```ignore
    /// let flags = Flags::fixed(["occasions"]);
    /// assert!(flags.active("occasions"));
    /// assert!(!flags.active("paywall"));
    /// ```
    #[must_use]
    pub fn fixed<I, S>(on: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            scope: Some(("test".to_owned(), "test".to_owned())),
            definitions: on
                .into_iter()
                .map(|key| {
                    (
                        key.into(),
                        Definition {
                            enabled: true,
                            rollout_percent: None,
                            bucket_group: None,
                        },
                    )
                })
                .collect(),
            overrides: HashMap::new(),
        }
    }

    /// Whether this flag has a row at all, however it is set.
    #[must_use]
    pub fn is_known(&self, key: &str) -> bool {
        self.definitions.contains_key(key)
    }

    /// Every flag name that has a row, in whatever order the map holds them.
    #[must_use]
    pub fn known(&self) -> Vec<&str> {
        self.definitions.keys().map(String::as_str).collect()
    }

    /// The four-step answer, in the order the design fixed.
    fn resolve(&self, key: &str) -> Result<bool> {
        // 1. A human decided about this subject. Nothing below gets a say.
        if let Some(decided) = self.overrides.get(key) {
            return Ok(*decided);
        }

        let Some(definition) = self.definitions.get(key) else {
            return Err(FlagError::UnknownFlag(key.to_owned()));
        };

        // 2. The kill switch beats the rollout, which is the entire point of having one.
        if !definition.enabled {
            return Ok(false);
        }

        // 3. Not a rollout: being enabled is the whole answer.
        let Some(percent) = definition.rollout_percent else {
            return Ok(true);
        };

        // 4. A rollout, which needs somebody to roll out to.
        let Some((scope_type, scope_id)) = &self.scope else {
            return Err(FlagError::RolloutWithoutScope(key.to_owned()));
        };

        // **The audience is the group's, or the flag's own name.** Two flags without a group reach
        // different people on purpose: without that, the unluckiest tenth of your users would meet
        // every experiment you ever run. Naming a group is how you deliberately undo it, so three
        // switches on one feature can be killed one at a time while reaching one audience.
        let audience = definition.bucket_group.as_deref().unwrap_or(key);

        Ok(bucket::is_inside(audience, scope_type, scope_id, percent))
    }
}

/// Every flag row, keyed by name.
///
/// **An impossible percentage fails closed.** The column carries a `CHECK` between nought and a
/// hundred, so this is unreachable through any supported path, and it is handled anyway because the
/// one way it could arrive, somebody editing the row by hand on an older schema, is also the way it
/// would do the most damage: clamping upward turned a stray `1000` into "release it to everybody",
/// which is the only place in the crate where corruption read as an instruction. A value the crate
/// could not have written is treated as no rollout at all, and said out loud.
async fn read_definitions(db: &impl ConnectionTrait) -> Result<HashMap<String, Definition>> {
    Ok(feature_flags::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|row| {
            let rollout_percent = row.rollout_percent.and_then(|percent| {
                u8::try_from(percent).ok().filter(|percent| *percent <= 100).or_else(|| {
                    tracing::error!(
                        flag = %row.key,
                        rollout_percent = percent,
                        "a rollout percentage outside 0..=100 is not a percentage, so this flag is \
                         being treated as having no rollout at all"
                    );
                    None
                })
            });
            (
                row.key,
                Definition {
                    enabled: row.enabled,
                    rollout_percent,
                    bucket_group: row.bucket_group,
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolution order, without a database anywhere near it.
    fn flags(
        scope: Option<(&str, &str)>,
        definitions: &[(&str, bool, Option<u8>)],
        overrides: &[(&str, bool)],
    ) -> Flags {
        grouped(scope, definitions, overrides, None)
    }

    /// The same, with every flag drawing from one named audience.
    fn grouped(
        scope: Option<(&str, &str)>,
        definitions: &[(&str, bool, Option<u8>)],
        overrides: &[(&str, bool)],
        bucket_group: Option<&str>,
    ) -> Flags {
        Flags {
            scope: scope.map(|(kind, id)| (kind.to_owned(), id.to_owned())),
            definitions: definitions
                .iter()
                .map(|(key, enabled, rollout_percent)| {
                    (
                        (*key).to_owned(),
                        Definition {
                            enabled: *enabled,
                            rollout_percent: *rollout_percent,
                            bucket_group: bucket_group.map(ToOwned::to_owned),
                        },
                    )
                })
                .collect(),
            overrides: overrides
                .iter()
                .map(|(key, enabled)| ((*key).to_owned(), *enabled))
                .collect(),
        }
    }

    #[test]
    fn a_name_nobody_created_is_false_rather_than_an_answer() {
        let flags = flags(None, &[], &[]);
        assert!(!flags.active("nothing"));
        assert!(matches!(
            flags.try_active("nothing"),
            Err(FlagError::UnknownFlag(name)) if name == "nothing"
        ));
    }

    #[test]
    fn enabled_with_no_rollout_is_simply_on() {
        let flags = flags(None, &[("paywall", true, None)], &[]);
        assert!(flags.active("paywall"));
    }

    #[test]
    fn the_kill_switch_beats_the_rollout() {
        // Inside the rollout by bucket, and switched off. Off wins.
        let flags = flags(
            Some(("host", "42")),
            &[("occasions", false, Some(100))],
            &[],
        );
        assert!(!flags.active("occasions"));
    }

    #[test]
    fn an_override_beats_everything_including_the_kill_switch() {
        let flags = flags(
            Some(("host", "42")),
            &[("occasions", false, Some(0))],
            &[("occasions", true)],
        );
        assert!(flags.active("occasions"));
    }

    #[test]
    fn an_override_can_also_take_it_away() {
        let flags = flags(
            Some(("host", "42")),
            &[("occasions", true, None)],
            &[("occasions", false)],
        );
        assert!(!flags.active("occasions"));
    }

    /// An override for a name with no flag row is honoured rather than ignored. The foreign key
    /// makes it unreachable through the tasks, and if it is somehow there it is still a human
    /// decision about a subject, which is what this table means.
    #[test]
    fn an_override_answers_even_when_the_flag_row_is_gone() {
        let flags = flags(Some(("host", "42")), &[], &[("occasions", true)]);
        assert!(flags.active("occasions"));
    }

    #[test]
    fn a_rollout_without_a_subject_is_refused_rather_than_guessed() {
        let flags = flags(None, &[("occasions", true, Some(50))], &[]);
        assert!(matches!(
            flags.try_active("occasions"),
            Err(FlagError::RolloutWithoutScope(name)) if name == "occasions"
        ));
        // And the bool-shaped question still gets a bool.
        assert!(!flags.active("occasions"));
    }

    #[test]
    fn a_rollout_at_a_hundred_reaches_everybody_and_at_nought_reaches_nobody() {
        let everybody = flags(Some(("host", "42")), &[("occasions", true, Some(100))], &[]);
        assert!(everybody.active("occasions"));

        let nobody = flags(Some(("host", "42")), &[("occasions", true, Some(0))], &[]);
        assert!(!nobody.active("occasions"));
    }

    #[test]
    fn a_global_flag_answers_a_scoped_load_the_same_way() {
        let flags = flags(Some(("host", "42")), &[("paywall", true, None)], &[]);
        assert!(flags.active("paywall"));
    }

    /// Two flags at one percentage, in one group, must reach exactly the same people: that is the
    /// whole point of a group, and it is what lets three switches on one feature be killed
    /// separately while the audience stays put.
    #[test]
    fn flags_in_one_group_reach_the_same_people() {
        for subject in 0..500 {
            let id = subject.to_string();
            let scope = Some(("host", id.as_str()));

            let first = grouped(
                scope,
                &[("checkout_button", true, Some(30))],
                &[],
                Some("checkout"),
            );
            let second = grouped(
                scope,
                &[("checkout_summary", true, Some(30))],
                &[],
                Some("checkout"),
            );

            assert_eq!(
                first.active("checkout_button"),
                second.active("checkout_summary"),
                "host {id} was in one half of the checkout and not the other"
            );
        }
    }

    /// And without a group they must not, or one unlucky tenth meets every experiment.
    #[test]
    fn flags_without_a_group_reach_different_people() {
        let answers: Vec<(bool, bool)> = (0..500)
            .map(|subject| {
                let id = subject.to_string();
                let scope = Some(("host", id.as_str()));
                (
                    flags(scope, &[("checkout_button", true, Some(30))], &[])
                        .active("checkout_button"),
                    flags(scope, &[("checkout_summary", true, Some(30))], &[])
                        .active("checkout_summary"),
                )
            })
            .collect();

        assert!(
            answers.iter().any(|(first, second)| first != second),
            "two ungrouped flags picked exactly the same audience"
        );
    }

    #[test]
    fn a_fixed_flags_answers_without_a_database() {
        let flags = Flags::fixed(["occasions", "paywall"]);
        assert!(flags.active("occasions"));
        assert!(flags.active("paywall"));
        assert!(!flags.active("something-else"));
    }

    #[test]
    fn known_reports_what_has_a_row() {
        let flags = flags(None, &[("paywall", true, None)], &[]);
        assert!(flags.is_known("paywall"));
        assert!(!flags.is_known("occasions"));
        assert_eq!(flags.known(), vec!["paywall"]);
    }
}
