# The design of loco-flags

Runtime feature flags for [loco.rs](https://loco.rs), kept in the database. This document is the
design of the crate: what it does, how it reaches an answer, and why each decision was taken,
including the alternatives that were rejected. It is kept alive in the same change as the code.

**Where a decision lives.** In this document, in the section it belongs to. A decision gets its own
file in [`decisions/`](decisions/) when it outgrows a paragraph here: when the alternatives need
arguing out, when it will be re-litigated, or when it is superseded later and both versions have to
stay readable. Nothing is decided in a commit message or in a pull request thread alone.

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

Two tables. The obvious design has three, and the third holds a resolved answer per subject. That
table is only needed when a flag can be a rule in code: a rule may answer differently on every call,
so its answers have to be frozen the first time they are given, and then invalidated, purged and
reasoned about. There are no rules here, so an answer is arithmetic over the flag and the subject.
It is recomputed on every read, it agrees with itself for ever, and a read is not secretly a write.

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

**The migration refuses a database that already has a `feature_flags` table.** The polite
alternative is `if_not_exists`, and it is the dangerous one. It skips the table that is already
there, creates everything around it, and records itself as applied. Every request then fails on
`column feature_flags.enabled does not exist`, with nothing pointing back at the cause, and `down`
would drop a table this crate never created. Refusing names the collision while renaming one of the
two tables is still cheap.

**Both index names carry their table.** A Postgres index name is schema-wide, so two crates that
each create an index called `idx_key` cannot share a database. The second migration to run is the
one that fails, which means it fails on somebody else's deployment and never on ours.

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
one unlucky tenth of an application's users meeting every experiment it ever runs. A group is how to
ask for the opposite, for the feature that is genuinely three switches: each wants its own kill
switch, and all three have to reach the same people. Without a group the only way to keep them in
step is to make them one flag, which costs the ability to kill them one at a time.

A rollout percentage asked without a subject is refused with a typed error rather than computed.
"Twenty five percent of requests" makes the answer flicker under one reader, which is not what
anybody means by a rollout.

## The bucket

```
bucket = first 8 bytes of sha256(len+audience, len+scope_type, len+scope_id) as u64, modulo 100
```

**Each part is length-prefixed rather than joined with a separator.** Joining on `:` is ambiguous:
`("occasions", "host", "7:9")` and `("occasions", "host:7", "9")` produce one digest and therefore
one bucket, so an application whose identifiers contain colons, which a urn or a prefixed uuid does,
would have two different subjects sharing one answer.

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

**`scope_type` returns `&'static str`, and a borrowed `&str` was rejected.** A consumer implements
this method with a literal, and clippy's `unnecessary_literal_bound` fires on every such
implementation when the trait returns a borrowed `&str`. Pushing a lint into everybody else's crate
to save one lifetime here is the wrong trade. A kind chosen at runtime never touches the trait: it
goes to `load_for` and to the `_for` functions in `store`, which take the pair of strings directly,
and `Subject::new(kind, id)` names a subject with no type to hang the trait on.

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
- **`flag:rollout … pct:clear` is the way back off a rollout.** Without it a percentage is a one-way
  door: `flag:on` keeps it, `flag:off` answers false without removing it, and the only other way out
  is deleting the flag, which cascades away every exception anybody wrote.

**A percentage above a hundred is refused and never clamped.** Clamping turns a mistyped `1000` into
"release it to everybody", quietly, and it disagrees with the task, which refuses the same number.
Two ways in with opposite answers, and the quiet one is the dangerous one.

**`set_override` runs in a transaction.** It is written as delete-then-insert rather than an upsert,
so the behaviour is the same on every backend sea-orm supports and the unique index stays the thing
that holds the invariant. Between the delete and the insert the subject has no decision at all, so a
failure in that gap, or a reader arriving in it, falls back to whatever the flag says. For a subject
deliberately excluded from something that is sold, that gap is the feature being given away.

**Every write goes through `require`**, so `flag:on key:typo` says no flag is called `typo` and
changes nothing. Creating what was asked for would turn a misspelling into a second flag that the
code never asks about and nobody ever finds.

## Wiring it into an application

Three lines, and no fork of loco. `AppContext.shared_store` exists for exactly this.

```rust
fn initializers(..) -> Vec<Box<dyn Initializer>> { vec![Box::new(loco_flags::Initializer)] }
fn register_tasks(tasks: &mut Tasks)             { loco_flags::register_tasks(tasks); }
// migration/src/lib.rs
Box::new(loco_flags::migrations::CreateFeatureFlags),
```

**The migration is named by hand rather than by `DeriveMigrationName`.** That derive takes its name
from the module path, so it would write this crate's path into a consumer's migration history, and
the recorded name would change whenever this crate moved a module.

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

**Each test connects with a pool of two.** sea-orm's default is a hundred connections per pool, and
a file that opens one pool per test exhausts Postgres and takes the container down with it. A test
that needs a second connection needs it for a transaction, not for throughput.

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

## Requires

loco 1.1, sea-orm 2.0, Rust 1.94.
