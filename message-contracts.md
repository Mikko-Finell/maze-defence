# Message Contracts

This document defines **exactly how systems in this engine communicate** — and more importantly how they *do not*.
It is language-agnostic in concept, but **expressed in idiomatic Rust terms** — enforced via type signatures and module boundaries.

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
