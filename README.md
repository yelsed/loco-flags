# loco-flags

Runtime feature flags for [loco.rs](https://loco.rs), kept in the database.

Loco's own "feature flags" are Cargo features, decided when you compile. These are decided while
the application is running. Switching one on, off, or up to a percentage is a task or a button, and
never a deploy.

```rust
let flags = Flags::load(&ctx, &host).await?;   // two queries, once per request

if flags.active("occasions") { /* ... */ }     // synchronous, returns bool
if flags.active("paywall")   { /* ... */ }     // a global flag answers the same call
```

Everything is read up front so no `.await?` appears between the lines of business logic that ask
about it.

## Wiring it in

Three lines, and no fork of loco.

```rust
// src/app.rs
async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
    Ok(vec![Box::new(loco_flags::Initializer)])
}

fn register_tasks(tasks: &mut Tasks) {
    loco_flags::register_tasks(tasks);
}
```

```rust
// migration/src/lib.rs
Box::new(loco_flags::migrations::CreateFeatureFlags),
```

Then teach one of your own types to be a subject:

```rust
impl FlagScope for Host {
    fn scope_type(&self) -> &'static str { "host" }
    fn scope_id(&self) -> String { self.id.to_string() }
}
```

No type from your application appears in this crate, and no type from this crate appears in your
tables.

## Switching things

```sh
cargo loco task flag:list
cargo loco task flag:create key:paywall description:"kill switch for taking money"
cargo loco task flag:on key:paywall
cargo loco task flag:off key:paywall
cargo loco task flag:rollout key:occasions pct:10
cargo loco task flag:override key:occasions scope:host:42 value:on
cargo loco task flag:override key:occasions scope:host:42 value:clear
cargo loco task flag:delete key:occasions
```

## How an answer is reached

In this order, and the order is the design:

1. an **override** row for this subject, which is a human decision and beats everything;
2. `enabled = false`, so the **kill switch beats the rollout**;
3. no rollout percentage, so being on is the whole answer;
4. otherwise, whether this subject falls under the percentage.

Step 2 is what makes an operations toggle worth having at three in the morning. Off means off, not
"off for the ninety percent who were not chosen".

## The rollout is computed, not remembered

```
bucket = first 8 bytes of sha256("{key}:{scope_type}:{scope_id}") as u64, modulo 100
```

Three things follow, and all three are tested:

- **Raising a percentage never takes the feature away from anybody who had it.** Lowering it does
  put some back out, which is the honest meaning of a number that says "this fraction".
- **Two flags at ten percent pick different tenths**, because the flag's key is in the digest.
  Without that, the unluckiest tenth of your users would meet every experiment you ever run.
- **The same subject gets the same answer in every process, for ever**, with nothing stored and
  nothing to invalidate.

sha256 rather than `DefaultHasher`, whose output is explicitly allowed to change between Rust
releases. A rollout built on that would reshuffle on a toolchain bump and nothing would report it.

## Not Pennant

[Pennant](https://laravel.com/docs/pennant) is the model, not the target. Pennant lets you define a
flag as a closure, and therefore has to store every resolved answer, because a closure can answer
differently on every call. There are no closures here, so the answer is computed rather than
remembered: one table fewer, no purge task, and a read that is not secretly a write.

## What version 1 deliberately leaves out

Seams, not oversights:

- **No process-wide cache**, so two queries per request. Making N processes agree on a value within
  a bounded time, with no query in the request path, is the hard problem and it is being kept.
- **Flag names are strings**, so a typo answers `false` and writes a warning rather than failing to
  compile. A macro that knows the set of flags at compile time is the other hard problem.
- **Booleans only.** No variants, so no three-way test.
- **No admin screen.** Tasks first. A button sits on the same `store` functions these do.

## Requires

loco 1.1, sea-orm 2.0, Rust 1.94.

## Licence

MIT or Apache-2.0, at your option.
