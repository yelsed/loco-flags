# The design of loco-flags

Runtime feature flags for [loco.rs](https://loco.rs), kept in the database. This document is the
design of the whole crate and it describes what is built, not what was once planned. It is kept
alive in the same change as the code.

**Where a decision lives.** The hard decisions are in this document, each with the reason it was
taken and the alternatives that were rejected. A decision that outgrows a paragraph gets its own
file in [`decisions/`](decisions/), numbered and dated, and this document links to it from the
section it belongs to. Nothing is decided in a commit message or in a pull request thread alone.

**What changed and when** is at the bottom, under [History](#history), so the body of the document
can describe the crate rather than its own past.

## Why this exists

Nothing in the loco ecosystem does runtime feature flags. Checked on 14 September 2026: crates.io
holds exactly five `loco-*` crates (`loco-rs`, `loco-gen`, `loco-cli`, `loco-openapi`,
`loco-oauth2`) and a GitHub search for loco feature flags returns zero repositories. Rust as a whole
has client SDKs for hosted services (LaunchDarkly, Unleash, Flagsmith, ConfigCat, GrowthBook,
FeatBit) and bare evaluation libraries (`open-feature`, `liteflags-rs`), and nothing that a web
framework carries as part of itself. Loco's own "feature flags" are Cargo features, which are
decided when you compile.

## What a flag is here

The database is the truth. Code knows a flag's name and nothing else. Turning one on, off, or up to
a percentage is a task or, later, a button, and never a deploy.

There are no rules in code, no closures, no definition step. That is the single largest decision in
this design and it is what keeps version 1 small.

## Data model

Two tables, where the obvious design has three. The third would hold a resolved value per subject,
and it is only needed when a flag can be defined as a rule in code: such a rule may answer
differently on every call, so its answers have to be frozen the moment they are first given. Without
rules the answer is computable, so it is computed rather than remembered. That removes a table,
removes the purge task it would need, and keeps a read from being a write.

```
feature_flags
  key               text primary key
  enabled           boolean not null default false
  rollout_percent   smallint null          -- null means "not a rollout", CHECK between 0 and 100
  bucket_group      text null              -- null means "draw the audience from the key"
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

**The migration refuses a database that already has a `feature_flags` table**, rather than adopting
it with `if_not_exists`. Adoption reads as politeness and is data loss waiting to happen: a table
with the same name and a different shape would be silently accepted, and `down` would then drop a
table this crate never created. Refusing names the collision while it is still cheap to rename one
of the two.

**Both index names carry their table**, because a Postgres index name is schema-wide while a table
name is not. `idx_key` from two crates in one database is a migration that fails on somebody else's
deployment and not on ours.

## Resolution, in one function

```
1. an override row for this subject        -> that row wins
2. enabled = false                         -> off      (the kill switch beats the rollout)
3. rollout_percent is null                 -> enabled
4. otherwise                               -> bucket(audience, subject) < rollout_percent
```

Step 2 is what makes the toggle worth having during an incident. Off means off, and not "off for the
ninety percent who were not chosen".

The audience in step 4 is `bucket_group` when the flag has one and the flag's own key when it does
not. Two flags at ten percent therefore reach two different tenths by default, which is what stops
one unlucky tenth of an application's users meeting every experiment it ever runs. A group is the
way to ask for the opposite, and it exists for the case the first draft had no answer for: one
feature that is genuinely three switches, each wanting its own kill switch, all of which have to
reach the same people. Without it the only way to keep three switches in step was to make them one
flag, which costs the ability to kill them one at a time.

A rollout percentage asked without a subject is refused with a typed error rather than computed.
"Twenty five percent of requests" makes the answer flicker under one reader, which is not what
anybody means by a rollout.

## The bucket

```
bucket = first 8 bytes of sha256(len+audience, len+scope_type, len+scope_id) as u64, modulo 100
```

**Each part is length-prefixed rather than joined with a separator.** Joining on `:` was ambiguous:
`("occasions", "host", "7:9")` and `("occasions", "host:7", "9")` produced the same digest and
therefore the same bucket, and an application whose identifiers contain colons, which a urn or a
prefixed uuid does, would have had two different subjects sharing one answer.

Sha2 is already in loco's dependency tree, so this adds no dependency. It is used rather than
`std::collections::hash_map::DefaultHasher` because that one carries no stability guarantee across
Rust releases, and a rollout that reshuffles when the compiler is upgraded silently moves people in
and out of a feature and reports nothing.

Raising a rollout from 25 to 50 keeps everyone who was already inside, inside. Lowering it puts some
back out, which is the honest behaviour for a number that means "this fraction". A percentage is a
fraction of everyone who asks and never a headcount, because a headcount needs a counter and every
process would count differently.

## The API

```rust
let flags = Flags::load(&ctx, &host).await?;   // two queries, once per request

if flags.active("occasions") { /* ... */ }     // synchronous, returns bool
if flags.active("paywall")   { /* ... */ }     // a global flag answers the same call
```

Everything is loaded up front so that no `.await?` appears between the lines of business logic that
ask about it.

| Call | For |
|---|---|
| `Flags::load(&ctx, subject)` | a request, with an `AppContext` |
| `Flags::load_on(&db, subject)` | a worker, a task or a test, with no application around it |
| `Flags::load_for(&db, kind, id)` | a subject whose kind is only known at runtime |
| `Flags::load_global(&ctx)`, `load_global_on(&db)` | no subject at all |
| `flags.active(key)` | the answer, as a `bool` |
| `flags.try_active(key)` | the same answer, with `UnknownFlag` and `RolloutWithoutScope` reachable |
| `Flags::fixed(["a", "b"])` | a set of answers for a test, with no database |
| `flags.is_known(key)`, `flags.known()` | what was loaded |

An application makes its own types into subjects:

```rust
impl FlagScope for Host {
    fn scope_type(&self) -> &'static str { "host" }
    fn scope_id(&self) -> String { self.id.to_string() }
}
```

**`scope_type` returns `&'static str` and not `&str`**, which was tried and reverted. A consumer
implements this with a literal, and clippy's `unnecessary_literal_bound` fires on every such
implementation when the trait returns a borrowed `&str`. Pushing a lint into everybody else's crate
to save one lifetime is the wrong trade, so the runtime-shaped calls, `load_for` and the `_for`
functions in `store`, take the pair of strings directly instead. `Subject::new(kind, id)` names a
subject without a type to hang the trait on.

No application's type appears in this crate, and no type from this crate appears in an application's
tables. It is meant to be published.

## Turning things on and off

```sh
cargo loco task flag:list
cargo loco task flag:create key:paywall description:"kill switch for taking money"
cargo loco task flag:on key:paywall
cargo loco task flag:off key:paywall
cargo loco task flag:rollout key:occasions pct:10
cargo loco task flag:rollout key:occasions pct:clear
cargo loco task flag:group key:checkout_button group:checkout
cargo loco task flag:override key:occasions scope:host:42 value:on
cargo loco task flag:override key:occasions scope:host:42 value:clear
cargo loco task flag:delete key:occasions
```

**Every write lives in `store`** and the tasks are a thin skin over it: parse arguments, call one
function, print a sentence. An admin screen sits on exactly the same functions, which is what keeps
"there is no admin screen yet" from being a decision that has to be unmade later.

Three of those writes carry a rule worth stating here:

- **`flag:create` makes a flag that is off**, always, and off is not a parameter. A flag that could
  be born on would make adding one a release of whatever it guards.
- **`flag:rollout` switches the flag on**, because a rollout on a killed flag reaches nobody and
  setting a percentage is nobody's way of saying "still off". A flag killed during an incident
  therefore comes back when somebody sets a percentage on it, and `flag:off` afterwards means what
  it says.
- **`flag:rollout … pct:clear` exists** because a rollout was otherwise a one-way door: `flag:on`
  keeps it, `flag:off` answers false without removing it, and only `flag:delete` cleared it, by
  deleting the flag and cascading away every exception anybody had written.

**A percentage above a hundred is refused and never clamped.** Clamping turned a mistyped `1000`
into "release it to everybody", quietly, and it disagreed with the task, which refused the same
number. Two ways in with opposite answers, and the quiet one was the dangerous one.

**`set_override` runs in a transaction.** It is written as delete-then-insert rather than an upsert,
so the behaviour is the same on every backend sea-orm supports and the unique index stays the thing
that holds the invariant. Between the delete and the insert the subject has no decision at all, so a
failure in that gap, or a reader arriving in it, would fall back to whatever the flag says. For a
subject deliberately excluded from something that is sold, that gap is the feature being given away.

**Every write goes through `require`**, so `flag:on key:typo` says no flag is called `typo` and
changes nothing. Creating what was asked for would turn a misspelling into a second flag that the
code never asks about and nobody ever finds.

## Wiring it into an application

Three lines, and no fork of loco. `AppContext.shared_store` exists for exactly this, and
`AppContext`, `Initializer` and `Task` are word for word identical between 0.16.4 and 1.1.0, so the
seams are stable across that upgrade.

```rust
fn initializers(..) -> Vec<Box<dyn Initializer>> { vec![Box::new(loco_flags::Initializer)] }
fn register_tasks(tasks: &mut Tasks)             { loco_flags::register_tasks(tasks); }
// migration/src/lib.rs
Box::new(loco_flags::migrations::CreateFeatureFlags),
```

**The migration is named by hand rather than by `DeriveMigrationName`.** That derive takes its name
from the module path, so it would have written this crate's path into a consumer's migration
history, and the recorded name would change whenever this crate moved a module.

## Errors

`FlagError` via `thiserror`, and `#[non_exhaustive]`, so adding a variant is not a breaking change
for anybody matching on it:

| Variant | Means |
|---|---|
| `UnknownFlag(key)` | a write against a key nobody created |
| `RolloutWithoutScope(key)` | a percentage rollout asked without a subject |
| `NotAPercentage(value)` | a rollout above a hundred |
| `Db(..)` | the database would not answer |

`From<FlagError> for loco_rs::Error` maps the caller's own mistakes onto `BadRequest` and lets a
database failure stay a database failure. **There is no catch-all arm**, so a variant added later
has to be given a status deliberately rather than inheriting one that happened to be at the bottom
of the match.

`active()` returns `bool` and never a `Result`, because a handler asking whether a feature is on has
nothing useful to do with a failure. A name that has no row answers `false` and writes a warning to
the log. A name is a string in version 1, so a typo is silently false, and that log line is the only
thing standing between it and an hour of confusion. Closing that gap properly is the compile-time
registry listed under what version 1 leaves out. `try_active` is the same answer for a caller that
does want to handle it.

`UnknownFlag` belongs to the writes, not to evaluation. Asking about a flag that is not there is a
question with an answer; writing to one is a mistake.

`rollout_percent` is read defensively as well as constrained: a value outside nought to a hundred,
which can only come from a schema older than the `CHECK`, is treated as no rollout rather than
clamped up to a hundred. The two directions of that mistake are not symmetric.

## Testing

A real Postgres, the way loco's own suite works: 23 unit tests and 33 against a database.

The tests that matter are about the bucket: that ten thousand subjects land within tolerance of the
requested percentage, that raising the percentage never moves anybody out, that two flags at the
same percentage reach different people, and that two flags sharing a group reach the same people.

**Each test connects with a pool of two.** sea-orm's default is a hundred per pool, and one test
file opening a pool per test exhausted Postgres and took the container down with it. A test that
needs a second connection needs it for a transaction, not for throughput.

## Deliberately not in version 1

These are seams, not oversights. The next version is meant to turn each into a real problem, and a
later session must not quietly close them off.

- **Two queries per request.** No process-wide cache and no invalidation. Making N processes agree
  on a flag's value within a bounded time, with no query in the request path, is the hard problem
  and it is being kept.
- **Flag names are strings.** A macro that knows the set of flags at compile time, refuses an
  unknown name, and finds dead flags in the crate graph is the other hard problem.
- **Booleans only.** No variants, so no three-way test.
- **No admin screen.** Tasks first. A button in an application's own panel sits on the same `store`
  functions the tasks do.

## Target versions

loco 1.1, sea-orm 2.0, Rust 1.94. Building against 0.16 was considered and rejected: a crate
published in late 2026 that only supports a version from October 2025 starts with debt, and Fuyf has
to make that jump anyway.

## History

**14 September 2026, the design.** Written before any code, as the document this one grew out of.
The shape it settled on, two tables, no rules in code, a computed bucket, is the shape that was
built.

**15 September 2026, the build.** Six things arrived that the design had not named, all of them
additive: `load_on` and `load_global_on` for a caller with no `AppContext`; `try_active`, so the
typed errors are reachable and not only loggable; `Subject`, for a caller with no type to hang
`FlagScope` on; `store`, so every write is in one place; the hand-written migration name; and
`flag:override … value:clear`.

**15 September 2026, the code review.** Eight changes, and two of them were data-loss class: the
migration adopting a table it did not create, and `down` then dropping it. The rest: the
length-prefixed digest, the `CHECK` on `rollout_percent` and the defensive read beside it,
`pct:clear`, `FlagScope::scope_type` briefly returning `&str` and being reverted, `FlagError`
becoming `#[non_exhaustive]` and gaining `NotAPercentage`, `set_override` gaining its transaction
and `clear_override` gaining the key check, and the two index names gaining their table.

**15 September 2026, the shared audience.** `bucket_group` and `flag:group`, asked for by the owner
after reading how the bucket works, for the feature that is genuinely three switches.

**17 September 2026, this document.** Renamed from `0001-design.md`, which read as the first of a
numbered series when it is the one global design, and rewritten to describe the crate rather than
the distance between the crate and its first draft. `decisions/` is where a decision goes that
outgrows a paragraph here.
