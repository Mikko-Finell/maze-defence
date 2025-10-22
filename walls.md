# Cell Wall Migration Plan

## Goals
- Remove the legacy `maze_defence_core::Wall` surface and represent the perimeter entirely with `structures::Wall` cells.
- Extend the navigation grid so the bottom of the map renders a walkable row above an explicit wall row, with exit cells living in an off-screen row below.
- Maintain deterministic bug movement: bugs march through the visible exit gap, step once more into the hidden exit row, and are culled there.
- Keep adapters dumb—rendering receives precomputed cell walls and colors, no geometry math beyond existing conversions.

## 1. Core contract updates (`core/src/lib.rs`) [DONE]
1. Delete the `pub struct Wall` type and its associated `query::wall` exposure.
2. Keep `Target`/`TargetCell` public but decouple them from the removed `Wall` wrapper.
3. Verify nothing else in `core` references the legacy `Wall`; adjust re-exports/tests accordingly.

## 2. World data and helpers (`world/src/lib.rs`) [DONE]
1. **State layout**
   - Replace the `wall: Wall` field on `World` with a `target: Target` field (and reuse the existing `targets: Vec<CellCoord>` cache).
   - Update `World::new` and the `Command::ConfigureTileGrid` branch to rebuild `target` directly.
2. **Grid dimensions**
   - Introduce `const BOTTOM_BORDER_CELL_LAYERS: u32 = 1;` alongside the existing side/top constants.
   - Replace `EXIT_CELL_LAYERS` handling with `const EXIT_CELL_LAYERS: u32 = 1;` (unchanged) but have `total_cell_rows(...)` include `TOP_BORDER + BOTTOM_BORDER + EXIT` layers so we own three extra rows: top walkway, visible wall row, hidden exit row.
   - Add helpers that surface the indices we care about (`visible_wall_row(columns, rows, cells_per_tile)`, etc.) to keep later code readable.
3. **Cell wall synthesis**
   - Implement `build_cell_walls(...) -> Vec<CellWall>` so it returns every perimeter cell on the visible wall row except the exit gap columns returned by `exit_columns_for_tile_grid(...)`.
   - Ensure the result spans the full width (`0 .. total_cell_columns`), including the walkway columns contributed by `SIDE_BORDER_CELL_LAYERS`.
4. **Queries and blockers**
   - Delete `query::wall` and adjust `query::target` to read from the new `target` field.
   - Update `query::walls` to continue returning a `CellWallView` backed by the rebuilt `MazeWalls` structure.
   - Confirm `query::is_cell_blocked` still checks `self.walls.contains(cell)` so the new cell walls act as solid blockers.
5. **Spawner maintenance**
   - Update `BugSpawnerRegistry::remove_bottom_row` (and any helpers) so it removes both the visible wall row and the hidden exit row from the rim set. That avoids queuing useless bottom spawners against blocked cells.
6. **Target caching**
   - Inline the old `build_wall`/`target_cells_from_wall` helpers into simpler `build_target(...)` logic that emits `Target` + `Vec<CellCoord>` directly.
7. **Tests**
   - Add focused unit coverage that `build_cell_walls` yields: (a) empty vector when any dimension is zero, (b) the expected contiguous walls with a central gap matching `cells_per_tile`.
   - Update existing tests that referenced `query::wall` or compared row counts to accommodate the new bottom border.
   - Extend movement/exit tests so at least one bug walks through the new gap row before being culled (assert the `BugAdvanced` event hits the wall-row cell first, then the exit row).

## 3. Bootstrap & adapters – contract changes [DONE]
### systems/bootstrap (`systems/bootstrap/src/lib.rs`)
- Remove the `wall(&self, world)` accessor. Continue exposing `target(&self, world)` so adapters can read exit data if they need it for UI copy later.

### CLI simulation (`adapters/cli/src/main.rs`)
1. Drop the `--wall-thickness` argument/flag and delete the `wall_thickness` field from `Args`.
2. When building the initial `Scene`, stop constructing a `WallPresentation`. Instead, pick a constant wall color (use the previous default `Color::from_rgb_u8(68, 45, 15)` for continuity) and store it in the scene (see rendering updates below).
3. `Simulation::populate_scene` already pushes cell walls via `SceneWall::new`; no changes needed there once the scene stores the wall color directly.

### Rendering core (`adapters/rendering/src/lib.rs`)
1. Remove `WallPresentation` entirely and replace the `Scene::wall` field with a simple `wall_color: Color` (or `cell_wall_color` for clarity).
2. Update `Scene::new` signature/tests to accept the color instead of a full presentation struct, and adjust `Scene::total_height()` to return `tile_grid.bordered_height()` (the wall thickness now comes from the extra bottom cell layer).
3. Set `TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS` to `1` so bordered height includes the new visible wall row; document the new layout in its comments/tests.

### Macroquad backend (`adapters/rendering_macroquad/src/lib.rs`)
1. Drop `draw_wall(...)` and any code paths that referenced the old `scene.wall`.
2. Update `draw_cell_walls(...)` to fetch the color from `scene.wall_color` (new field) and leave the rest untouched.
3. Simplify `SceneMetrics::from_scene(...)` – `world_height` should now be `scene.tile_grid.bordered_height()`.
4. Refresh adapter tests to reflect the new scene structure (no `WallPresentation`, new height calculation, etc.).

## 4. Documentation & CLI messaging
1. Rewrite the wall section in `README.md` to describe the cell-based wall and remove references to `--wall-thickness`.
2. Update `tile_cell.md` (and any other docs mentioning the perimeter wall) to illustrate the new triple-row layout (playable walkway, visible wall row, hidden exit row).
3. Ensure help output (`--help`) no longer advertises wall thickness.

## 5. Regression tests & golden runs
- Update or add world unit tests verifying the new wall row and the exit gap.
- Run the deterministic replay test suite after adjusting the layout to capture a new golden snapshot (bugs should still exit deterministically).
- Extend adapter tests to assert that the bottom border renders the expected number of rows when `cells_per_tile` varies (so scaling math stays correct).

## 6. Rollout order
1. Land the `core` + `world` structural changes and unit tests so the simulation owns the new data model.
2. Update bootstrap + CLI + rendering crates to consume the new surface (compiling end-to-end).
3. Refresh docs and command-line help.
4. Re-run replay/integration suites and update artifacts if needed.
