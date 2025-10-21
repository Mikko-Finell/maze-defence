# Tower Targeting Specification

## Intent

Elevate the static tower scaffolding from `tower-spec.md` into interactive defences. Towers must deterministically select a bug target within their configured range and surface that intent to adapters so they can render a targeting beam. The targeting logic must stay pure (system-level), reuse existing world queries, and avoid introducing new mutation paths so future combat systems can build on the same contracts.

---

## Guiding principles

* **Derived, not stored:** Target acquisition is transient state computed from world snapshots. The world continues to own only authoritative tower/bug data; targeting lives in a dedicated system.
* **Deterministic ties:** Given identical world state the system must always make the same choice. Distance comparisons and tie-breaks must be integer based to avoid float drift.
* **Cell-space first:** All computations operate in cell units so they align with world queries and existing drawing maths. Conversions to world units happen at the presentation layer.
* **Keep adapters dumb:** Adapters receive ready-to-render line descriptors. They do no spatial reasoning beyond unit conversions already in place for bugs and towers.
* **Builder isolation:** Targeting is inactive in builder mode to guarantee edit workflows remain unaffected.

---

## Domain rules

* **Range model:** Every tower kind exposes a range radius expressed in tile lengths. For `TowerKind::Basic` the radius is `4 * tile_length`. Convert this to cell units using `cells_per_tile` so the targeting system can stay in integer math.
* **Tower anchor:** Targeting originates from the geometric centre of the tower footprint expressed in cell coordinates (e.g., origin column + width/2.0). Bug centres are the owning cell plus `0.5` in each axis.
* **Candidate filtering:** Only bugs whose centre lies within (radius)^2 of the tower centre are considered. Towers with no candidates produce no assignment.
* **Tie-break order:** Sort candidates by: 1) squared distance (ascending), 2) `BugId` (ascending), 3) bug cell column, then row. This keeps selection deterministic without extra state.

---

## Core contracts (`core` crate)

* Extend `TowerKind` with documented helpers:
  * `pub const fn range_in_tiles(self) -> f32` – returns `4.0` for `Basic`.
  * `pub fn range_in_cells(self, cells_per_tile: u32) -> u32` – multiplies `range_in_tiles` by `cells_per_tile`, clamps at zero when input is zero, and returns the radius in whole cells.
* Add doc comments clarifying that range helpers are authoritative and used by combat systems.
* No new commands/events are required; targeting remains a derived projection.

---

## World queries (`world` crate)

* Expose `pub fn cells_per_tile(world: &World) -> u32` so systems can derive ranges directly from the authoritative world configuration instead of caching adapter arguments.
* Document that `cells_per_tile` returns at least `1` (world normalises zero to one during configuration).
* No additional world state is introduced; tower and bug snapshots already expose all geometric data required.

---

## Tower targeting system (`systems/tower_targeting` crate)

### Public surface

Create a new pure system crate `systems/tower_targeting` with:

```rust
pub struct TowerTargeting { /* scratch buffers */ }

impl TowerTargeting {
    pub fn handle(
        &mut self,
        play_mode: PlayMode,
        towers: &TowerView,
        bugs: &BugView,
        cells_per_tile: u32,
        out: &mut Vec<TowerTarget>,
    );
}
```

* `TowerTarget` is a DTO defined in the system crate containing:
  * `tower: TowerId`
  * `bug: BugId`
  * `tower_center_cells: CellPoint` – centre expressed in cell units as `f32` (e.g., `12.0`).
  * `bug_center_cells: CellPoint`
* `CellPoint` is a simple struct `{ pub column: f32, pub row: f32 }` documenting that values already include the `+0.5` cell-centre offset.

### Algorithm

1. Early out when `play_mode != PlayMode::Attack` or when either collection is empty.
2. Derive the targeting radius in cell units using `TowerKind::range_in_cells`. Multiply by two once to work in half-cell integers (`i64`) to avoid floating point comparison errors.
3. For each tower snapshot:
   * Compute the centre in half-cell units from `region.origin()` and `region.size()`.
   * Scan all bugs, compute the bug centre (cell index * 2 + 1), and compare squared distance against `(radius * 2)^2`.
   * Track the best candidate using the deterministic ordering described above.
