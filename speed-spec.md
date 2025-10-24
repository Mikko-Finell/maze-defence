# Bug Movement Cadence Specification (RFC)

## 1. Intent

Introduce **per-bug movement cadence** (step frequency) so bugs can advance at different speeds within the deterministic simulation — without altering any existing movement logic, system contracts, or message shapes other than explicitly adding cadence as resolved bug state.

This spec **only governs movement cadence** (when a bug may legally step).
It **does not redefine or mention spawning**, wave pacing, or pressure emission.

---

## 2. Guiding Principles

* **World is authoritative.** Cadence is stored, accumulated, and resolved inside `world` only.
* **Resolved only. No “default vs special”.** Systems never branch on provenance. Every bug exposes a *concrete* cadence value.
* **Integer math only.** Cadence and accumulators are `u32` milliseconds (or comparable integer unit). No floats.
* **Planner hot path unchanged.** Movement system continues to filter on `ready_for_step` from snapshots. No new branching.
* **Deterministic replay guaranteed.** Snapshots and journal log capture cadence as part of state.

---

## 3. Core Domain Model

At spawn time, each bug is assigned a *resolved* cadence:

```
step_ms: u32 // duration (ms) required between steps
accum_ms: u32 // accumulator for elapsed time since last step
```

A bug is **ready for step** when:

```
accum_ms >= step_ms
```

Upon a successful movement step:

```
accum_ms -= step_ms   // carry remainder if any
```

On tick:

```
accum_ms = min(step_ms, accum_ms + dt_ms)
```

No provenance flags. No “inherits default”. The resolved cadence value is authoritative.

---

## 4. Core Contract Changes (`core` crate)

### 4.1 Spawn Command Extension

Extend `Command::SpawnBug` with:

```rust
pub struct SpawnBug {
    // existing fields ...
    pub step_ms: u32, // required resolved cadence
}
```

Existing codepaths keep working by passing the current global cadence as `step_ms`.

### 4.2 Snapshot Extension

Extend `BugSnapshot` (and therefore `BugView`) with:

```rust
pub struct BugSnapshot {
    // existing fields ...
    pub step_ms: u32,
    pub accum_ms: u32,
    pub ready_for_step: bool, // derived from accum_ms >= step_ms
}
```

No further flags needed. This is *resolved state* only.

---

## 5. World Behaviour (`world` crate)

* Store cadence per bug:

```rust
pub struct Bug {
    // existing fields ...
    step_ms: u32,
    accum_ms: u32,
}
```

* `Tick(dt_ms)`:

  ```
  accum_ms = min(step_ms, accum_ms + dt_ms)
  ```

* `StepBug` success:

  ```
  accum_ms -= step_ms  // saturating; carry remainder
  ```

* Step rejection logic remains identical to today.

* Snapshot builder copies `step_ms` and `accum_ms`, and derives `ready_for_step`.

---

## 6. Movement System (`systems/movement`)

**No change to logic.**
Planner continues to use `BugSnapshot::ready_for_step`.

No inspection of raw numbers or cadence math required.

---

## 7. Testing & Determinism

* Golden replay tests must now include `step_ms` and `accum_ms` in hash/state.
* New scenario: spawn mixed‐cadence bugs (slow/fast) and verify replay stability.

---

## 8. Non-Goals

* No introduction of fractional or subcell movement.
* No alteration of path selection, congestion, or reservations.
* No implicit inheritance or “default cadence” concepts.
* No opinion on authoring tools, UI, or spawn pacing.

---

## 9. Summary

* Each bug owns a **resolved integer cadence**.
* Movement behaviour is strictly gated by **snapshot-level `ready_for_step`**.
* No branches or semantic flags leak into systems.
* World remains **full timing authority**, fully deterministic.

**This is the final authoritative movement cadence model.**
