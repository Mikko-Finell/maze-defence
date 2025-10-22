Read `path-spec.md` in full before you touch any code. The static flow field,
congestion heuristics, detour search, and reservation rules there are the single
source of truth for crowd movement expectations.

This roadmap mirrors the other `*-impl.md` guides: each step is mergeable on its
own, keeps determinism guards green, and layers behaviour from contracts →
world authority → pure systems → harness coverage. Because no work has landed
yet, every stage below is marked `[TODO]`.

# 1) [DONE] Navigation contracts & tuning constants (`core`)

**Goal:** Establish the shared vocabulary and fixed tuning knobs the rest of the
stack relies on.

**Deliverables:**

* Introduce a documented `NavigationFieldView` DTO (or equivalent) that exposes
  width, height, and the dense `Vec<u16>` backing without granting mutation.
* Add `CONGESTION_LOOKAHEAD`, `CONGESTION_WEIGHT`, and `DETOUR_RADIUS` (integer)
  constants exactly as specced, with doc comments calling out determinism and
  acceptable ranges.
* Extend the query surface so systems can request the navigation field via an
  explicit message/DTO rather than reaching into world internals.
* Update any serialization/serde registries so the new types participate in
  existing test helpers and fixtures.

**Exit checks:** `core` compiles, constant values are unit-tested, and the query
API has doc tests demonstrating read-only usage.

# 2) [DONE] Static navigation field storage (`world`)

**Goal:** Build and persist the Manhattan-distance grid authoritative to the
world state.

**Deliverables:**

* Add a `world::navigation` module holding `NavigationField` (dense vector plus
  dimensions, including the virtual exit row).
* Implement the reverse breadth-first builder seeded from every exit tile,
  ignoring dynamic occupancy but respecting static walls exactly as described in
  the spec.
* Hook field construction into world initialisation and rebuild paths whenever
  maze geometry or exit configuration changes (builder mode edits, target
  swaps, etc.).
* Store the field in the world struct alongside a dirty flag so rebuilds happen
  exactly once per structural edit.
* Write focused world tests on hand-authored mazes confirming gradients decrease
  toward the exit and virtual exit cells get the expected `0` distance.

**Exit checks:** World crate compiles, builder tests pass, and maze mutation
paths trigger deterministic rebuilds without panics.

# 3) [DONE] Navigation field query surface (`world::query`)

**Goal:** Expose read-only access to the field so systems/adapters can consume
it without violating authority boundaries.

**Deliverables:**

* Implement `query::navigation_field(&World) -> NavigationFieldView` that borrows
  the world-owned field and documents lifetime/ordering guarantees.
* Ensure the view iterates in stable row-major order and includes the virtual
  exit row so adapters can reason about the boundary condition.
* Update any existing query aggregators to thread the navigation field through
  replay/simulation harnesses without duplicating buffers.
* Add unit tests (or doctests) verifying the view’s indexing helpers and
  asserting that updates to the world swap in a fresh field transparently.

**Exit checks:** Query tests pass, replay fixtures compile, and no caller needs
mutable access to the field.

# 4) [TODO] Movement system scaffolding (`systems/movement`)

**Goal:** Restructure the movement system to drive all stepping through a new
crowd planner while keeping legacy behaviour intact for now.

**Deliverables:**

* Introduce a `CrowdPlanner` (or similar) struct owning reusable scratch
  buffers for congestion counts, detour queues, and the two-tick ring buffer for
  `last_cell` tracking.
* Thread the new navigation field view and reservation ledger into the planner’s
  entry point while preserving the existing `StepBug` emission behaviour.
* Enforce deterministic bug iteration by sorting/iterating in ascending `BugId`
  inside the planner wrapper, documenting the contract for later steps.
* Backfill system tests to prove the refactor keeps legacy straight-line motion
  identical before congestion heuristics land.

**Exit checks:** System crate compiles, existing movement tests remain green, and
profiling shows no unexpected allocations after the refactor.

