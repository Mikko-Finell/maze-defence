# Maze Defence

A tile-based defence prototype rendered with Macroquad. This repository exposes a command-line adapter that boots the experience and allows you to tweak the play field layout when launching the game.

## Running the game

Use Cargo to start the CLI adapter from the workspace root:

```bash
cargo run --bin maze-defence
```

By default the grid measures **10×10 tiles** and the surrounding wall thickness is **40 pixels**.

## Configuring the grid size

You can control the number of tiles in the grid using either a compact `WIDTHxHEIGHT` argument or explicit dimensions:

```bash
# 12 columns by 18 rows using the compact syntax
cargo run --bin maze-defence -- --size 12x18

# 20 columns by 15 rows using explicit flags
cargo run --bin maze-defence -- --width 20 --height 15
```

If no size is supplied the game falls back to the default 10×10 layout.

## Adjusting the wall thickness

The perimeter wall defaults to a 40 pixel thickness. Override it with the `--wall-thickness` flag:

```bash
cargo run --bin maze-defence -- --wall-thickness 64
```

Combine this flag with either of the grid size options to customise the scene at launch.