4. Convert the winning tower/bug centres back to cell floats (`centre_units as f32 / 2.0`) and push a `TowerTarget` into `out`.
5. Reuse internal buffers (e.g., pre-allocated `Vec` for candidate ordering) to avoid per-frame allocations.

### Determinism guarantees

* All calculations use integer arithmetic until the final conversion step to cell floats, ensuring identical decisions across platforms.
* Sorting/tie-breaks rely solely on deterministic data (`BugId`, cell indices).

### Tests

* Unit tests covering:
  * Bug inside/outside range boundaries.
  * Two bugs at identical distance resolve via `BugId` ordering.
  * Towers with zero width/height footprints yield no targets.
  * Builder-mode short-circuits.
* Property-style test verifying that decreasing `cells_per_tile` or removing bugs never yields a target beyond range.

---

## Simulation integration (`adapters/cli` crate)

* Add a `tower_targeting: TowerTargeting` field plus `current_targets: Vec<TowerTarget>` cache to `Simulation`.
* During `process_pending_events` (after movement resolves and before builder commands), call `tower_targeting.handle(...)` with:
  * `play_mode = query::play_mode(&self.world)`
  * `towers = query::towers(&self.world)`
  * `bugs = query::bug_view(&self.world)`
  * `cells_per_tile = query::cells_per_tile(&self.world)`
  * Output vector = `self.current_targets`
* When play mode flips to Builder, clear the cached targets to avoid stale beams.
* In `populate_scene`:
  * Convert each `TowerTarget` into a rendering descriptor by multiplying cell coordinates by `cell_length = tile_grid.tile_length() / cells_per_tile as f32` and adding the same border offsets used for bugs.
  * Populate the scene’s targeting collection (see below).

---

## Scene & adapter updates (`adapters/rendering` crates)

* Extend `Scene` with a new field:

```rust
pub struct TowerTargetLine {
    pub tower: TowerId,
    pub bug: BugId,
    pub from: Vec2, // cell-space coordinates including the 0.5 offset
    pub to: Vec2,
}
```

* Add `pub tower_targets: Vec<TowerTargetLine>` to `Scene` and update constructors/tests accordingly. Initialise with `Vec::new()`.
* Provide a helper `fn push_tower_targets(scene: &mut Scene, targets: &[TowerTarget])` in the CLI simulation to translate DTOs into `TowerTargetLine` entries using cell coordinates (`Vec2::new(column, row)`).
* In `adapters/rendering_macroquad` add `draw_tower_targets(&scene.tower_targets, &metrics)` invoked after drawing towers and before bugs. Convert cell coordinates to world positions like bug rendering and draw a thin (`1.0` px) black line.
* Ensure other adapters (CLI text) either ignore or log the new field without side effects.

---

## Testing plan

1. **System tests (new crate):** validate the targeting algorithm under various layouts as described above.
2. **Simulation tests (CLI adapter):**
   * Construct a world with one tower/bug and assert `populate_scene` emits a single `TowerTargetLine` pointing from tower centre to bug centre (using known coordinates).
   * Toggle to builder mode and ensure the scene’s target list is emptied.
   * Multiple bugs scenario verifying deterministic tie-break (inspect `bug.id`).
3. **Adapter tests:** Extend existing presentation tests to assert `Scene::new` carries the new vector and that macroquad `draw_tower_targets` early-outs when the vector is empty.
4. **Deterministic harness:** Add a replay test that positions two bugs equidistant from a tower and verifies the selected target remains stable across runs.

---

## Rollout sequence

1. Implement `TowerKind` range helpers and `world::query::cells_per_tile` with unit tests.
2. Land the new `systems/tower_targeting` crate and cover its behaviour.
3. Wire the system into the CLI simulation loop and expose `Scene::tower_targets`.
4. Update adapters/rendering to draw beams, including constructor/test adjustments.
5. Add integration/replay tests proving determinism and builder-mode behaviour.

---

## Future considerations

* Additional tower kinds can extend `TowerKind::range_in_tiles` without touching the targeting system.
* When projectile firing arrives, reuse `TowerTarget` outputs to seed firing commands instead of recomputing selection logic.
* Consider caching bug positions in a spatial index if targeting cost grows with large bug counts; the system interface leaves room to swap implementations without changing adapters.