# 5) [TODO] Gradient-first progress (`systems/movement`)

**Goal:** Replace the "path to exit or nothing" logic with the static gradient
walker so bugs always advance when a lower-distance neighbour is free.

**Deliverables:**

* Enumerate the four orthogonal neighbours (plus virtual exit) and compute
  `distance_delta` using the navigation field for each.
* Emit `StepBug` toward the neighbour with the minimal `(distance, cell order)`
  tie-breaker when any candidate has `distance_delta < 0` and is unoccupied.
* Reset `stalled_for` counters whenever a bug moves; increment only when all
  neighbours fail checks.
* Extend system tests with small mazes proving bugs progress toward the exit
  even when distant tiles are blocked.

**Exit checks:** Movement tests cover gradient-only progress, and the planner no
longer falls back to full-path searches.

# 6) [TODO] Congestion map & side-step heuristics (`systems/movement`)

**Goal:** Bias traffic away from saturated lanes and allow controlled lateral
moves without oscillation.

**Deliverables:**

* Build the transient congestion `Vec<u8>` each tick by following the gradient
  up to `CONGESTION_LOOKAHEAD` cells per bug, skipping their current cell.
* Incorporate `CONGESTION_WEIGHT` into the neighbour scoring so ties prefer
  lower congestion after comparing distance.
* Implement the flat side-step rule: allow `distance_delta == 0` moves only when
  the neighbour’s congestion is lower than the current cell and it differs from
  the two-tick `last_cell` ring buffer entry.
* Add deterministic tests that demonstrate lane-formation behaviour and verify
  the anti-oscillation guard.

**Exit checks:** System tests confirm congestion-influenced routing, and
profiling/logging shows congestion buffers are reused between ticks.

# 7) [TODO] Detour BFS fallback & reservation awareness (`systems/movement`)

**Goal:** Teach the planner to escape jams via bounded detours while honouring
existing reservations.

**Deliverables:**

* Implement the depth-limited BFS (radius `DETOUR_RADIUS`) that searches for any
  free cell with a lower navigation score, falling back to the lowest
  `(distance + congestion)` cell when none exist.
* Respect reservation data: treat cells claimed by lower `BugId` moves as
  occupied, but allow targeting a cell currently occupied when the ledger shows
  that occupant vacating this tick.
* Reconstruct the first hop from the BFS result and emit a `StepBug` toward it;
  when BFS fails entirely, increment `stalled_for` so bugs retry promptly once
  space opens.
* Unit-test mazes with temporary blockers and side corridors to prove bugs take
  detours instead of waiting indefinitely.

**Exit checks:** New tests cover detour success/failure, and replay traces show
stable ordering despite BFS exploration.

# 8) [TODO] Determinism harness & documentation (tests + docs)

**Goal:** Lock in the new behaviour with replay coverage and contributor
guidance.

**Deliverables:**

* Extend the deterministic replay suite with the dense-crowd scenarios from the
  spec: jammed corridor, side hallway diversion, and the original stall
  regression.
* Document the new planner in `movement.md` (or adjacent docs), cross-linking to
  `path-spec.md` and explaining each constant plus the reservation interplay.
* Capture tuning guidance in the docs so future adjustments to lookahead/weights
  rerun the same replay scenarios.
* Run the full guard set (`cargo fmt --check`, `cargo clippy --deny warnings`,
  `cargo test`, `cargo hack check --each-feature`, `cargo +nightly udeps`) to
  record a clean baseline after the overhaul.

**Exit checks:** Replay hashes remain stable across runs, documentation builds
without warnings, and the CI guard set is green.

---

Following this sequence keeps the architecture honest: contracts land first,
world builds the authoritative data once, the movement system evolves through
pure, testable increments, and determinism guards close the loop before any
future tuning. Each checkpoint is mergeable and provides a clear audit trail
back to `path-spec.md`.
