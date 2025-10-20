# Tiles vs. Cells in Maze Defence

## Data model difference
The world owns a `TileGrid` that tracks how many whole tiles wide and tall the maze is, the physical edge length of each tile, and how many logical cells subdivide every tile edge. The structure also reports the playable cell rectangle (tiles × cells-per-tile plus the left/right and top rims) and the extra exit corridor carved beneath the wall so adapters can size their renders and systems can reason about boundaries.【F:world/src/lib.rs†L32-L149】【F:core/src/lib.rs†L107-L191】

Bugs, wall openings, and path-finding never talk in tiles. They use `CellCoord`, a pair of column/row indices that pinpoints an individual traversable cell inside that expanded grid. The `World` sizes its occupancy buffer to the playable rectangle, seeds bugs inside the interior (skipping the rim), and exposes the wall opening through the rows reported by `TileGrid::exit_row_range`.【F:core/src/lib.rs†L107-L191】【F:world/src/lib.rs†L151-L815】

In short: tiles describe the coarse layout and physical sizing of the maze, while cells are the discrete navigation nodes used by gameplay logic.

## What bugs use for movement
Both the authoritative world state and the movement system store, plan, and validate bug motion with `CellCoord` values. Each bug records its current cell, movement proposals resolve to a destination cell, and the A* planner iterates over neighboring cells (including the synthetic exit row) before emitting a one-cell `StepBug` command. No movement code operates on `TileCoord` directly.【F:world/src/lib.rs†L467-L815】【F:systems/movement/src/lib.rs†L22-L326】

## Meaning of `cargo run -p maze-defence-cli --bin maze-defence -- --size 21x30`
Passing `--size 21x30` makes the CLI parse `21` columns by `30` rows and feed those exact dimensions—and the chosen `--cells-per-tile` subdivision count—into `Simulation::new`, which configures the world’s tile grid accordingly.【F:adapters/cli/src/main.rs†L35-L214】

That configuration produces:

- **Tiles:** `21 × 30 = 630` square tiles in the interior grid, because the tile grid dimensions come directly from the parsed width and height.【F:world/src/lib.rs†L32-L149】
- **Cells:** Let the CLI `--cells-per-tile` value be `C` (defaults to `4`). The playable cell rectangle spans `(21 × C + 2)` columns by `(30 × C + 1)` rows—the extra two columns form the left/right rim (including the top corners) and the extra row is the top rim. Beneath that, the world exposes `C` additional rows for the exit corridor, so bugs, the occupancy grid, and the path-finder all operate on those coordinates.【F:adapters/cli/src/main.rs†L106-L214】【F:world/src/lib.rs†L245-L815】【F:systems/movement/src/lib.rs†L22-L326】

## How bugs approach the wall opening
The world exposes the wall target as the full exit corridor returned by `TileGrid::exit_row_range`, and the movement system allows a bug to step from the last playable row into the first exit row whenever its column lies within `TileGrid::exit_columns_range`. Bugs therefore plan and take their final step toward the opening entirely in cell space; tiles only provide the coarse grid dimensions needed to size that cell graph.【F:world/src/lib.rs†L702-L815】【F:systems/movement/src/lib.rs†L201-L326】
