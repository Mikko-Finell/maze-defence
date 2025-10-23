# Maze Defence

A tile-based defence prototype rendered with Macroquad. This repository exposes a command-line adapter that boots the experience and allows you to tweak the play field layout when launching the game.

## Running the game

Use Cargo to start the CLI adapter from the workspace root:

```bash
cargo run --bin maze-defence
```

By default the grid measures **10×10 tiles**, each tile is subdivided into **four cells per edge**, and the world synthesises a dedicated perimeter wall row so bugs march across a walkway before entering a hidden exit row. Bugs attempt a step every **250 milliseconds** while new bugs spawn every **1,000 milliseconds**.

All flags must be passed after the `--` separator so that Cargo forwards them to the game binary.

## Command-line options

The CLI exposes the following arguments:

| Flag | Description | Default |
| ---- | ----------- | ------- |
| `-s`, `--size WIDTHxHEIGHT` | Sets both tile dimensions at once (for example `12x18`). Conflicts with `--width`/`--height`. | `10x10` |
| `--width COLUMNS` | Overrides the number of tile columns. Requires `--height` so the grid stays rectangular. | `10` |
| `--height ROWS` | Overrides the number of tile rows. Requires `--width`. | `10` |
| `--cells-per-tile COUNT` | Chooses how many sub-cells are rendered inside each tile edge. Must be at least `1`. | `4` |
| `--bug-step-ms MILLISECONDS` | Sets how long each bug waits before taking another step. Accepts values from `1` to `60_000`. | `250` |
| `--bug-spawn-interval-ms MILLISECONDS` | Controls the interval between automatic spawns while in attack mode. Accepts values from `1` to `60_000`. | `1_000` |
| `--vsync on\|off` | Requests enabling (`on`) or disabling (`off`) vertical sync. | Platform default |
| `--layout LAYOUT` | Restores a serialized tower layout before launching the renderer. | None |
| `--show-fps on\|off` | Prints per-second frame timing metrics to stdout when set to `on`. | `off` |

## Configuring the grid size

You can control the number of tiles in the grid using either a compact `WIDTHxHEIGHT` argument or explicit dimensions:

```bash
# 12 columns by 18 rows using the compact syntax
cargo run --bin maze-defence -- --size 12x18

# 20 columns by 15 rows using explicit flags
cargo run --bin maze-defence -- --width 20 --height 15
```

If no size is supplied the game falls back to the default 10×10 layout. The `--width` and `--height` flags must always be specified together.

## Understanding the perimeter wall

Maze Defence no longer scales the wall by pixel thickness. The world instead expands its occupancy grid with three extra rows: a playable walkway directly below the interior tiles, a visible wall row, and a hidden exit row that consumes culled bugs. `total_cell_rows(...)` accounts for all three layers, `visible_wall_row_for_tile_grid(...)` selects which row is rendered as walls, and the test-only helper `walkway_row_for_tile_grid(...)` shows that the walkway remains a playable strip just above that wall.【F:world/src/lib.rs†L1048-L1098】【F:world/src/lib.rs†L1099-L1129】

When the walls are rebuilt, `build_cell_walls(...)` fills every column on the visible row except for the exit gap reported by `exit_columns_for_tile_grid(...)`, ensuring the walkway funnels bugs into the precise opening derived from `cells_per_tile`. Rendering mirrors this layout: `TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS` documents that the bottom border corresponds to the visible wall row so the on-screen height matches the world geometry.【F:world/src/lib.rs†L1118-L1150】【F:adapters/rendering/src/lib.rs†L310-L339】

## Tuning tile rendering detail

Use `--cells-per-tile` to change how many sub-cells the renderer draws along each tile edge. Higher values increase the amount of visual detail when zoomed in, while lower values provide a blockier look and can help when experimenting on slower machines:

```bash
cargo run --bin maze-defence -- --cells-per-tile 6
```

Values must be whole numbers greater than or equal to one.

## Adjusting bug speed

Bugs sprint toward the wall opening every 250 milliseconds by default. Use the `--bug-step-ms` flag to control the interval between their moves:

```bash
cargo run --bin maze-defence -- --bug-step-ms 400
```

Larger values slow the swarm down while smaller numbers make them more aggressive. The flag accepts any value between 1 and 60,000 milliseconds.

## Controlling bug spawn cadence

Attack mode also spawns new bugs at a fixed cadence. Adjust the interval with `--bug-spawn-interval-ms` to make the waves denser or sparser:

```bash
cargo run --bin maze-defence -- --bug-spawn-interval-ms 2000
```

The accepted range matches `--bug-step-ms` — anything between 1 and 60,000 milliseconds is valid.

## Toggling vertical sync

The renderer requests the platform's default swap interval when no flag is provided. Use `--vsync off` to disable vertical sync and measure raw rendering throughput, or `--vsync on` to explicitly request synchronisation with the display refresh rate:

```bash
cargo run --bin maze-defence -- --vsync off
```
## Displaying frame timing metrics

Enable `--show-fps on` to log per-second frame timing breakdowns to the terminal. This keeps the output silent by default while still making it easy to monitor simulation and rendering performance when needed:

```bash
cargo run --bin maze-defence -- --show-fps on
```

## Sharing layouts via the clipboard

* Provide a layout string with `--layout` to rebuild the maze before the first frame renders. The simulation validates the
  payload, rebuilds the maze, and the `CxR` segment overrides any CLI grid sizing so the snapshot's dimensions always win.
  【F:adapters/cli/src/main.rs†L253-L293】
* Layout strings begin with `maze:v2:CxR` and carry a URL-safe base64 payload containing varint-encoded grid metadata and
  tower records; legacy `maze:v1` JSON payloads remain accepted for backwards compatibility. Share the full string
  (including the prefix) to reliably reproduce a layout. 【F:adapters/cli/src/layout_transfer.rs†L38-L249】
* Entering or leaving build mode automatically prints the latest layout snapshot to stdout, making it easy to capture
  incremental edits without relying on the clipboard. 【F:adapters/cli/src/main.rs†L702-L714】
* Whenever the process exits it prints the most recent snapshot so you can recover the layout after a run. 【F:adapters/cli/src/main.rs†L1270-L1272】
