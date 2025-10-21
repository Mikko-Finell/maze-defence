Here’s a tight, low-risk sequence that keeps the tree green at every merge and proves determinism early. Each step has a hard “exit check”. 

Do not start working before you're read `tower-spec.md` so you understand the context of what you're doing.

# 1) Contracts first (core)

**Goal:** Define the vocabulary; no behavior.

**Status:** Done

* Add `TowerId`, `TowerKind`, `PlacementError`, `RemovalError`.
* Add `Command::{PlaceTower, RemoveTower}` and `Event::{TowerPlaced, TowerRemoved, TowerPlacementRejected, TowerRemovalRejected}`.
* No half-tile/visual types; all cell-space.

**Tests/Exit:** Core compiles; serialization round-trip tests for new types.

# 2) World scaffolding (no public behavior yet)

**Goal:** Prepare authoritative storage & occupancy folding.

**Status:** Done

* Add `world::towers` module with `BTreeMap<TowerId, TowerState>`, `next_tower_id`.
* Add `tower_occupancy: BitGrid` and integrate it into `is_cell_blocked` (behind feature gate so it’s dormant until towers exist).
* Implement `footprint_for(kind)` (e.g., `Basic → 2×2` cells).

**Tests/Exit:** World compiles; `is_cell_blocked` unchanged when no towers exist; unit test proves folding logic is inert without entries.

# 3) World handlers (authoritative mutation)

**Goal:** Make the world able to accept/reject tower mutations deterministically.

**Status:** Done

* Implement `apply(PlaceTower)`: mode check, alignment check, bounds, occupancy check, allocate id, set bits, insert, emit `TowerPlaced`; emit `TowerPlacementRejected` on any failure.
* Implement `apply(RemoveTower)`: mode check, existence check, clear bits, remove, emit `TowerRemoved` (or `RemovalRejected`).

**Tests/Exit:** Pure world tests for each success/failure path; ID increments only on success; occupancy flips exactly the footprint; zero adapter/system changes yet.

# 4) Read-only queries

**Goal:** Give systems/adapters a projection surface without leaking internals.

**Status:** Done

* Add `query::towers(world) -> iter (TowerId, TowerKind, CellRect)`.
* Add `query::tower_at(world, CellCoord) -> Option<TowerId>`.

**Tests/Exit:** Unit tests prove stable iteration order and accurate hit-tests over footprints.

# 5) Replay & determinism guard

**Goal:** Prove the mutation semantics are replay-safe before wiring UI.

**Status:** Done

* Added a deterministic replay harness that drives a place/remove/place script mixing valid placements with rejection cases.
* Asserted the resulting tower snapshots and event journal hashes match across runs, establishing a reproducible fingerprint.

**Tests/Exit:** New test runs in CI; marks a “determinism baseline” for towers.

# 6) Builder Tower System (pure systems layer)

**Goal:** Emit messages; still no rendering changes.

**Status:** Done

* New system subscribes to play-mode/cache, consumes preview & `FrameInput`.
* On confirm → `PlaceTower { kind, origin }`; on remove over hovered tower → `RemoveTower`.
* No ID allocation, no world calls.

**Tests/Exit:** Headless system tests: only emits commands in Builder; never when Attack; emits nothing on invalid preview.

# 7) Simulation integration (preview math stays here)

**Goal:** Close the loop without changing visuals yet.

**Status:** Done

* Extend preview to compute snapped `CellCoord` + candidate `CellRect` + “placeable” bool using queries.
* Queue system-emitted commands into the world at the proper step boundary.

**Tests/Exit:** Headless sim tests: preview flags occupancy conflicts; successful confirm produces `TowerPlaced` after a tick.

# 8) Scene & adapters (minimal rendering)

**Goal:** Make towers visible with zero logic in adapters.

**Status:** TODO

* Extend `Scene { towers: Vec<SceneTower { id, kind, region }] }` and optional `preview`.
* Adapters draw towers (rect/sprite) and translucent preview; no occupancy math, no inference.

**Tests/Exit:** Golden frame/scene tests (or snapshot assertions) proving scene contains expected towers; (manual run shows static placement working; you don't need to test this).

# 9) Removal UX + rejection feedback

**Goal:** Tighten edit loop and audit trail.

**Status:** TODO

* Wire right-click/delete to emit `RemoveTower`.
* Surface `Tower*Rejected` reasons back into preview tint/tooltips.

**Tests/Exit:** System tests cover removal; world tests cover all rejection reasons; scene reflects removals immediately.

# 10) Hardening & docs

**Goal:** Finish the loop; lock in guarantees.

**Status:** TODO

* Add property tests for footprint/occupancy mapping.
* Document contract in `CORE.md`/`AGENTS.md` cross-refs (authority, events always on failures, order guarantees).
* Add “tower place/remove” to the determinism harness suite.

---

## Why this order?

* **Contracts → world → tests → systems → sim → adapters** preserves layering and lets you **prove determinism before UI**.
* Each step is **mergeable and reversible**, with clear **exit checks** so you never ship half-mutations.
* World owns IDs/footprints early; adapters stay dumb the entire time; rejection events are in from step 3, so auditability is baked in.

If you want an even tighter loop, you can collapse steps 6–8 into a single PR stack, but keep 1–5 strictly in order.
