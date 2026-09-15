//! Switching flags from a terminal.
//!
//! ```sh
//! cargo loco task flag:list
//! cargo loco task flag:create key:paywall description:"kill switch for taking money"
//! cargo loco task flag:on key:paywall
//! cargo loco task flag:off key:paywall
//! cargo loco task flag:rollout key:occasions pct:10
//! cargo loco task flag:override key:occasions scope:host:42 value:on
//! cargo loco task flag:delete key:occasions
//! ```
//!
//! Tasks before a screen, because a task needs no design, works on a deployment with a broken
//! front end, and is the thing an incident reaches for. A button in an application's own panel sits
//! on [`crate::store`], which is the same place these sit.

use loco_rs::{
    app::AppContext,
    task::{Task, TaskInfo, Tasks, Vars},
    Error, Result,
};

use crate::{scope::Subject, store};

/// Add every flag task to an application's registry.
///
/// ```ignore
/// fn register_tasks(tasks: &mut Tasks) {
///     loco_flags::register_tasks(tasks);
/// }
/// ```
pub fn register(tasks: &mut Tasks) {
    tasks.register(List);
    tasks.register(Create);
    tasks.register(On);
    tasks.register(Off);
    tasks.register(Rollout);
    tasks.register(Override);
    tasks.register(Delete);
}

/// One required argument, or a message naming what is missing.
fn required<'vars>(vars: &'vars Vars, name: &str) -> Result<&'vars str> {
    vars.cli_arg(name)
        .map_err(|_| Error::Message(format!("this task needs `{name}:…`")))
}

/// `host:42` into a subject.
///
/// Split on the **first** colon, so an identifier may contain one: a slug, a uuid with a prefix, or
/// a tenant name are all likelier than not to.
fn subject(argument: &str) -> Result<Subject> {
    let (kind, id) = argument.split_once(':').ok_or_else(|| {
        Error::Message(format!(
            "a scope looks like `host:42`, and `{argument}` has no colon in it"
        ))
    })?;

    if kind.is_empty() || id.is_empty() {
        return Err(Error::Message(format!(
            "a scope looks like `host:42`, and `{argument}` is missing one of the two halves"
        )));
    }

    // Leaked so the kind can be the `&'static str` the trait asks for. One leak per task run, in a
    // process that exits immediately afterwards; the alternative is making the trait own a String
    // for the benefit of every application, to suit one caller here.
    Ok(Subject::new(
        Box::leak(kind.to_owned().into_boxed_str()),
        id,
    ))
}

/// `on`, `off`, `true`, `false`, `yes`, `no`, `1`, `0`.
fn onoff(argument: &str) -> Result<bool> {
    match argument.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        other => Err(Error::Message(format!(
            "`{other}` is not a value: say `on` or `off`"
        ))),
    }
}

/// Every flag, how it is set, and who has an exception.
pub struct List;

#[async_trait::async_trait]
impl Task for List {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:list".to_string(),
            detail: "Every feature flag, how it is set, and who has an exception".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        let flags = store::list(&ctx.db).await?;

        if flags.is_empty() {
            println!("No feature flags yet. `flag:create key:…` makes one.");
            return Ok(());
        }

        for flag in flags {
            let state = match (flag.enabled, flag.rollout_percent) {
                (false, _) => "off".to_string(),
                (true, None) => "on".to_string(),
                (true, Some(percent)) => format!("on, rolling out to {percent}%"),
            };
            println!("{:<28} {state}", flag.key);

            if let Some(description) = &flag.description {
                println!("{:<28}   {description}", "");
            }

            for decision in store::overrides_of(&ctx.db, &flag.key).await? {
                let verdict = if decision.enabled { "on" } else { "off" };
                println!(
                    "{:<28}   {}:{} forced {verdict}",
                    "", decision.scope_type, decision.scope_id
                );
            }
        }

        Ok(())
    }
}

/// Create a flag, switched off.
pub struct Create;

#[async_trait::async_trait]
impl Task for Create {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:create".to_string(),
            detail: "Create a flag, switched off: key:… [description:…]".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        let description = vars.cli_arg("description").ok().map(ToOwned::to_owned);

