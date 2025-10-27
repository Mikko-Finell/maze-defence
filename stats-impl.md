Read `build-mode.md`, `tower-spec.md`, and `path-spec.md` before proposing any
code for analytics. These metrics depend on authoritative maze geometry, tower
stats, and the navigation gradient contracts already enforced elsewhere.

This roadmap mirrors the other `*-impl.md` guides: each stage is mergeable on
its own, respects the message-passing architecture, and keeps analytics
computation fully decoupled from the gameplay hot path. Nothing has landed yet,
so every checkpoint below is marked `[TODO]`.

# 1) [DONE] Analytics contracts (`core`)

**Goal:** Introduce an explicit stats surface so systems can publish analytics
without leaking internal helpers.

**Deliverables:**

* [x] Define a `StatsReport` DTO that exposes the five metrics (tower coverage mean,
  firing-complete percentage, shortest path length, tower count, total DPS) plus
  doc comments describing sampling rules and invariants.
* [x] Add a `Event::AnalyticsUpdated { report: StatsReport }` (or similar) emitted
  whenever analytics recompute, and a `Query::analytics()` helper returning the
  latest report for adapters.
* [x] Introduce a `Command::RequestAnalyticsRefresh` so build-mode tools can nudge
  recomputation without reaching into the analytics system directly.
* [x] Document determinism expectations: analytics recomputation must derive solely
  from authoritative world state, never wall-clock timers.

**Exit checks:** `core` compiles, analytics types have documentation, and unit
tests/doctests illustrate that the report is immutable consumer-facing data.

# 2) [DONE] World data taps (`world`)

**Goal:** Expose the authoritative data analytics need without cross-cutting the
simulation authority layers.

**Deliverables:**

* [x] Add a `world::analytics` module that snapshots tower placements, spawner
  indices, target tiles, and cached tower DPS values without mutating gameplay
  state.
* [x] Extend `world::query` with read-only accessors that provide:
  * [x] the static navigation field/path gradient already stored for movement; and
  * [x] a lightweight iterator over tower entities yielding cell coordinates,
    targeting range, and DPS.
* [x] Emit `Event::MazeLayoutChanged` (if not already available) whenever build-mode
  adds/removes towers or modifies walls, keeping that emission inside world
  mutation handlers so gameplay systems stay untouched.
* [x] Backfill world tests ensuring layout edits set the dirty flag and that
  analytics queries match the authoritative state (tower counts, DPS sums).

**Exit checks:** World crate compiles, analytics queries are read-only, and
layout mutations reliably publish the layout-changed event.

# 3) [DONE] Analytics runtime (`systems/analytics`)

**Goal:** Build a dedicated pure system that listens for layout changes and
recomputes metrics on demand, keeping CPU usage predictable.

**Deliverables:**

* [x] Add a new `systems::analytics` module with an `Analytics` struct storing the
  last `StatsReport`, a queue of pending recompute requests, and reusable scratch
  buffers for path traversal.
* [x] Implement `ConsumesEvents` to watch for `MazeLayoutChanged` and
  `Command::RequestAnalyticsRefresh`, coalescing multiple signals into a single
  recompute per tick.
* [x] Ensure `Ticks` (or equivalent update trait) runs at normal game cadence but
  performs recomputation only when dirty, so analytics update in the background
  while remaining deterministic and CPU-friendly.
* [x] Write system-level tests verifying that tower edits enqueue a recompute and
  that the report is published via `Event::AnalyticsUpdated` without touching
  other systems.

**Exit checks:** System crate compiles, analytics system introduces no new
imports into gameplay modules, and unit tests confirm lazy recomputation.

# 4) [TODO] Metric algorithms (`systems/analytics::metrics`)

**Goal:** Implement the concrete metric calculations with deterministic integer
math and bounded iteration.

**Deliverables:**

* [x] Reuse the navigation field/pathfinding gradient to extract the shortest path
  from every spawner to the goal, selecting the overall shortest as the analysis
  track. Cache this path as a vector of cell indices for reuse during coverage
  sampling.
* [ ] Implement the invulnerable bug sweep:
  * [ ] For each cell on the path, gather towers whose range covers the cell and
    accumulate the per-cell coverage ratio (`towers_in_range / total_towers`);
    return the mean percentage as a fixed-point integer or rational documented
    in the spec.
  * [ ] Measure the earliest firing opportunities for towers:
    * [ ] Traverse the cached path in order, and for each tower record the first
      path cell where the bug is within range.
    * [ ] Report the path-length percentage corresponding to the furthest "first
      opportunity" among all towers. If any tower never gains line-of-sight,
      clamp the metric to 100%.
* [ ] Compute supporting metrics directly from the tower iterator: total count and
  sum of DPS (damage per second), ensuring the DPS calculation mirrors the
  authoritative combat system (document the formula source).
* [ ] Add focused unit tests on synthetic mazes covering corner cases: no towers,
  towers with zero DPS, spawners equidistant from the goal, and layouts where
  some towers never fire.

**Exit checks:** Metrics module compiles, tests cover edge cases, and all loops
execute in O(path_length × tower_count) time with documented bounds.

# 5) [TODO] Adapter integration & docs (`adapters` + guides)

**Goal:** Surface analytics to build-mode UI without coupling adapters to system
internals, and document usage for future maintainers.

**Deliverables:**

* [ ] Update build-mode adapters to issue `Command::RequestAnalyticsRefresh` whenever
  the player confirms a tower placement/removal, and subscribe to
  `Event::AnalyticsUpdated` to display the latest report.
* [ ] Document the analytics flow in `build-mode.md` (or a dedicated `stats-spec.md`)
  covering recompute triggers, expected latency, and background execution
  guarantees.
* [ ] Extend replay/tests to ensure analytics events appear deterministically given
  a fixed build sequence.
* [ ] Run the full guard suite (`cargo fmt --check`, `cargo clippy --deny warnings`,
  `cargo test`, `cargo hack check --each-feature`, `cargo +nightly udeps`) to
  establish a clean baseline once analytics land.

**Exit checks:** UI receives analytics updates without new tight coupling,
replay fixtures remain deterministic, and documentation explains the background
compute guarantees.

---

Following this sequence keeps analytics authoritative, rebuilds metrics only when
layout changes, and isolates stats logic inside a dedicated system so gameplay
code stays untouched.
