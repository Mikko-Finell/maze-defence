# Maze Defence

A tile-based defence prototype rendered with Macroquad. This repository exposes a command-line adapter that boots the experience and allows you to tweak the play field layout when launching the game.

## Running the game

Use Cargo to start the CLI adapter from the workspace root:

```bash
cargo run --bin maze-defence
```

By default the grid measures **10×10 tiles**, the surrounding wall thickness is **40 pixels**, four cell divisions are drawn inside each tile, and bugs attempt a step every **250 milliseconds** while new bugs spawn every **1,000 milliseconds**.

All flags must be passed after the `--` separator so that Cargo forwards them to the game binary.

## Command-line options

The CLI exposes the following arguments:

| Flag | Description | Default |
| ---- | ----------- | ------- |
| `-s`, `--size WIDTHxHEIGHT` | Sets both tile dimensions at once (for example `12x18`). Conflicts with `--width`/`--height`. | `10x10` |
| `--width COLUMNS` | Overrides the number of tile columns. Requires `--height` so the grid stays rectangular. | `10` |
| `--height ROWS` | Overrides the number of tile rows. Requires `--width`. | `10` |
| `--wall-thickness PIXELS` | Controls the thickness of the perimeter wall. | `40` |
| `--cells-per-tile COUNT` | Chooses how many sub-cells are rendered inside each tile edge. Must be at least `1`. | `4` |
| `--bug-step-ms MILLISECONDS` | Sets how long each bug waits before taking another step. Accepts values from `1` to `60_000`. | `250` |
| `--bug-spawn-interval-ms MILLISECONDS` | Controls the interval between automatic spawns while in attack mode. Accepts values from `1` to `60_000`. | `1_000` |
| `--vsync on\|off` | Requests enabling (`on`) or disabling (`off`) vertical sync. | Platform default |

## Configuring the grid size

You can control the number of tiles in the grid using either a compact `WIDTHxHEIGHT` argument or explicit dimensions:

```bash
# 12 columns by 18 rows using the compact syntax
cargo run --bin maze-defence -- --size 12x18

# 20 columns by 15 rows using explicit flags
cargo run --bin maze-defence -- --width 20 --height 15
```

If no size is supplied the game falls back to the default 10×10 layout. The `--width` and `--height` flags must always be specified together.

## Adjusting the wall thickness

The perimeter wall defaults to a 40 pixel thickness. Override it with the `--wall-thickness` flag:

```bash
cargo run --bin maze-defence -- --wall-thickness 64
```

Combine this flag with either of the grid size options to customise the scene at launch.

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
