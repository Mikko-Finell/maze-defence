# Towers Architecture

## Intent

Introduce towers as first-class, world-owned blockers that are placed/removed only in **Builder** mode, persist across mode switches, and immediately affect pathfinding via the unified occupancy view—without leaking UI concerns into core contracts, and preserving strict determinism and auditability.

---

## Guiding principles

* **World authority**: All mutations flow through `World::apply`. Systems emit messages; adapters render and report input.
* **Flat contracts**: Core defines minimal, adapter-agnostic types (serde-friendly; no visual hints).
* **Determinism by construction**: World allocates IDs, uses an order-guaranteeing map, validates alignment, and emits explicit acceptance/rejection events.
* **Single occupancy view**: Movement/pathfinding consult the same `is_blocked(cell)` that already accounts for walls; towers plug into it.
* **Preview outside core**: Half-tile/cursor snapping belongs to systems/simulation; the world only validates generic alignment and bounds.

---

## Core contracts (crate: `core`)

**Types**

* `TowerId(u32)` — opaque, allocated by world.
* `TowerKind` — starts with `Basic`; future variants tune behavior (fire rate, etc.).
* `CellCoord`, `CellRect` — existing cell-space types (add if missing).

**Commands**

* `PlaceTower { kind: TowerKind, origin: CellCoord }`
  *Minimal*: footprint is not in the command; it’s derived by the world from `TowerKind`.
* `RemoveTower { tower: TowerId }`

**Events**

* `TowerPlaced { tower: TowerId, kind: TowerKind, region: CellRect }`
* `TowerRemoved { tower: TowerId, region: CellRect }`
* `TowerPlacementRejected { kind: TowerKind, origin: CellCoord, reason: PlacementError }`
* `TowerRemovalRejected { tower: TowerId, reason: RemovalError }`

**Errors**

* `PlacementError = { InvalidMode, OutOfBounds, Misaligned, Occupied }`
* `RemovalError = { InvalidMode, MissingTower }`

**Queries (read-only projections)**

* `query::towers(world) -> impl Iterator<Item = (TowerId, TowerKind, CellRect)>`
* `query::tower_at(world, c: CellCoord) -> Option<TowerId>`
* `query::is_cell_blocked(world, c: CellCoord) -> bool` (already exists; ensure towers are folded in)

> Note: No half-tile types or visual sizes in core; keep contracts domain-centric.

---

## World (crate: `world`)

**State**

* `towers: BTreeMap<TowerId, TowerState>` — stable iteration for projection/replay.
* `next_tower_id: TowerId` — monotonic, increment on successful placement only.
* `tower_occupancy: BitGrid` — same shape as movement grid; 1 = blocked by tower.
* `TowerState { id, kind, region: CellRect }`

**Footprint resolution**

* `fn footprint_for(kind: TowerKind) -> CellRect::Size` (e.g., `4×4` cells for `Basic`).
  Orientation left for later (axis-aligned now).

**Apply: PlaceTower**

1. If `mode != Builder` → emit `TowerPlacementRejected(InvalidMode)`.
2. Validate **alignment**: `origin` must satisfy the configured half-grid congruence (derived from `cells_per_tile/2`). If not, `Misaligned`.
3. Compute `region = CellRect::from(origin, footprint_for(kind))`.
4. Bounds check → `OutOfBounds` if fails.
5. Check occupancy (walls + towers + any future blockers) → if any cell blocked, `Occupied`.
6. Allocate `id = next_tower_id++`, set bits in `tower_occupancy`, insert `TowerState`, emit `TowerPlaced { id, kind, region }`.

**Apply: RemoveTower**

1. If `mode != Builder` → `RemovalRejected(InvalidMode)`.
2. Lookup `id`; if missing → `RemovalRejected(MissingTower)`.
3. Clear `tower_occupancy` bits for its `region`, remove state, emit `TowerRemoved`.

**Mode transitions**

* Towers persist unchanged across `Builder ↔ Attack`. No implicit mutation on mode change.

**Queries**

* Build projections strictly from `towers` and `tower_occupancy`. Never expose interior references; always immutable, serde-friendly views.

---

## Systems (crate: `systems`)

**Builder Tower System**

* Subscribes to `PlayModeChanged` to cache whether Builder is active.
* Consumes simulation-provided preview (snapped to half-tile) and `FrameInput`:

  * On confirm: translate snapped preview to `CellCoord origin` and emit `PlaceTower { kind, origin }`.
  * On remove action: hit-test via `query::tower_at` and emit `RemoveTower { tower }` if present.
* Pure: no direct world mutation; no ID allocation (world owns it).

**Tower Sync / UI Feedback**

* Listens to `Tower*` events to refresh any cached lists or to surface precise rejection reasons back to adapters (e.g., tint preview red with `Occupied`).

**Movement**

* No tower-specific code. Always call `query::is_cell_blocked`.

---

## Simulation & Adapters

**Simulation**

* Computes **snapped preview** (half-tile) from cursor and grid metrics (pure, deterministic math).
* Populates a preview descriptor (kind, snapped `CellCoord`, placeable bool via `query::is_cell_blocked` over the candidate region).
* Forwards user actions to the Builder Tower System.

**Adapters (rendering/input)**

* Input: report clicks/keys; no game logic.
* Rendering: draw towers from the **scene** (see below) and a translucent preview; never infer occupancy.

---

## Scene Projection (adapter-facing, not core)

Extend `Scene` (simulation population step) with:

* `towers: Vec<SceneTower { id, kind, region: CellRect }>`
* `preview: Option<PreviewTower { kind, region: CellRect, placeable: bool }>`
  Adapters use `region` to compute world-space transforms (one-tile sprite centered or debug outlines) as they see fit.

---

## Determinism & Auditability

* **Stable ordering** via `BTreeMap`.
* **World-owned ID allocation**; increment only on success.
* **Explicit rejection events** for every failed mutation path; no silent no-ops.
* **Alignment check in world** guards against drift between preview math and authority.
* **Replay test** includes mixed place/remove scripts and asserts snapshot/journal hashes.

---

## Testing Plan

**World unit tests**

* Place in Builder: emits `TowerPlaced`, sets occupancy, increments ID.
* Place out of bounds/misaligned/occupied/invalid mode: emits `TowerPlacementRejected` with correct reason; no state change.
* Remove in Builder: clears occupancy, emits `TowerRemoved`.
* Remove invalid mode/missing: emits `RemovalRejected`.

**Systems tests**

* Builder Tower System only emits commands in Builder mode; translates preview confirm/remove correctly; remains pure.

**Movement integration**

* Bugs cannot traverse tower cells; removing a tower immediately reopens the path.

**Replay**

* Deterministic run of scripted placements/removals yields identical world and journal hashes.

---

## Migration & Sequencing

1. **Contracts**: land `Command/Event/Error` additions and docstrings.
2. **World**: add `towers` module, ID allocator, alignment validator, occupancy folding, queries.
3. **Systems**: implement Builder Tower System; wire to existing builder preview.
4. **Simulation/Adapters**: extend preview + scene population; minimal rendering changes.
5. **Tests**: add world, systems, integration, and replay coverage; gate with CI.

---

## Future-proofing

* **Kinds → footprints/behaviour**: changing `TowerKind` updates world-side `footprint_for`.
* **Upgrades**: `UpgradeTower { tower, to: TowerKind }` can reuse the same world seams later.
* **Combat**: separate systems consume `towers()` and `bugs()` to drive targeting/timers/projectiles via messages—no new world leaks required.
