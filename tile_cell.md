# Tiles vs. Cells in Maze Defence

## Data model difference
The world still describes its coarse layout in whole tiles via `TileGrid`, but the grid
configuration command carries a `cells_per_tile` factor so each tile expands into
`cells_per_tile × cells_per_tile` navigation cells. Helper routines compute dense occupancy
dimensions by multiplying the tile counts and then appending side borders plus three extra
southern layers that represent the walkway, the visible wall, and the hidden exit row.【F:world/src/lib.rs†L1049-L1076】

Those row indices are exposed through dedicated helpers: `exit_row_for_tile_grid(...)` pins the
hidden row where bugs are culled, `visible_wall_row_for_tile_grid(...)` identifies the rendered
wall band, and the test-only `walkway_row_for_tile_grid(...)` confirms the playable strip that
sits immediately above the wall.【F:world/src/lib.rs†L1078-L1116】

`World::apply` normalises incoming grid changes (rejecting zero subdivisions), rebuilds the
target cells, resizes the dense occupancy buffer using the derived dimensions, recreates the
wall cells, and refreshes the bug spawner rim so the new triple-row layout is consistent.【F:world/src/lib.rs†L253-L366】
Bug spawning logic only seeds insects into interior coordinates, keeping them out of the wall
and exit rows until they march there organically.【F:world/src/lib.rs†L775-L820】

## What bugs use for movement
Bugs, targets, and reservations are all stored in `CellCoord`s. The world checks proposed steps
against the expanded occupancy grid, filters out moves that collide with the rebuilt wall cells,
and vacates bugs once they enter the hidden exit coordinates.【F:world/src/lib.rs†L140-L244】【F:world/src/lib.rs†L1118-L1140】
The movement system consumes the same grid dimensions from `OccupancyView`, prepares workspaces
that match the world’s rows, and enumerates neighbours with guards that respect the explicit
exit columns so deterministic path-finding and authoritative movement stay aligned.【F:systems/movement/src/lib.rs†L57-L176】【F:systems/movement/src/lib.rs†L216-L330】

## How CLI configuration maps to actual cells
The CLI forwards `--cells-per-tile` directly into `Command::ConfigureTileGrid`, ensuring gameplay
and rendering share the same subdivision. With the default `--cells-per-tile 4`, running
`cargo run -p maze-defence-cli --bin maze-defence -- --size 21x30` produces:

- **Interior cells:** `21 × 4 = 84` columns and `30 × 4 = 120` rows.
- **Walkway row:** the final interior row at index `120`, immediately above the wall.
- **Visible wall row:** index `121`, filled with wall cells except for the exit gap.
- **Hidden exit row:** index `122`, where bugs take their last step before being culled.
- **Exit columns:** four contiguous cells centred on the middle tile.

Those counts come straight from the helper calculations (`interior_cell_rows`, `total_cell_rows`,
`walkway_row_for_tile_grid`, `visible_wall_row_for_tile_grid`, and `exit_columns_for_tile_grid`), so every
bug path operates on a dense grid aligned to the maze interior with the tile-width opening.【F:adapters/cli/src/main.rs†L142-L215】【F:world/src/lib.rs†L1049-L1116】

## How bugs approach the wall opening
`build_cell_walls(...)` populates the visible wall row while skipping the exit gap reported by
`exit_columns_for_tile_grid(...)`, and `target_cells(...)` constructs the contiguous target band in the
hidden exit row. Movement queries clone those targets, choose the nearest candidate, and their
neighbour enumeration only allows the southward step when the bug is aligned with one of those
exit columns, ensuring each bug pauses on the walkway row before being removed from the world.【F:world/src/lib.rs†L1118-L1174】【F:systems/movement/src/lib.rs†L96-L176】
