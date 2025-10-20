# Tiles vs. Cells in Maze Defence

## Tile space versus cell space
The `TileGrid` recorded by the world keeps the coarse maze layout—tile columns, tile rows, tile edge length, and the number of visual cells per tile edge—using the `TileCoord` wrapper so adapters talk in whole tiles.【F:world/src/lib.rs†L99-L162】 When a grid is configured the world derives a `CellGeometry` from that tile description. The helper multiplies the tile dimensions by `cells_per_tile`, adds a one-cell rim on the top, left, and right sides, and records a wall opening whose width equals `cells_per_tile`.【F:world/src/lib.rs†L24-L91】

## How many cells really exist
Actual gameplay happens entirely in the dense cell grid produced by `CellGeometry`. `World::new` and `apply` rebuild the occupancy grid with `columns = tile_columns × cells_per_tile + 2` (for the side rim) and `rows = tile_rows × cells_per_tile + 1` (for the top rim). The wall opening is represented by `cells_per_tile` extra cells centred along the bottom edge; those cells live at `row == rows` and do not occupy the dense grid itself.【F:world/src/lib.rs†L255-L383】【F:world/src/lib.rs†L718-L770】 Bugs are seeded across every interior and rim cell of that occupancy buffer, with capacity capped at `available_cells − 1` so at least one spot remains open.【F:world/src/lib.rs†L772-L813】

For example, running `cargo run -p maze-defence-cli --bin maze-defence -- --size 21x30` with the default CLI `--cells-per-tile 4` configures the world with 21×30 tiles. The interior expands to `21 × 4 = 84` cell columns and `30 × 4 = 120` cell rows. Adding the side rim yields 86 cell columns; adding the top rim yields 121 occupied rows. The wall opening is centred, so the four exit cells sit in row 121 at columns 41 through 44 (zero-based), matching the tile’s breadth.【F:adapters/cli/src/main.rs†L51-L175】【F:world/src/lib.rs†L36-L85】

## Movement operates on cells
Every bug snapshot, reservation, and movement proposal is stored in `CellCoord`. The movement system asks the occupancy view for its dimensions, plans A* paths across that cell grid, and only permits entering the wall opening when the destination column matches the target range reported by the world.【F:systems/movement/src/lib.rs†L29-L187】【F:world/src/lib.rs†L288-L356】【F:world/src/lib.rs†L729-L770】 Bugs stepping into any of the exit cells are removed from the dense grid so the opening stays free for others.【F:world/src/lib.rs†L339-L356】

In short, tiles define physical scale, but cell geometry—scaled by `cells_per_tile`, wrapped with the outer rim, and extended through the wall opening—drives every authoritative decision.
