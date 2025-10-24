# Architecture Guards

This codebase **will not rely on developer willpower** — architectural integrity is enforced by *structure, automation, and tests*.

This document is a style guide. Refer to `architecture.md` for the current engine model, keep that document up to date if you add a new system or significantly change an existing one. It will tell you where things are, how things work, and where to add new things. Read it.

---

## 1. Crate & Module Structure (Non-Negotiable)

```
/core            // Command, Event, Query types. No logic.
/world           // Owns authoritative state. Only mutation point.
/systems/*       // Pure. Consumes events + queries, emits commands.
/adapters/*      // IO edge: render, input, net. No logic or heuristics.
/tests           // Headless replay + golden snapshot tests.
```

**Illegal imports (CI REJECTS):**

* `systems::*` importing another system
* `systems::*` importing anything under adapters
* anything importing world internals except `world::apply()` / `world::query()`
* any `pub` that is not *part of an intentional outward contract*

---

## 2. Lint & Clippy Enforcement (on by default)

```rust
#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub,
)]
```

**PR auto-reject triggers:**

* `.clone()` without strong justification
* `Arc<Mutex<_>>` in systems or world (except concurrency boundary, documented)
* global mutable or thread-local state
* adapter calling world methods except `apply()` / `query()`
* systems importing adapters or vice versa

---

## 3. Harness: Deterministic Replay

A **hard requirement** for this engine:

```rust
fn replay(initial_state, commands, rng_seed) -> FinalSnapshot
```

—must be **bit-identical every run**.

We maintain a golden snapshot test like:

```rust
#[test]
fn wave_1_scenario_is_stable() {
    let out1 = replay(INIT_WORLD, scripted_commands(), seed(1234));
    let out2 = replay(INIT_WORLD, scripted_commands(), seed(1234));
    assert_eq!(hash(out1), hash(out2));
}
```

This makes state drift, time-based bugs, and “oops I didn’t think this change mattered” **immediately obvious**.

---

## 4. CI Guards

PR is rejected if **any** of the following fail:

| Check                             | Purpose                             |
| --------------------------------- | ----------------------------------- |
| `cargo fmt --check`               | No style drift                      |
| `cargo clippy --deny warnings`    | No sneaky landmines                 |
| `cargo udeps`                     | No dead code leaks                  |
| `cargo hack check --each-feature` | Features do not accidentally couple |
| Deterministic replay test         | No hidden side-effects / drift      |

---

## 5. Definition of Done (Enforced)

A PR is not accepted unless:

