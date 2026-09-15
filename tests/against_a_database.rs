//! The half that a pure test cannot reach: the tables, the constraints and the cascade.
//!
//! **Marked `#[ignore]`, and that is not the same as skipped.** These need a Postgres, and a crate
//! whose `cargo test` fails on a machine without one is a crate nobody runs the rest of the suite
//! on. They are run deliberately:
//!
//! ```sh
//! docker run --rm -d -p 55433:5432 -e POSTGRES_USER=loco -e POSTGRES_PASSWORD=loco postgres:16
//! LOCO_FLAGS_TEST_DATABASE_URL=postgres://loco:loco@localhost:55433/loco_flags_test \
//!   cargo test --test against_a_database -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` because every one of them lays the schema down again, and two doing that at
//! once deadlock on the drop. `--test` names this binary rather than the whole suite, because
//! `--ignored` on its own also asks rustdoc to run the `ignore`-marked examples in the
//! documentation, which are illustrations and were never meant to compile.

use loco_flags::{
    entities::feature_flag_overrides, migrations, store, FlagScope, Flags, Subject,
};
use sea_orm::{ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait, QueryFilter};
use sea_orm_migration::MigratorTrait;

/// A migrator holding only this crate's migrations, which is what a consumer splices into theirs.
struct OnlyTheFlags;

#[async_trait::async_trait]
impl MigratorTrait for OnlyTheFlags {
    fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
        migrations::all()
    }
}

/// A clean database with the two tables in it.
async fn fresh() -> DatabaseConnection {
    let url = std::env::var("LOCO_FLAGS_TEST_DATABASE_URL").expect(
        "these tests need LOCO_FLAGS_TEST_DATABASE_URL; the module comment says how to get one",
    );

    let db = Database::connect(&url)
        .await
        .expect("the test database accepts a connection");

    OnlyTheFlags::fresh(&db)
        .await
        .expect("the migrations lay the schema down");

    db
}

