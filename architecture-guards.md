# Architecture Guards

This codebase **will not rely on developer willpower** — architectural integrity is enforced by *structure, automation, and tests*.

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