        store::create(&ctx.db, key, description).await?;
        println!("Created `{key}`, switched off. `flag:on key:{key}` turns it on.");
        Ok(())
    }
}

/// Switch a flag on, keeping any rollout.
pub struct On;

#[async_trait::async_trait]
impl Task for On {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:on".to_string(),
            detail: "Switch a flag on, keeping any rollout: key:…".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        let flag = store::set_enabled(&ctx.db, key, true).await?;

        match flag.rollout_percent {
            Some(percent) => println!("`{key}` is on, reaching {percent}% of subjects."),
            None => println!("`{key}` is on, for everybody."),
        }
        Ok(())
    }
}

/// Switch a flag off for everybody, whatever its rollout says.
pub struct Off;

#[async_trait::async_trait]
impl Task for Off {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:off".to_string(),
            detail: "Switch a flag off for everybody, whatever its rollout says: key:…".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        store::set_enabled(&ctx.db, key, false).await?;
        println!("`{key}` is off, for everybody, rollout or not.");
        Ok(())
    }
}

/// Give a flag a percentage, and switch it on.
pub struct Rollout;

#[async_trait::async_trait]
impl Task for Rollout {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:rollout".to_string(),
            detail: "Give a flag a percentage and switch it on: key:… pct:10".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        let raw = required(vars, "pct")?;

        let percent: u8 = raw
            .parse()
            .ok()
            .filter(|percent| *percent <= 100)
            .ok_or_else(|| {
                Error::Message(format!("`{raw}` is not a percentage between 0 and 100"))
            })?;

        store::set_rollout(&ctx.db, key, percent).await?;
        println!(
            "`{key}` is on and reaching {percent}% of subjects. Raising this number never takes \
             the feature away from anybody who already had it."
        );
        Ok(())
    }
}

/// Decide about one subject, whatever the flag says.
pub struct Override;

#[async_trait::async_trait]
impl Task for Override {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:override".to_string(),
            detail: "Decide for one subject: key:… scope:host:42 value:on|off|clear".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        let who = subject(required(vars, "scope")?)?;
        let value = required(vars, "value")?;

        if value.eq_ignore_ascii_case("clear") {
            let removed = store::clear_override(&ctx.db, key, &who).await?;
            if removed == 0 {
                println!("`{key}` had no exception for that subject; nothing changed.");
            } else {
                println!("`{key}` no longer has an exception for that subject.");
            }
            return Ok(());
        }

        let enabled = onoff(value)?;
        let decision = store::set_override(&ctx.db, key, &who, enabled).await?;
        println!(
            "`{key}` is forced {} for {}:{}, whatever the flag says.",
            if enabled { "on" } else { "off" },
            decision.scope_type,
            decision.scope_id
        );
        Ok(())
    }
}

/// Delete a flag and every exception on it.
pub struct Delete;

#[async_trait::async_trait]
impl Task for Delete {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "flag:delete".to_string(),
            detail: "Delete a flag and every exception on it: key:…".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let key = required(vars, "key")?;
        store::delete(&ctx.db, key).await?;
        println!(
            "`{key}` is gone, and so is every exception on it. Any code still asking about it now \
             gets `false` and a warning in the log."
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::FlagScope;

    #[test]
    fn a_scope_splits_on_the_first_colon_so_an_identifier_may_contain_one() {
        let who = subject("host:42").expect("a plain scope parses");
        assert_eq!(who.scope_type(), "host");
        assert_eq!(who.scope_id(), "42");

        let uuid = subject("party:urn:uuid:1234").expect("an identifier may contain colons");
        assert_eq!(uuid.scope_type(), "party");
        assert_eq!(uuid.scope_id(), "urn:uuid:1234");
    }

    #[test]
    fn a_scope_missing_a_half_is_refused_rather_than_guessed() {
        assert!(subject("host").is_err());
        assert!(subject("host:").is_err());
        assert!(subject(":42").is_err());
    }

    #[test]
    fn a_value_is_read_generously_but_not_carelessly() {
        for yes in ["on", "ON", "true", "yes", "1"] {
            assert!(onoff(yes).expect("a yes parses"));
        }
        for no in ["off", "OFF", "false", "no", "0"] {
            assert!(!onoff(no).expect("a no parses"));
        }
        assert!(onoff("maybe").is_err());
    }
}