/// The subject used throughout, so the tests read like one story.
fn host(id: &str) -> Subject {
    Subject::new("host", id)
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_flag_is_created_switched_off() {
    let db = fresh().await;

    store::create(&db, "paywall", Some("kill switch".to_string()))
        .await
        .expect("creating works");

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(flags.is_known("paywall"));
    assert!(
        !flags.active("paywall"),
        "a new flag must arrive off, or creating one is a release"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn switching_on_and_off_is_read_back() {
    let db = fresh().await;
    store::create(&db, "paywall", None).await.expect("created");

    store::set_enabled(&db, "paywall", true).await.expect("on");
    assert!(Flags::load_global_on(&db).await.unwrap().active("paywall"));

    store::set_enabled(&db, "paywall", false).await.expect("off");
    assert!(!Flags::load_global_on(&db).await.unwrap().active("paywall"));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_task_against_a_key_nobody_created_changes_nothing() {
    let db = fresh().await;

    let refused = store::set_enabled(&db, "typo", true).await;
    assert!(
        matches!(refused, Err(loco_flags::FlagError::UnknownFlag(key)) if key == "typo"),
        "a write to an unknown key must say so rather than create it"
    );

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(flags.known().is_empty(), "nothing was created behind our back");
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_kill_switch_beats_the_rollout_through_the_database() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");

    // Everybody, by percentage.
    store::set_rollout(&db, "occasions", 100).await.expect("rolled out");
    assert!(Flags::load_on(&db, host("42")).await.unwrap().active("occasions"));

    // And now off, which must win.
    store::set_enabled(&db, "occasions", false).await.expect("off");
    assert!(!Flags::load_on(&db, host("42")).await.unwrap().active("occasions"));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_rollout_switches_the_flag_on_because_a_rollout_that_reaches_nobody_is_not_one() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");

    let flag = store::set_rollout(&db, "occasions", 25).await.expect("rolled out");
    assert!(flag.enabled, "setting a percentage switches the flag on");
    assert_eq!(flag.rollout_percent, Some(25));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn an_override_beats_the_flag_in_both_directions() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_enabled(&db, "occasions", false).await.expect("off");

    store::set_override(&db, "occasions", host("42"), true)
        .await
        .expect("forced on");
    assert!(
        Flags::load_on(&db, host("42")).await.unwrap().active("occasions"),
        "an override must beat even the kill switch"
    );
    assert!(
        !Flags::load_on(&db, host("43")).await.unwrap().active("occasions"),
        "and must not reach anybody else"
    );

    store::set_enabled(&db, "occasions", true).await.expect("on");
    store::set_override(&db, "occasions", host("42"), false)
        .await
        .expect("forced off");
    assert!(!Flags::load_on(&db, host("42")).await.unwrap().active("occasions"));
    assert!(Flags::load_on(&db, host("43")).await.unwrap().active("occasions"));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn setting_an_override_twice_leaves_one_row_and_the_second_answer() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");

    store::set_override(&db, "occasions", host("42"), true).await.expect("first");
    store::set_override(&db, "occasions", host("42"), false).await.expect("second");

    let rows = feature_flag_overrides::Entity::find()
        .filter(feature_flag_overrides::Column::FlagKey.eq("occasions"))
        .all(&db)
        .await
        .expect("reading works");

    assert_eq!(rows.len(), 1, "one decision per subject per flag");
    assert!(!rows[0].enabled, "and it is the most recent one");
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_unique_index_is_the_thing_holding_one_decision_per_subject() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_override(&db, "occasions", host("42"), true).await.expect("first");

    // Behind the store's back, the way a second operator racing the first would arrive.
    let refused = db
        .execute_unprepared(
            "INSERT INTO feature_flag_overrides (flag_key, scope_type, scope_id, enabled) \
             VALUES ('occasions', 'host', '42', false)",
        )
        .await;

    assert!(
        refused.is_err(),
        "a second decision about one subject must be refused by the database, not by a check in \
         the task layer that a race can lose"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn clearing_an_override_puts_the_subject_back_under_the_flag() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_enabled(&db, "occasions", true).await.expect("on");
    store::set_override(&db, "occasions", host("42"), false).await.expect("forced off");

    assert!(!Flags::load_on(&db, host("42")).await.unwrap().active("occasions"));

    let removed = store::clear_override(&db, "occasions", host("42")).await.expect("cleared");
    assert_eq!(removed, 1);
    assert!(Flags::load_on(&db, host("42")).await.unwrap().active("occasions"));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn deleting_a_flag_takes_its_overrides_with_it() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_override(&db, "occasions", host("42"), true).await.expect("forced on");

    store::delete(&db, "occasions").await.expect("deleted");

    let left = feature_flag_overrides::Entity::find()
        .all(&db)
        .await
        .expect("reading works");
    assert!(
        left.is_empty(),
        "an exception on a flag that no longer exists is a decision about nothing, and would be \
         inherited by anybody who recreated the key"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn an_override_cannot_be_written_for_a_flag_that_does_not_exist() {
    let db = fresh().await;

    let refused = store::set_override(&db, "occasions", host("42"), true).await;
    assert!(matches!(
        refused,
        Err(loco_flags::FlagError::UnknownFlag(key)) if key == "occasions"
    ));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_scope_type_and_an_identifier_together_are_who_you_are() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_override(&db, "occasions", Subject::new("host", "42"), true)
        .await
        .expect("forced on for the host");

    let as_host = Flags::load_on(&db, Subject::new("host", "42")).await.unwrap();
    let as_party = Flags::load_on(&db, Subject::new("party", "42")).await.unwrap();

    assert!(as_host.active("occasions"));
    assert!(
        !as_party.active("occasions"),
        "party 42 is not host 42, and an override on one must not reach the other"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_rollout_read_from_the_database_matches_the_pure_function() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_rollout(&db, "occasions", 30).await.expect("rolled out");

    for subject in 0..200 {
        let id = subject.to_string();
        let through_the_database = Flags::load_on(&db, host(&id))
            .await
            .unwrap()
            .active("occasions");
        let directly = loco_flags::bucket::is_inside("occasions", "host", &id, 30);

        assert_eq!(
            through_the_database, directly,
            "host {id} was answered differently by the database and by the bucket"
        );
    }
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_rollout_asked_without_a_subject_is_refused() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_rollout(&db, "occasions", 50).await.expect("rolled out");

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(matches!(
        flags.try_active("occasions"),
        Err(loco_flags::FlagError::RolloutWithoutScope(key)) if key == "occasions"
    ));
    assert!(!flags.active("occasions"));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn clearing_a_rollout_leaves_the_flag_answering_for_everybody() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_rollout(&db, "occasions", 1).await.expect("rolled out");
    store::clear_rollout(&db, "occasions").await.expect("cleared");

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(
        flags.active("occasions"),
        "with no percentage left, being switched on is the whole answer"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_migration_can_be_rolled_back_and_laid_down_again() {
    let db = fresh().await;
    store::create(&db, "paywall", None).await.expect("created");

    OnlyTheFlags::down(&db, None).await.expect("down works");
    OnlyTheFlags::up(&db, None).await.expect("and up again");

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(
        flags.known().is_empty(),
        "down then up is a clean schema, not a half-dropped one"
    );
}

/// A consumer's own type, to prove nothing in this crate needs to know about it.
struct Host {
    id: i32,
}

impl FlagScope for Host {
    fn scope_type(&self) -> &'static str {
        "host"
    }

    fn scope_id(&self) -> String {
        self.id.to_string()
    }
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn an_application_teaches_its_own_type_to_be_a_subject() {
    let db = fresh().await;
    store::create(&db, "occasions", None).await.expect("created");
    store::set_override(&db, "occasions", &Host { id: 42 }, true)
        .await
        .expect("forced on");

    assert!(Flags::load_on(&db, &Host { id: 42 })
        .await
        .unwrap()
        .active("occasions"));
    assert!(!Flags::load_on(&db, &Host { id: 43 })
        .await
        .unwrap()
        .active("occasions"));
}
