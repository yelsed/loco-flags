# loco-flags, version 1

> **Status:** approved 14 September 2026. Not yet built.
> **Blocked by:** Fuyf's upgrade from loco 0.16.4 to 1.1.0, because this crate targets
> loco 1.1 and sea-orm 2.0 and Fuyf is its first consumer.

## Why this exists

Nothing in the loco ecosystem does runtime feature flags. Checked on 14 September 2026:
crates.io holds exactly five `loco-*` crates (`loco-rs`, `loco-gen`, `loco-cli`,
`loco-openapi`, `loco-oauth2`) and a GitHub search for loco feature flags returns zero
repositories. Rust as a whole has client SDKs for hosted services (LaunchDarkly, Unleash,
Flagsmith, ConfigCat, GrowthBook, FeatBit) and bare evaluation libraries (`open-feature`,
`liteflags-rs`), and no framework-integrated equivalent of Laravel Pennant for any Rust
framework. Loco's own "feature flags" are Cargo features, which are compile-time.

Pennant is the model. This is not a port of it: the differences below are deliberate and
each one is written down with its reason.

## What a flag is here

The database is the truth. Code knows a flag's name and nothing else. Turning one on, off,
or up to a percentage is a task or, later, a button, and never a deploy.

There are no rules in code, no closures, no definition step. That is the single largest
departure from Pennant and it is what keeps version 1 small.

## Data model

Two tables. Pennant has a third, holding the resolved value per scope, because a PHP
closure can answer differently on every call and its answers must be frozen. Without
closures the answer is computable, so it is computed rather than remembered. That removes
a table, removes the purge task it would need, and keeps a read from being a write.

```
feature_flags
  key               text primary key
  enabled           boolean not null default false
  rollout_percent   smallint null          -- null means "not a rollout"
  description       text null
  created_at, updated_at

feature_flag_overrides
  flag_key          text not null references feature_flags(key) on delete cascade
  scope_type        text not null
  scope_id          text not null
  enabled           boolean not null
  unique (flag_key, scope_type, scope_id)
```

An override row is an explicit human decision about one subject, not a cached answer.

## Resolution, in one function

```
1. an override row for this scope        -> that row wins
2. enabled = false                        -> off      (the kill switch beats the rollout)
3. rollout_percent is null                -> enabled
4. otherwise                              -> bucket(key, scope) < rollout_percent
```

A rollout percentage asked without a scope is refused with a typed error rather than
computed. "Twenty five percent of requests" makes the answer flicker under one reader,
which is not what anybody means by a rollout.

## The bucket

```
bucket = first 8 bytes of sha256("{key}:{scope_type}:{scope_id}") as u64, modulo 100
```

Sha2 is already in loco's dependency tree, so this adds no dependency. It is used rather
than `std::collections::hash_map::DefaultHasher` because that one carries no stability
guarantee across Rust releases, and a rollout that reshuffles when the compiler is
upgraded silently moves people in and out of a feature.

Raising a rollout from 25 to 50 keeps everyone who was already inside, inside. Lowering it
puts some back out, which is the honest behaviour for a number that means "this fraction".

## The API

```rust
let flags = Flags::load(&ctx, &host).await?;   // two queries, once per request

if flags.active("occasions") { ... }           // synchronous, returns bool
if flags.active("paywall")   { ... }           // a global flag answers the same call
```

Everything is loaded up front so that no `.await?` appears between lines of business
logic. Without a scope:

```rust
let flags = Flags::load_global(&ctx).await?;
```

An application makes its own types into scopes:

```rust
impl FlagScope for Host {
    fn scope_type(&self) -> &'static str { "host" }
    fn scope_id(&self) -> String { self.id.to_string() }
}
```

No application's types appear in this crate. It is meant to be published.

## Turning things on and off

```sh
cargo loco task flag:list
cargo loco task flag:create key:paywall description:"kill switch for taking money"
cargo loco task flag:on key:paywall
cargo loco task flag:off key:paywall
cargo loco task flag:rollout key:occasions pct:10
cargo loco task flag:override key:occasions scope:host:42 value:on
cargo loco task flag:delete key:occasions
```

## Wiring it into an application

Three lines, and no fork of loco. `AppContext.shared_store` exists for exactly this, and
`AppContext`, `Initializer` and `Task` are word for word identical between 0.16.4 and
1.1.0, so the seams are stable across that upgrade.

```rust
fn initializers(..) -> Vec<Box<dyn Initializer>> { vec![Box::new(loco_flags::Initializer)] }
fn register_tasks(tasks: &mut Tasks)             { loco_flags::register_tasks(tasks); }
// migration/src/lib.rs
Box::new(loco_flags::migrations::CreateFeatureFlags),
```

## Errors

`FlagError` via `thiserror`: `RolloutWithoutScope(key)`, `UnknownFlag(key)`, `Db(..)`.

`active()` returns `bool`, never a `Result`, because a handler asking whether a feature is
on has nothing useful to do with a failure. A name that has no row answers `false` and
writes a warning to the log. A name is a string in version 1, so a typo is silently false,
and that log line is the only thing standing between it and an hour of confusion. Closing
that gap properly is the compile-time registry listed under what version 1 leaves out.

`UnknownFlag` belongs to the tasks, not to evaluation: `flag:on` against a key nobody
created says so and changes nothing, rather than creating the row it was not asked to
create.

## Testing

A real Postgres, the way loco's own suite works. The two tests that matter are about the
bucket: that ten thousand scope ids land within tolerance of the requested percentage, and
that raising the percentage never moves anybody out.

## Deliberately not in version 1

These are seams, not oversights. The next version is meant to turn each into a real
problem, and a later session must not quietly close them off.

- **Two queries per request.** No process-wide cache and no invalidation. Making N
  processes agree on a flag's value within a bounded time, with no query in the request
  path, is the hard problem and it is being kept.
- **Flag names are strings.** A macro that knows the set of flags at compile time, refuses
  an unknown name, and finds dead flags in the crate graph is the other hard problem.
- **Booleans only.** No variants, so no three-way A/B/C test.
- **No admin screen.** Tasks first. A button in an application's own panel sits on top of
  the same API.

## Target versions

loco 1.1, sea-orm 2.0. Building against 0.16 was considered and rejected: a crate
published in late 2026 that only supports a version from October 2025 starts with debt,
and Fuyf has to make that jump anyway.
