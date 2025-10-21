Start by reading `target-spec.md` so the behaviour contracts are locked in before you touch any code.

Here’s the leanest sequence to bring tower targeting online without breaking determinism or layering rules. Each step is a mergeable checkpoint with explicit exit criteria.

# 1) [DONE] Range contracts (core + world queries)

**Goal:** Give systems deterministic range information without duplicating adapter math.

**Deliverables:**

* `TowerKind::range_in_tiles` and `TowerKind::range_in_cells` with docs that call out the `Basic` radius and integer rounding expectations.
* `world::query::cells_per_tile(&World) -> u32` so every consumer reuses the authoritative spacing.
* Unit tests for both helpers, covering zero/edge inputs and verifying `cells_per_tile` never returns `0`.

**Exit checks:** Core and world crates compile; new helpers are doc-commented and exercised by tests only.

# 2) [TODO] Targeting system crate

**Goal:** Implement the pure targeting logic exactly once.

**Deliverables:**

* New `systems/tower_targeting` crate owning `TowerTargeting`, DTOs, and tests. Method signature mirrors the plan from `target-spec.md` (play mode, tower/bug views, cells per tile, output buffer).
* Algorithm stays in integer half-cell space until emitting the final float centres; tie-breaking follows the spec’s order.
* Scratch buffers stored on the struct to avoid per-call allocations.

**Exit checks:** System unit tests cover in/out-of-range bugs, deterministic ties, builder-mode early-outs, and empty collections. Clippy passes with `--deny warnings` for the new crate.

# 3) [TODO] Simulation wiring (CLI adapter)

**Goal:** Feed the system authoritative data each tick and retain results for rendering.

**Deliverables:**

* Extend the CLI `Simulation` with a `TowerTargeting` field and a reusable `Vec<TowerTarget>` cache.
* Invoke `tower_targeting.handle` immediately after movement resolution while in attack mode, clearing cached results whenever play mode flips back to builder.
* Re-export a helper that converts the DTOs into scene-ready line descriptors in cell space.

**Exit checks:** Headless simulation tests prove targets appear/disappear as play mode switches, and that equidistant bugs resolve to the same `BugId` every tick.

# 4) [TODO] Scene & rendering adapters

**Goal:** Visualise targeting without embedding logic in adapters.

**Deliverables:**

* Extend `Scene` with a `tower_targets: Vec<TowerTargetLine>` that stores tower/bug ids plus cell-space endpoints.
* Update CLI scene population to fill the new vector using the helper from step 3; macroquad (and any other renderer) draws thin black lines after towers but before bugs.
* Adjust constructors/tests for `Scene` (and any snapshot expectations) to account for the new field while keeping non-visual adapters no-ops.

**Exit checks:** Rendering unit/golden tests cover the new vector population, and manual run shows lines anchored at entity centres. No adapter pulls in system logic.

# 5) [TODO] Determinism harness & polish

**Goal:** Lock in behaviour and guard against regressions.

**Deliverables:**

* Add replay coverage that positions two equidistant bugs to assert stable tie-breaking, plus a case verifying builder mode yields zero targets.
* Document the new targeting flow in developer docs if needed (cross-link from `tower-impl.md`).
* Audit `cargo fmt`, `clippy`, `udeps`, and feature checks for green CI.

**Exit checks:** Deterministic replay test passes repeatedly; CI suite is clean; documentation references the targeting contracts without duplication.

---

Following this order lets you prove targeting correctness in isolation before touching adapters, then surface the visuals with zero behavioural guesswork. Each checkpoint is shippable and keeps `target-spec.md` as the single source of truth for domain rules.
