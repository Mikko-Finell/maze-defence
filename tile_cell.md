# Tiles vs. Cells in Maze Defence

## Data model difference
The world owns a `TileGrid` that stores how many whole tiles wide and tall the maze is, how many navigation cells each tile is subdivided into, and the physical edge length of each tile; all of those fields stay in whole-tile units via `TileCoord` or `NonZeroU32` wrappers.【F:world/src/lib.rs†L26-L132】【F:core/src/lib.rs†L175-L191】

Bugs, wall openings, and path-finding never talk in tiles. They use `CellCoord`, a pair of column/row indices that pinpoints an individual traversable cell inside that tile grid (with the row immediately below the interior reserved for the exit corridor). The `World` keeps bug positions, target cells, and the dense occupancy buffer entirely in these cell coordinates.【F:core/src/lib.rs†L105-L173】【F:world/src/lib.rs†L134-L606】

In short: tiles describe the coarse layout and physical sizing of the maze, while cells are the discrete navigation nodes used by gameplay logic.

## What bugs use for movement
Both the authoritative world state and the movement system store, plan, and validate bug motion with `CellCoord` values. Each bug records its current cell, movement proposals resolve to a destination cell, and the A* planner iterates over neighboring cells before emitting a one-cell `StepBug` command. No movement code operates on `TileCoord` directly.【F:world/src/lib.rs†L467-L490】【F:systems/movement/src/lib.rs†L29-L326】

## Meaning of `cargo run -p maze-defence-cli --bin maze-defence -- --size 21x30`
Passing `--size 21x30` makes the CLI parse `21` columns by `30` rows and feed those exact dimensions, along with the `--cells-per-tile` value (defaulting to `4`), into `Simulation::new`, which configures the world’s tile grid with `TileCoord::new(21)`, `TileCoord::new(30)`, and `NonZeroU32::new(4)`.【F:adapters/cli/src/main.rs†L34-L194】【F:core/src/lib.rs†L175-L191】

That configuration produces:

- **Tiles:** `21 × 30 = 630` square tiles in the interior grid, because the tile grid dimensions come directly from the parsed width and height.【F:world/src/lib.rs†L26-L132】
- **Cells:** Each tile is subdivided into `4 × 4` navigation cells. The authoritative grid therefore spans `21 × 4 = 84` interior columns and `30 × 4 = 120` interior rows, plus a one-cell rim on the left, right, and top edges, yielding `86 × 121 = 10 406` addressable cells in the occupancy buffer. The wall opening exposes an additional strip of `4` exit cells directly below the bottom interior row so bugs can step through the wall.【F:world/src/lib.rs†L62-L133】【F:world/src/lib.rs†L296-L357】【F:world/src/lib.rs†L588-L719】

## How bugs approach the wall opening
The world exposes the wall target as cell coordinates, and the movement system expands its neighbor list with every exit cell when a bug stands on the interior row just above the wall. Bugs therefore plan and take their final step toward the opening entirely in cell space; tiles only provide the coarse grid dimensions needed to size that cell graph.【F:world/src/lib.rs†L588-L719】【F:systems/movement/src/lib.rs†L214-L332】
