Read `tower-shooting-spec.md` end-to-end before touching code so you understand the integer cooldown/speed math, message names, and the determinism requirements.

The sequence below layers combat from contracts → world authority → pure systems → adapters → tests. Each stage has clear deliverables and exit checks so you can merge incrementally without breaking replay determinism.

# 1) Core contracts (`core`) — DONE

**Goal:** Introduce the shared vocabulary for health, damage, projectiles, and firing commands/events.

**Deliverables:**

* Add the `Health`, `Damage`, `ProjectileId`, and `ProjectileRejection` types exactly as specced, with doc comments spelling out zero/dead semantics and rejection reasons.
* Extend `TowerKind` with integer timing/damage accessors (`fire_cooldown_ms`, `speed_half_cells_per_ms`, `projectile_damage`). Keep them `const fn` and unit-test the `Basic` values.
* Add the `Command::FireProjectile` variant, extend `Command::SpawnBug` with a `health: Health` field, and update `Event` with the new projectile/bug health events. Document ordering/meaning for each event.

**Exit checks:** `core` compiles, serialization/serde (if present) covers the new types, and unit tests prove the constant values/constructors.

# 2) World data scaffolding (`world`) — DONE

**Goal:** Prepare authoritative storage for cooldowns, health, and projectiles without altering behaviour yet.

**Deliverables:**

* Extend tower state with `cooldown_remaining: Duration` (initialised to zero) and ensure builder mode keeps it frozen.
* Extend bug state with `health: Health`; update spawn helpers/default fixtures to pass the new health argument.
* Introduce `ProjectileState` struct (id, tower, target, start/end `CellPointHalf`, `distance_half`, `travelled_half`, `speed_half_per_ms`, cached `damage`). Add `projectiles: BTreeMap<ProjectileId, ProjectileState>` and `next_projectile_id` allocator.
* Wire bug removal helpers so they key off `health == Health::ZERO` rather than ad-hoc flags (but no behaviour change yet).

**Exit checks:** World crate compiles, state constructors/tests updated, and existing behaviour is unchanged when no projectiles exist.

# 3) World command handling — DONE

**Goal:** Make the world authoritatively accept or reject `FireProjectile` requests.

**Deliverables:**

* Implement `apply(Command::FireProjectile)` following the spec’s rejection order (mode → tower existence/cooldown → bug existence/health).
* Reuse helpers to compute tower/bug centres in half-cell space, initialise `ProjectileState`, allocate ids, set tower cooldown via `Duration::from_millis`.
* Emit exactly one of `ProjectileFired` or `ProjectileRejected { .. }` per command. Cooldown should reset only on successful fire.

**Exit checks:** Focused world unit tests cover each rejection reason and the success path, ensuring state mutations/events match expectations.

# 4) Tick integration & bug death — DONE

**Goal:** Advance cooldowns/projectiles during attack ticks and resolve damage deterministically.

**Deliverables:**

* During `Tick` handling (attack mode only): decrement tower cooldowns saturating at zero, advance projectiles by `speed_half_per_ms * dt.as_millis()` with clamping to `distance_half`.
* When `travelled_half >= distance_half`, branch on bug liveness (by id). Apply damage atomically, emit `BugDamaged` and optionally `BugDied` when health hits zero, clean up occupancy/pathing, then emit `ProjectileHit`. If the bug is already gone, emit `ProjectileExpired`.
* Remove projectiles from the map exactly once after emitting the terminal event.
* Ensure builder mode leaves cooldowns/projectiles untouched even if ticks continue for UI cadence.

**Exit checks:** World tests cover cooldown pausing/resuming, projectile travel with varied `dt`, damage/death ordering, and expiration when bugs die early.

# 5) World queries & views — DONE

**Goal:** Expose read-only projections for systems/adapters without leaking internals.

**Deliverables:**

* Implement `query::tower_cooldowns(&World) -> impl Iterator<Item = TowerCooldownView>` returning tower id, kind, and `ready_in: Duration` or milliseconds.
* Implement `query::projectiles(&World) -> impl Iterator<Item = ProjectileSnapshot>` including ids, tower/bug ids, integer endpoints (`origin_half`, `dest_half`), `distance_half`, `travelled_half`, and `speed_half_per_ms`.
* Update existing queries (bugs, towers) to filter out dead bugs automatically.

