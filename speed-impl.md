Read `speed-spec.md` end-to-end before touching any code so cadence contracts,
domain invariants, and determinism rules stay aligned.

This roadmap mirrors the other `*-impl.md` guides: every stage is mergeable on
its own, respects the architecture guardrails, and layers behaviour from
contract extensions → world authority → system integration → harness coverage.
Nothing has landed yet, so each checkpoint below is marked `[TODO]`.

# 1) [DONE] Cadence contracts & snapshots (`core`)

**Goal:** Extend shared command/snapshot types so every caller deals with
resolved cadence data explicitly.

**Deliverables:**

* Add `step_ms: u32` to `Command::SpawnBug` and ensure helper constructors (e.g.
  the spawning tests and CLI helpers in `adapters/cli::simulation`) forward the
  value without defaulting silently.
* Extend `BugSnapshot` (and the corresponding `BugView`) with `step_ms`,
  `accum_ms`, and `ready_for_step`, plus doc comments that reiterate the
  integer-math contract from the spec.
* Update serde/encoding surfaces in `core` (including any replay fixtures) to
  include the new fields so journal logs remain faithful.
* Add focused unit or doc tests that prove `ready_for_step` toggles when
  `accum_ms` crosses the threshold and never exceeds `step_ms` after clamping.

**Exit checks:** `core` crate compiles, new fields are documented, existing
constructors/tests compile with explicit cadence arguments, and doctests cover
`ready_for_step` behaviour.

# 2) [TODO] World authority & accumulation (`world`)

**Goal:** Persist cadence per bug and update tick/step flows without disturbing
other movement logic.

**Deliverables:**

* Extend the internal `Bug` struct (see `world::entities::bug`) with `step_ms`
  and `accum_ms`, defaulting both via the spawn command.
* Update `world::apply::spawn_bug` (and any builder-mode spawn helpers) to copy
  `step_ms` from the command and initialise `accum_ms` to `step_ms` so freshly
  spawned bugs may move immediately when appropriate.
* Modify the tick handler (`world::apply::tick` or the dedicated cadence
  advance function) to accumulate `dt_ms` with `accum_ms = (accum_ms + dt_ms)
  .min(step_ms)` as pure integer math.
* Adjust the movement resolution path (`world::apply::step_bug_success`) to
  subtract `step_ms`, saturating at zero and carrying remainder when a bug moves
  multiple times in one tick.
* Backfill world-level tests that exercise edge cases: slow cadence bugs, large
  `dt_ms` jumps, and repeated step success within a single tick.

**Exit checks:** World crate compiles, cadence state persists across ticks,
world tests cover accumulation/consumption, and movement rejection paths remain
unchanged.

# 3) [TODO] Snapshot & query surfaces (`world::snapshot` + `world::query`)

**Goal:** Propagate cadence state to observers while keeping systems on the
`ready_for_step` contract.

**Deliverables:**

* Update snapshot builders (e.g. `world::snapshot::assemble_bug_snapshot`) to
  copy `step_ms`/`accum_ms` from the world entity and derive
  `ready_for_step = accum_ms >= step_ms` exactly once.
* Thread the new fields through `BugView` so systems/movement queries receive
  the resolved cadence without borrowing world internals.
* Audit the movement system entry point (`systems::movement::Movement::handle`)
  to ensure it continues filtering exclusively on `ready_for_step`, removing any
  leftover manual counters or assumptions that a global cadence still exists.
* Extend snapshot/query unit tests to assert cadence fields survive world →
  snapshot → query round trips with no mutation.

**Exit checks:** Query helpers compile, movement system still compiles without
additional branching, and snapshot tests validate cadence state and the derived
flag.

# 4) [TODO] Spawn pipelines & adapter wiring (`systems/spawning` + adapters)

**Goal:** Ensure every command producer supplies explicit cadence data and
scenario helpers stay ergonomic.

**Deliverables:**

* Update `systems::spawning::Spawning::handle` to resolve cadence per species or
  wave using the rules from `speed-spec.md`, emitting `Command::SpawnBug` with
  populated `step_ms`.
* Thread cadence through any scenario configuration structs (e.g.
  `adapters/cli::scenario::BugTemplate`) and CLI helpers that currently inject
  spawn commands so authors cannot forget to provide a value.
* Adjust default scenarios and tests in `systems/spawning/tests` and
  `adapters/cli/tests` to provide explicit `step_ms`, using shared helpers when
  a global constant is appropriate.
* Verify that other command sources (editor/build mode tools, replay loaders)
  populate cadence fields when reconstructing `SpawnBug` commands.

**Exit checks:** All spawn command emitters compile with explicit cadence,
scenario fixtures cover both shared and custom cadence values, and replay inputs
round-trip without missing data.

# 5) [TODO] Determinism harness & documentation polish (tests + docs)

**Goal:** Lock cadence behaviour into the replay suite and document contributor
guardrails.

**Deliverables:**

* Extend deterministic replay tests (`systems/movement/tests/deterministic_replay.rs`
  and any golden harnesses) with mixed-cadence scenarios that exercise staggered
  stepping and verify hash stability.
* Update documentation (`movement.md`, `path-impl.md` cross-links, or adjacent
  contributor guides) to explain cadence expectations, including the
  accumulator clamp rule and the reason planners only read `ready_for_step`.
* Run the full guard set (`cargo fmt --check`, `cargo clippy --deny warnings`,
  `cargo test`, `cargo hack check --each-feature`, `cargo +nightly udeps`) to
  establish a clean baseline and capture the cadence fields in replay logs.

**Exit checks:** Replay hashes remain stable, docs reference the cadence flow,
and CI guard commands pass locally.

---

Following this sequence keeps cadence authoritative in the world, exposes only
resolved state to systems, and hardens determinism before tuning future bug
behaviour.