* ✅ All new behavior demonstrated via **messages**, not private calls
* ✅ At least *1 harness test* proves the change is stable and deterministic
* ✅ No temporary hacks (ANY “// TODO remove later” is automatic reject)
* ✅ The new code obeys import rules **without exception**

---

## 6. What This Guarantees Long-Term

This completes the Rust engine mandate triad.

These guardrails ensure:

* Long-term architectural stability
* Fully deterministic state evolution
* Strictly bounded module dependencies
* Auditability and mechanical testability of all behavior
* No reliance on hidden side effects or developer discipline

---

# Message Contracts

The following defines **exactly how systems in this engine communicate** — and more importantly how they *do not*. It is language-agnostic in concept, but **expressed in idiomatic Rust terms** — enforced via type signatures and module boundaries.

---

## 1. Only Three Legal Cross-System Interfaces

```rust
enum Command { /* request state change */ }
enum Event   { /* something happened */ }
enum Query   { /* read-only information */ -> Result<T> }
```

* **Commands** → “please mutate world state” (one-way, ordered, authoritative).
* **Events**   → “this happened” (fan-out, read-only, no mutation).
* **Queries**  → “give me a readonly view” (zero side effects).

Anything that doesn't fit **must be redesigned — not hacked in**.

---

## 2. The Mutation Choke Point

```rust
trait World {
    fn apply(&mut self, cmd: Command) -> Vec<Event>;
    fn query(&self, q: Query) -> Result<QueryResult>;
}
```

* No system, adapter, or helper is **ever** allowed to modify world state directly.
* All state mutation flows through **exactly one** implementation (`World::apply`).
* This is what makes time-travel, rollback, determinism, and headless testing possible.

---

## 3. Traits Define Legal Roles

```rust
trait ConsumesEvents {
    fn on_event(&mut self, event: &Event, out: &mut Emitter);
}

trait Ticks {
    fn tick(&mut self, dt: Duration, out: &mut Emitter);
}

trait Emitter {
    fn emit_command(&mut self, cmd: Command);
    fn emit_event(&mut self, event: Event); // typically only adapters log/forward
}
```

* Systems **do not** read or mutate world state directly — they express **intent** via commands.
* **Pure logic**, not ad-hoc plumbing. Everything is testable with just `(initial world, events, dt)`.

---

## 4. Boundaries Enforced by Module Hierarchy

```
core/           // Command, Event, Query definitions. Zero logic.
world/          // the only mutation authority. apply() + query().
systems/*       // pure, stateless logic: reads via events/queries, emits commands.
adapters/*      // IO: render, net, input. translate to/from messages ONLY.
```

**Systems must not:**

* import another system’s internals
* import any adapter or IO crate
* access world state directly

**Adapters must not:**

* mutate world
* “fix up” or reinterpret data
* inject behavior or heuristics

---

## 5. Message Schema Discipline

* All messages must be **serde-friendly struct literals**.
* Payloads = **flat, explicit, boring**. No trait objects, pointers, or hidden dependencies.
* No timestamps or nondeterministic values in payloads — that belongs to the engine tick, not the domain.

```rust
pub enum Command {
    SpawnEnemy { enemy_id: EnemyId, path: PathId },
    FireProjectile { tower_id: TowerId, target: EnemyId },
    // ...
}
```

If a payload is hard to serialize, it is probably architecturally wrong.

---

## 6. Determinism Guarantee

Given **(initial world + ordered commands + fixed RNG seed)** —
→ replay must produce **bit-identical events and state**.

Anything that breaks this (random sysclock calls, floating-point chaos, thread race) = **strict reject**.

---

## 7. Definition of Done (Message Perspective)

A PR is not “done” unless:

✅ It expresses all new behavior via **Command/Event/Query**, not private API
✅ It adds/updates message types explicitly, with docs
✅ It includes **at least one headless test** proving the behavior via messages only
✅ It does **not** require future cleanup / TODO / "temporary" cross-call hacks

---

**Everything is messages.**

---

# Code Mandate

These are the mandatory Rust engineering principles for this codebase. They exist to prevent “C++/Go/Unity-style spaghetti written in Rust.” Every PR is evaluated against this.

---

## 1. Ownership First — Never Fight the Borrow Checker

* Prefer **move semantics** where ownership is clear. Borrow (`&/&mut`) only when lifetime is naturally scoped.
* **Avoid `.clone()` unless you can justify it** in the PR description.
* If you feel the need for `Rc<RefCell<_>>`, you are almost always **architecturally wrong** — back up and redesign your data flow.
* Data mutation happens in **exactly one place** per tick (`World.apply()` or equivalent). Never leak interior mutability across systems.

---

## 2. Algebraic Types Over Primitive Soup

* Express domain logic using **enums + structs**, not flags/strings/booleans/int codes.
* All state that can be absent must be `Option<T>`, never “0 or empty = none”.
* All fallible operations must return `Result<T, E>`, never panics or magic defaults.
* Avoid “generic config bags.” Replace with **typed config structs per subsystem**.

```rust
enum Command { FireProjectile { from: EntityId, target: EntityId } }
enum Event   { ProjectileHit { target: EntityId, damage: u32 } }
```

---

## 3. Zero-Cost Abstractions Only

* Traits are for **interfaces** and **capability expression**, not inheritance.
* Generic functions + trait bounds > boxed trait objects, unless truly hot-swap runtime.
* No “manager” or “service” objects. Use **narrow role-specific traits**.

Bad:

```rust
trait GameSystem { fn update(&mut self, world: &mut World); }
```

Good:

```rust
trait ConsumesEvents { fn on_event(&mut self, event: &Event, out: &mut Emitter); }
trait Ticks          { fn tick(&mut self, dt: Duration, out: &mut Emitter); }
```

---

## 4. RAII and Deterministic Behavior

* Lifecycle management must rely on **Drop** or **owned scopes**, not “cleanup later” logic.
* No implicit global state — everything flows through explicit injection or construction.
* No hidden time: **explicit Duration dt** always passed to tick logic.
* Every tick must be 100% deterministic given (state + commands + seed).

---

## 5. Clarity Over Convenience

* Avoid frameworky patterns, reflection, serialization hacks, or ad-hoc DSLs.
* No auto-registering or magical discovery of systems or components.
* Prefer **explicit, boring wiring** over “magic extensibility.”

---

## 6. Strict Module / Visibility Discipline

* `pub(crate)` is default; `pub` is **only for stable surface contracts**.
* No system is allowed to import or depend on another system’s internals.
* Run `cargo udeps` and `cargo crev` regularly — unused code or unreviewed deps are red flags.

---

## 7. No Fictional Safety or Soft Contracts

* No `assert!(should_never_fail())` — write actual types that make it impossible to fail.
* No “we trust this conditional to be correct.” Either domain-model it or prove it with tests.
* No speculative abstractions — if a trait has only one implementation, it probably shouldn't exist.

---

## 8. Definition of “Idiomatic PR”

A PR is *automatically rejected* if it includes:

* `.clone()` “just to make it compile”
* global mutable state or thread-local hacks
* `Arc<Mutex<T>>` not justified by concurrency boundary
* calls across system boundaries instead of emitting messages
* `pub` types/methods that aren’t clearly part of an intentional external contract
* “cleanup later” comments

A PR is *strong* if:

* it **removed** an abstraction or `clone`
* it made **types narrower or more explicit**
* it **proved** behavior via message-driven tests
* it is **boringly predictable to read**
