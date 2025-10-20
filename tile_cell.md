# Tiles vs. Cells in Maze Defence

## Data model difference
The world still describes its coarse layout in whole tiles via `TileGrid`, but the grid
configuration command now also carries a `cells_per_tile` factor. The world remembers
that value, expands every tile into `cells_per_tile × cells_per_tile` navigation cells,
and then adds a single walkable layer on the left, top, and right sides. The helper
routines in the world crate derive the occupancy dimensions
`columns * cells_per_tile + 2` by `rows * cells_per_tile + 1` and compute the exit row at
`rows * cells_per_tile + 1` so the opening still sits just outside the bottom wall.【F:core/src/lib.rs†L19-L44】【F:world/src/lib.rs†L176-L208】【F:world/src/lib.rs†L714-L780】

`World::apply` normalises the incoming value (rejecting zero), rebuilds the wall target, and
resizes the dense occupancy buffer using those derived dimensions before regenerating bugs.
The bug seeding logic only places bugs inside the interior cells that make up the playable maze.【F:world/src/lib.rs†L289-L337】【F:world/src/lib.rs†L714-L780】

## What bugs use for movement
Bugs, targets, and reservations are all stored in `CellCoord`s. The world checks potential
steps against the expanded occupancy grid and only lets a bug move south out of the maze
when its column matches one of the exit cells. The movement system consumes the same
dimensions from `OccupancyView`, adds an extra virtual row for the exit, and enumerates
neighbours with the same guard so path-finding and authoritative movement agree.【F:world/src/lib.rs†L213-L269】【F:world/src/lib.rs†L682-L717】【F:systems/movement/src/lib.rs†L29-L200】【F:systems/movement/src/lib.rs†L234-L330】

## How CLI configuration maps to actual cells
The CLI now forwards the `--cells-per-tile` argument directly into
`Command::ConfigureTileGrid`, so gameplay and rendering use the same subdivision. With the
default `--cells-per-tile 4`, running
`cargo run -p maze-defence-cli --bin maze-defence -- --size 21x30` produces:

- **Interior cells:** `21 × 4 = 84` columns and `30 × 4 = 120` rows inside the maze.
- **Walkable border:** `+2` columns (left/right) and `+1` row (top) surrounding the maze,
  leading to an occupancy buffer sized `86 × 121`.
- **Wall opening:** four contiguous exit cells at row index `121` (zero-based) centred on the
  middle tile.

Those counts come straight from the helper calculations and the multi-cell target builder,
so every bug path operates on a dense grid aligned to the maze interior with the tile-width opening.【F:adapters/cli/src/main.rs†L108-L198】【F:world/src/lib.rs†L714-L780】【F:world/src/lib.rs†L682-L717】

## How bugs approach the wall opening
`Target::aligned_with_grid` constructs `cells_per_tile` contiguous target cells positioned in the
exit row just outside the bottom wall. Movement queries clone those cells, choose the nearest
candidate, and the neighbour enumeration allows the final southward step only when the bug is
aligned with one of those exit columns. The target row is not part of the occupancy grid, so once a bug
steps into one of those cells it vacates the maze entirely.【F:world/src/lib.rs†L101-L140】【F:world/src/lib.rs†L682-L717】【F:systems/movement/src/lib.rs†L29-L200】【F:systems/movement/src/lib.rs†L234-L330】
