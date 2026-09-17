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

No fork of loco. Add it to your application:

```sh
cargo add loco-flags
```

And to your migration crate, because your `Migrator` names the migration below:

```sh
cargo add --manifest-path migration/Cargo.toml loco-flags
```

Then three lines:

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

One command per block, so copying one gives you one. There is an eighth, `flag:group`, under
[when you do want several switches on one feature](#when-you-do-want-several-switches-on-one-feature).

### Look at what is there

```sh
cargo loco task flag:list
```

### Make one

It arrives switched **off**, always. A flag that could be born on would make adding one a release.

```sh
cargo loco task flag:create key:paywall description:"kill switch for taking money"
```

### On and off

`flag:on` keeps any rollout the flag had, so switching off during an incident and back on
afterwards returns it to the rollout rather than to everybody at once.

```sh
cargo loco task flag:on key:paywall
```

`flag:off` beats a rollout: off means off, not off for the ninety percent who were not chosen.

```sh
cargo loco task flag:off key:paywall
```

### A percentage

This **also switches the flag on**, because a rollout on a killed flag reaches nobody.

```sh
cargo loco task flag:rollout key:occasions pct:10
```

Take the percentage away again, so being on is the whole answer:

```sh
cargo loco task flag:rollout key:occasions pct:clear
```

### One subject at a time

A decision about one host, party or tenant, which beats everything the flag says.

```sh
cargo loco task flag:override key:occasions scope:host:42 value:on
```

Withdraw it, and that subject goes back under the flag:

```sh
cargo loco task flag:override key:occasions scope:host:42 value:clear
```

### Delete it

Every exception on it goes too.

```sh
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

Two things about that are worth knowing before you need them. **`flag:rollout` switches the flag
on**, so setting a percentage on a flag you killed during an incident brings it back; `flag:off`
afterwards means what it says. And **a percentage rollout asked without a subject has no honest
answer**, so `Flags::load_global` reports `RolloutWithoutScope` and answers `false` for that flag.
If a flag is read both ways, in a request with a subject and in a worker without one, keep it off
rollouts or give the worker a subject.

## One feature is one flag

Check the same name everywhere the feature shows up: in the controller, in the worker, in the
payload the page reads. Same name, same subject, same answer, so nobody ever gets half a feature.

**Do not invent a second name for a second half.** `checkout_v2_backend` and `checkout_v2_frontend`
at ten percent each reach two *different* tenths, and a user lands in one without the other. That
is the rollout doing exactly what it is supposed to do, applied to a mistake.

## When you do want several switches on one feature

Sometimes one feature really is three switches, each needing its own kill switch, but all of them
have to reach the same people. Give them a group:

```sh
cargo loco task flag:group key:checkout_button  group:checkout
cargo loco task flag:group key:checkout_summary group:checkout
cargo loco task flag:group key:checkout_receipt group:checkout
```

Now all three at thirty percent reach exactly the same thirty percent, and switching one off leaves
the other two alone. A flag with no group draws from its own name, which is the default and the
reason two unrelated experiments never land on the same unlucky tenth of your users.

Set the group **before** a rollout starts. Changing or clearing it reshuffles who is inside, because
the audience is what the sum is over.

## The rollout is computed, not remembered

```
bucket = first 8 bytes of sha256(len+key, len+scope_type, len+scope_id) as u64, modulo 100
```

Three things follow, and all three are tested:

- **Raising a percentage never takes the feature away from anybody who had it.** Lowering it does
  put some back out, which is the honest meaning of a number that says "this fraction".
- **Two flags at ten percent pick different tenths**, because the flag's key is in the digest.
  Without that, the unluckiest tenth of your users would meet every experiment you ever run.
- **The same subject gets the same answer in every process, for ever**, with nothing stored and
  nothing to invalidate.

A percentage is a fraction of everyone who asks, not a headcount. At twenty five percent, a hundred
hosts means about twenty five; grow to a thousand and it is about two hundred and fifty, and the
original twenty five keep it. There is no "the first fifty people", because that would need a
counter and every process would count differently.

sha256 rather than `DefaultHasher`, whose output is explicitly allowed to change between Rust
releases. A rollout built on that would reshuffle on a toolchain bump and nothing would report it.

## Two tables, not three

A flag's definition, and the exceptions people wrote about particular subjects. There is no third
table holding resolved answers, and there is deliberately no way to define a flag as a rule in code.

Those two facts are the same fact. A rule written in code can answer differently on every call, so
its answers have to be frozen somewhere the moment they are first given, and that store then needs
invalidating, purging and reasoning about. Here the answer is arithmetic over the flag and the
subject, so it is recomputed every time and agrees with itself for ever. Nothing to store, nothing
to purge, and a read that is not secretly a write.

## What version 1 deliberately leaves out

Seams, not oversights:

- **No process-wide cache**, so two queries per request. Making N processes agree on a value within
  a bounded time, with no query in the request path, is the hard problem and it is being kept.
- **Flag names are strings**, so a typo answers `false` and writes a warning rather than failing to
  compile. A macro that knows the set of flags at compile time is the other hard problem.
- **Booleans only.** No variants, so no three-way test.
- **No admin screen.** Tasks first. A button sits on the same `store` functions these do.

## The design

[`docs/DESIGN.md`](docs/DESIGN.md) is the whole design with its reasoning: why two tables rather
than three, why the bucket is computed rather than stored, what was considered and rejected, and
what version 1 leaves out on purpose.

## Requires

loco 1.1, sea-orm 2.0, Rust 1.94.

## Licence

MIT. See [LICENSE](LICENSE).

Do what you like with it, keep the copyright line, and there is no warranty.
