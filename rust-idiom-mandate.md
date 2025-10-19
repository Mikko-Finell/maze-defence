# Code Mandate

This document defines the mandatory Rust engineering principles for this codebase.
It exists to prevent “C++/Go/Unity-style spaghetti written in Rust.” Every PR is evaluated against this.

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