**Exit checks:** Query unit tests verify stable ordering (BTree iteration), correct values, and that snapshots remain integer-based for determinism.

# 6) Deterministic replay guard — DONE

**Goal:** Lock in world-side determinism before layering systems.

**Deliverables:**

* Extend the replay harness to spawn bugs with health, issue scripted `FireProjectile` commands/ticks, and assert identical world state + event log hashes across runs.
* Cover cases where a bug dies before impact to ensure `ProjectileExpired` ordering stays deterministic.
* Added a world-level combat replay test that fires two projectiles at the same bug, asserting a hit followed by an expiration produces identical fingerprints across runs.

**Exit checks:** New replay test passes repeatedly and is wired into CI.

# 7) Tower combat system (`systems/tower_combat`) — DONE

**Goal:** Emit `FireProjectile` commands from pure data using the new queries.

**Deliverables:**

* Create the new system struct with scratch buffers, implement `handle` that early-outs unless `PlayMode::Attack`.
* Iterate the targeting DTOs in stable order, consult cooldown view (map or binary search) to confirm `ready_in == 0`, and push `Command::FireProjectile { tower, target }`.
* Unit tests verify readiness gating, deterministic ordering, and builder-mode silence.
* Implemented `TowerCombat` using a scratch command buffer and binary-search cooldown lookup, plus unit tests covering builder mode, cooldown gating, and missing tower entries.

**Exit checks:** System crate compiles, clippy/test suite passes with the new coverage.

# 8) Simulation wiring — DONE

**Goal:** Drive the new system each tick and queue commands to the world without violating layering.

**Deliverables:**

* Extend the CLI simulation (and shared sim harness) to fetch `tower_cooldowns` & `projectiles` queries, hold reusable caches, and call `tower_combat.handle` after targeting.
* Ensure commands flow through the existing queueing mechanism before the next tick; no direct world mutation.
* Keep builder mode short-circuit consistent with targeting system (clear caches when play mode changes).
* Wired the CLI simulation to cache cooldown/projectile snapshots, invoke tower combat after targeting, queue fire commands through the existing buffer, and cover builder/cooldown gating with new tests.

**Exit checks:** Headless simulation tests cover attack/builder transitions, confirm commands emit only when cooldown-ready, and ensure projectile snapshots are cached.

# 9) Scene & adapters — DONE

**Goal:** Visualise projectiles using deterministic data only.

**Deliverables:**

* Extend `Scene` with `projectiles: Vec<SceneProjectile>` containing ids, endpoints, float positions/progress derived from integer snapshots.
* Update CLI/macroquad adapters to populate/draw projectile dots (radius derived from cell size) without inventing new timing logic.
* Adjust existing scene constructors/tests/snapshots to include the new field while keeping non-visual adapters no-ops.
* Populated scene projectiles from world snapshots, exposed them via adapters, and rendered them as deterministic macroquad dots.

**Exit checks:** Rendering/unit tests cover the new scene data; manual smoke test shows dots travelling along targeting lines.

# 10) Full test suite & docs — DONE

**Goal:** Harden the feature and update documentation.

**Deliverables:**

* Expand world/system tests for edge cases (multiple towers, simultaneous hits, cooldown overlaps) and ensure rejection events stay logged.
* Update any developer docs referencing tower behaviour to include shooting flow; cross-link `tower-impl.md`, `target-impl.md`, and the new combat steps.
* Run the full CI guard set (`cargo fmt --check`, `cargo clippy --deny warnings`, `cargo test`, `cargo hack check --each-feature`, `cargo +nightly udeps`) to confirm a clean slate.
* Added `tick_resolves_simultaneous_projectiles_in_id_order` plus supporting documentation cross-links, then recorded clean guard runs for fmt, clippy, tests, feature checks, and nightly `udeps`.

**Exit checks:** Entire suite passes locally (fmt, clippy, tests, cargo-hack feature matrix, nightly `udeps`); docs compiled; replay hashes stable across repeated runs.

---

## Why this order?

* Contracts first keeps dependent crates compiling and lets you write tests against stable types.
* World authority is proven (including replay) before any system or adapter depends on it, preventing “spec drift” while UI work is in flight.
* Systems and simulation wiring remain pure consumers of queries, so adapters stay logic-free and deterministic.
* Ending with the full test suite guarantees projectile combat can ship without reopening earlier layers.
