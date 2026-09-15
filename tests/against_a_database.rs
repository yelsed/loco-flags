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

use loco_flags::{FlagScope, Flags, Subject, entities::feature_flag_overrides, migrations, store};
use sea_orm::{
    ColumnTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter,
};
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

    // **A small pool, on purpose.** sea-orm's default is a hundred connections, and every test in
    // this file opens its own; the first run of the expanded suite exhausted Postgres and failed
    // twenty-eight tests with `PoolTimedOut`, which reads exactly like a broken crate and was a
    // broken test helper.
    let mut options = ConnectOptions::new(url);
    options.max_connections(2).min_connections(0);

    let db = Database::connect(options)
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

    store::set_enabled(&db, "paywall", false)
        .await
        .expect("off");
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
    assert!(
        flags.known().is_empty(),
        "nothing was created behind our back"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_kill_switch_beats_the_rollout_through_the_database() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");

    // Everybody, by percentage.
    store::set_rollout(&db, "occasions", 100)
        .await
        .expect("rolled out");
    assert!(
        Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );

    // And now off, which must win.
    store::set_enabled(&db, "occasions", false)
        .await
        .expect("off");
    assert!(
        !Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_rollout_switches_the_flag_on_because_a_rollout_that_reaches_nobody_is_not_one() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");

    let flag = store::set_rollout(&db, "occasions", 25)
        .await
        .expect("rolled out");
    assert!(flag.enabled, "setting a percentage switches the flag on");
    assert_eq!(flag.rollout_percent, Some(25));
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn an_override_beats_the_flag_in_both_directions() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_enabled(&db, "occasions", false)
        .await
        .expect("off");

    store::set_override(&db, "occasions", host("42"), true)
        .await
        .expect("forced on");
    assert!(
        Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions"),
        "an override must beat even the kill switch"
    );
    assert!(
        !Flags::load_on(&db, host("43"))
            .await
            .unwrap()
            .active("occasions"),
        "and must not reach anybody else"
    );

    store::set_enabled(&db, "occasions", true)
        .await
        .expect("on");
    store::set_override(&db, "occasions", host("42"), false)
        .await
        .expect("forced off");
    assert!(
        !Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );
    assert!(
        Flags::load_on(&db, host("43"))
            .await
            .unwrap()
            .active("occasions")
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn setting_an_override_twice_leaves_one_row_and_the_second_answer() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");

    store::set_override(&db, "occasions", host("42"), true)
        .await
        .expect("first");
    store::set_override(&db, "occasions", host("42"), false)
        .await
        .expect("second");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_override(&db, "occasions", host("42"), true)
        .await
        .expect("first");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_enabled(&db, "occasions", true)
        .await
        .expect("on");
    store::set_override(&db, "occasions", host("42"), false)
        .await
        .expect("forced off");

    assert!(
        !Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );

    let removed = store::clear_override(&db, "occasions", host("42"))
        .await
        .expect("cleared");
    assert_eq!(removed, 1);
    assert!(
        Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn deleting_a_flag_takes_its_overrides_with_it() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_override(&db, "occasions", host("42"), true)
        .await
        .expect("forced on");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_override(&db, "occasions", Subject::new("host", "42"), true)
        .await
        .expect("forced on for the host");

    let as_host = Flags::load_on(&db, Subject::new("host", "42"))
        .await
        .unwrap();
    let as_party = Flags::load_on(&db, Subject::new("party", "42"))
        .await
        .unwrap();

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_rollout(&db, "occasions", 30)
        .await
        .expect("rolled out");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_rollout(&db, "occasions", 50)
        .await
        .expect("rolled out");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_rollout(&db, "occasions", 1)
        .await
        .expect("rolled out");
    store::clear_rollout(&db, "occasions")
        .await
        .expect("cleared");

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
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_override(&db, "occasions", &Host { id: 42 }, true)
        .await
        .expect("forced on");

    assert!(
        Flags::load_on(&db, &Host { id: 42 })
            .await
            .unwrap()
            .active("occasions")
    );
    assert!(
        !Flags::load_on(&db, &Host { id: 43 })
            .await
            .unwrap()
            .active("occasions")
    );
}

// ---------------------------------------------------------------------------
// What the code review of 15 September 2026 found, pinned so it cannot return.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_percentage_that_is_not_a_percentage_is_refused_rather_than_clamped() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");

    let refused = store::set_rollout(&db, "occasions", 200).await;
    assert!(
        matches!(refused, Err(loco_flags::FlagError::NotAPercentage(200))),
        "clamping turned a mistyped 1000 into `release it to everybody`, which is the one \
         direction where corruption reads as an instruction"
    );

    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(!flags.active("occasions"), "and nothing was written");
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_database_refuses_an_impossible_percentage_too() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");

    let refused = db
        .execute_unprepared(
            "UPDATE feature_flags SET rollout_percent = 1000 WHERE key = 'occasions'",
        )
        .await;

    assert!(
        refused.is_err(),
        "the CHECK constraint is what makes the clamp unreachable rather than merely unlikely"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_rollout_can_be_taken_off_again() {
    let db = fresh().await;
    store::create(&db, "occasions", None)
        .await
        .expect("created");
    store::set_rollout(&db, "occasions", 50)
        .await
        .expect("rolled out");

    // Before the fix this was a one-way door: `flag:on` kept the percentage, `flag:off` answered
    // false without removing it, and only deleting the flag cleared it, taking every exception
    // with it. Meanwhile any `load_global` reader answered false for ever.
    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(matches!(
        flags.try_active("occasions"),
        Err(loco_flags::FlagError::RolloutWithoutScope(_))
    ));

    store::clear_rollout(&db, "occasions")
        .await
        .expect("cleared");
    let flags = Flags::load_global_on(&db).await.expect("loading works");
    assert!(
        flags.active("occasions"),
        "and the global reader can answer again"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn clearing_an_exception_on_a_key_nobody_created_says_so() {
    let db = fresh().await;

    let refused = store::clear_override(&db, "typo", host("42")).await;
    assert!(
        matches!(refused, Err(loco_flags::FlagError::UnknownFlag(key)) if key == "typo"),
        "this was the one write that skipped the check, so a mistyped key reported the \
         honest-looking `nothing changed`"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_migration_refuses_a_database_that_already_has_a_table_of_that_name() {
    let db = fresh().await;

    // A consumer's own flags table, which is the most likely collision there is: an application
    // with one is exactly the application that reaches for this crate.
    OnlyTheFlags::down(&db, None).await.expect("down works");
    db.execute_unprepared(
        "CREATE TABLE feature_flags (key varchar(64) PRIMARY KEY, \"on\" boolean, note text)",
    )
    .await
    .expect("the consumer's own table");

    let refused = OnlyTheFlags::up(&db, None).await;
    assert!(
        refused.is_err(),
        "with `if_not_exists` this passed, recorded itself as applied, and then failed on every \
         request with `column feature_flags.enabled does not exist`"
    );

    // And their table is untouched, which is the half that mattered most: `down` used to drop it.
    let survived = db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT count(*) AS columns FROM information_schema.columns \
             WHERE table_name = 'feature_flags' AND column_name = 'note'",
        ))
        .await
        .expect("reading works");
    assert!(
        survived.is_some(),
        "the consumer's own column is still there"
    );

    db.execute_unprepared("DROP TABLE feature_flags").await.ok();
}

// ---------------------------------------------------------------------------
// The operator's own interface. Seven tasks and a health check, none of which
// any test touched until the review pointed at them.
// ---------------------------------------------------------------------------

/// A real `AppContext`, which is the only thing a `Task` will accept.
///
/// `AppContext` is `#[non_exhaustive]`, so it cannot be built with a struct literal from out here.
/// loco's own testing helper hands one out with a dummy connection; `db` is public, so the real
/// connection goes in afterwards.
async fn context_on(db: DatabaseConnection) -> loco_rs::app::AppContext {
    let mut ctx = loco_rs::tests_cfg::app::get_app_context().await;
    ctx.db = db;
    ctx
}

fn vars(pairs: &[(&str, &str)]) -> loco_rs::task::Vars {
    loco_rs::task::Vars::from_cli_args(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn every_task_is_registered_under_the_name_the_readme_promises() {
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    let mut names = tasks.names();
    names.sort();
    assert_eq!(
        names,
        vec![
            "flag:create",
            "flag:delete",
            "flag:group",
            "flag:list",
            "flag:off",
            "flag:on",
            "flag:override",
            "flag:rollout",
        ],
        "a task renamed without the README following is a command that silently does not exist"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_tasks_drive_a_flag_from_nothing_to_a_rollout_and_back() {
    let db = fresh().await;
    let ctx = context_on(db.clone()).await;
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    // The whole operator story, through the registry rather than by calling `store` directly.
    tasks
        .run(
            &ctx,
            "flag:create",
            &vars(&[("key", "occasions"), ("description", "the new list")]),
        )
        .await
        .expect("created");
    assert!(
        !Flags::load_global_on(&db)
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(&ctx, "flag:on", &vars(&[("key", "occasions")]))
        .await
        .expect("on");
    assert!(
        Flags::load_global_on(&db)
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(
            &ctx,
            "flag:rollout",
            &vars(&[("key", "occasions"), ("pct", "100")]),
        )
        .await
        .expect("rolled out");
    assert!(
        Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(&ctx, "flag:off", &vars(&[("key", "occasions")]))
        .await
        .expect("off");
    assert!(
        !Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(
            &ctx,
            "flag:rollout",
            &vars(&[("key", "occasions"), ("pct", "clear")]),
        )
        .await
        .expect("rollout cleared");
    assert!(
        !Flags::load_global_on(&db)
            .await
            .unwrap()
            .active("occasions"),
        "clearing a rollout must not resurrect a flag that was killed: `flag:off` came before this, \
         and only `flag:on` undoes it"
    );

    // Switched back on, with no percentage left, being on is the whole answer again, which is the
    // state a reader with no subject can answer for.
    tasks
        .run(&ctx, "flag:on", &vars(&[("key", "occasions")]))
        .await
        .expect("on again");
    assert!(
        Flags::load_global_on(&db)
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(&ctx, "flag:list", &vars(&[]))
        .await
        .expect("list runs");
    tasks
        .run(&ctx, "flag:delete", &vars(&[("key", "occasions")]))
        .await
        .expect("deleted");
    assert!(Flags::load_global_on(&db).await.unwrap().known().is_empty());
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_override_task_decides_and_then_forgets() {
    let db = fresh().await;
    let ctx = context_on(db.clone()).await;
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    tasks
        .run(&ctx, "flag:create", &vars(&[("key", "occasions")]))
        .await
        .expect("created");

    tasks
        .run(
            &ctx,
            "flag:override",
            &vars(&[("key", "occasions"), ("scope", "host:42"), ("value", "on")]),
        )
        .await
        .expect("forced on");
    assert!(
        Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );

    tasks
        .run(
            &ctx,
            "flag:override",
            &vars(&[
                ("key", "occasions"),
                ("scope", "host:42"),
                ("value", "clear"),
            ]),
        )
        .await
        .expect("cleared");
    assert!(
        !Flags::load_on(&db, host("42"))
            .await
            .unwrap()
            .active("occasions")
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_task_pointed_at_a_key_nobody_created_refuses() {
    let db = fresh().await;
    let ctx = context_on(db.clone()).await;
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    for task in ["flag:on", "flag:off", "flag:delete"] {
        assert!(
            tasks
                .run(&ctx, task, &vars(&[("key", "typo")]))
                .await
                .is_err(),
            "{task} against an unknown key must refuse rather than create it"
        );
    }
    assert!(Flags::load_global_on(&db).await.unwrap().known().is_empty());
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn a_task_missing_its_argument_says_which_one() {
    let db = fresh().await;
    let ctx = context_on(db).await;
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    let refused = tasks
        .run(&ctx, "flag:create", &vars(&[]))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("key"),
        "the message must name the missing argument, and said: {refused}"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_doctor_reports_the_flags_when_the_tables_are_there() {
    use loco_rs::app::Initializer as _;

    let db = fresh().await;
    store::create(&db, "paywall", None).await.expect("created");
    store::set_enabled(&db, "paywall", true).await.expect("on");
    let ctx = context_on(db).await;

    let reported = loco_flags::Initializer
        .check(&ctx)
        .await
        .expect("the check runs")
        .expect("and has something to say");

    assert!(matches!(reported.status, loco_rs::doctor::CheckStatus::Ok));
    assert!(
        reported.message.contains("1 defined") && reported.message.contains("1 switched on"),
        "the doctor line must count what is there, and said: {}",
        reported.message
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_doctor_says_so_when_the_migration_was_never_registered() {
    use loco_rs::app::Initializer as _;

    let db = fresh().await;
    // Exactly the state of an application that wired up the initializer and forgot the migration,
    // which is the mistake this check exists to catch.
    OnlyTheFlags::down(&db, None).await.expect("down works");
    let ctx = context_on(db).await;

    let reported = loco_flags::Initializer
        .check(&ctx)
        .await
        .expect("the check runs")
        .expect("and has something to say");

    assert!(matches!(
        reported.status,
        loco_rs::doctor::CheckStatus::NotOk
    ));
    assert!(
        reported
            .description
            .unwrap_or_default()
            .contains("CreateFeatureFlags"),
        "the failure must name the migration to add, or it is just a red line"
    );
}

// ---------------------------------------------------------------------------
// A shared audience: several switches on one feature, one set of people.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn flags_in_one_group_reach_the_same_people_through_the_database() {
    let db = fresh().await;

    // Three switches on one imaginary checkout, each able to be killed on its own.
    for key in ["checkout_button", "checkout_summary", "checkout_receipt"] {
        store::create(&db, key, None).await.expect("created");
        store::set_bucket_group(&db, key, Some("checkout"))
            .await
            .expect("one audience");
        store::set_rollout(&db, key, 30)
            .await
            .expect("a third of them");
    }

    let mut together = 0;
    for subject in 0..300 {
        let flags = Flags::load_on(&db, host(&subject.to_string()))
            .await
            .expect("loading works");

        let button = flags.active("checkout_button");
        assert_eq!(
            button,
            flags.active("checkout_summary"),
            "host {subject} got half a checkout"
        );
        assert_eq!(
            button,
            flags.active("checkout_receipt"),
            "host {subject} got two thirds of a checkout"
        );
        if button {
            together += 1;
        }
    }

    assert!(
        (60..=120).contains(&together),
        "a third of three hundred landed at {together}, which is not a third"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn each_switch_in_a_group_still_kills_on_its_own() {
    let db = fresh().await;

    for key in ["checkout_button", "checkout_summary"] {
        store::create(&db, key, None).await.expect("created");
        store::set_bucket_group(&db, key, Some("checkout"))
            .await
            .expect("one audience");
        store::set_rollout(&db, key, 100).await.expect("everybody");
    }

    store::set_enabled(&db, "checkout_button", false)
        .await
        .expect("one of them killed");

    let flags = Flags::load_on(&db, host("42"))
        .await
        .expect("loading works");
    assert!(!flags.active("checkout_button"), "the killed one is off");
    assert!(
        flags.active("checkout_summary"),
        "and the other one is untouched, which is the reason they are separate flags at all"
    );
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn clearing_a_group_returns_the_flag_to_its_own_audience() {
    let db = fresh().await;

    store::create(&db, "checkout_button", None)
        .await
        .expect("created");
    store::set_bucket_group(&db, "checkout_button", Some("checkout"))
        .await
        .expect("grouped");
    store::set_rollout(&db, "checkout_button", 50)
        .await
        .expect("half");

    let grouped: Vec<bool> = (0..200)
        .map(|subject| loco_flags::bucket::is_inside("checkout", "host", &subject.to_string(), 50))
        .collect();

    store::set_bucket_group(&db, "checkout_button", None)
        .await
        .expect("ungrouped");

    let alone: Vec<bool> = (0..200)
        .map(|subject| {
            loco_flags::bucket::is_inside("checkout_button", "host", &subject.to_string(), 50)
        })
        .collect();

    assert_ne!(
        grouped, alone,
        "clearing a group must change which people are inside, or the group never meant anything"
    );

    // And the database agrees with the pure function about which of the two it is now using.
    for subject in 0..50 {
        let id = subject.to_string();
        assert_eq!(
            Flags::load_on(&db, host(&id))
                .await
                .unwrap()
                .active("checkout_button"),
            loco_flags::bucket::is_inside("checkout_button", "host", &id, 50)
        );
    }
}

#[tokio::test]
#[ignore = "needs a Postgres: see the module comment"]
async fn the_group_task_sets_and_clears_it() {
    let db = fresh().await;
    let ctx = context_on(db.clone()).await;
    let mut tasks = loco_rs::task::Tasks::default();
    loco_flags::register_tasks(&mut tasks);

    tasks
        .run(&ctx, "flag:create", &vars(&[("key", "checkout_button")]))
        .await
        .expect("created");

    tasks
        .run(
            &ctx,
            "flag:group",
            &vars(&[("key", "checkout_button"), ("group", "checkout")]),
        )
        .await
        .expect("grouped");
    assert_eq!(
        store::find(&db, "checkout_button")
            .await
            .unwrap()
            .unwrap()
            .bucket_group
            .as_deref(),
        Some("checkout")
    );

    tasks
        .run(
            &ctx,
            "flag:group",
            &vars(&[("key", "checkout_button"), ("group", "clear")]),
        )
        .await
        .expect("ungrouped");
    assert!(
        store::find(&db, "checkout_button")
            .await
            .unwrap()
            .unwrap()
            .bucket_group
            .is_none()
    );
}
