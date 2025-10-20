# Maze Defence

A tile-based defence prototype rendered with Macroquad. This repository exposes a command-line adapter that boots the experience and allows you to tweak the play field layout when launching the game.

## Running the game

Use Cargo to start the CLI adapter from the workspace root:

```bash
cargo run --bin maze-defence
```

By default the grid measures **10×10 tiles**, the surrounding wall thickness is **40 pixels**, and bugs attempt a step every **250 milliseconds** (roughly four moves per second).

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

## Adjusting bug speed

Bugs sprint toward the wall opening every 250 milliseconds by default. Use the `--bug-step-ms` flag to control the interval between their moves:

```bash
cargo run --bin maze-defence -- --bug-step-ms 400
```

Larger values slow the swarm down while smaller numbers make them more aggressive. The flag accepts any value between 1 and 60,000 milliseconds.
