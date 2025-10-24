# Maze Defence Architecture

This document explains how the Maze Defence engine is organised so that new contributors can orient themselves quickly, understand how information flows between crates, and identify the correct location for new behaviour. The project is a Cargo workspace composed of small, single-purpose crates that communicate exclusively through explicit messages and immutable snapshots.

## Layered crate layout

The workspace follows a strict four-layer architecture, enforced both socially and through compile-time visibility rules:

| Layer | Crates | Responsibilities |
| --- | --- | --- |
| Core contracts | `core` | Domain types and message contracts such as `Command`, `Event`, `PlayMode`, and read-only views (`BugView`, `NavigationFieldView`, `TowerCooldownView`). No behaviour lives here beyond lightweight helpers; its sole purpose is to define the shapes that other crates share. |
| Authoritative state | `world` | Owns all mutable simulation state (`World`) and exposes the `apply` entry point that executes commands deterministically. Provides read-only query helpers (`world::query`) that project internal state into the view types defined in `core`. |
| Pure systems | `systems/*` | Each sub-crate consumes events and immutable snapshots to emit new commands. Systems never mutate the world directly and never depend on one another. They are responsible for higher-level behaviour such as pathfinding, tower targeting, and spawning. |
| Adapters | `adapters/*` | Integrations with IO surfaces (CLI, renderer, Macroquad backend). Adapters orchestrate systems, forward user input, and render world snapshots. They are the only crates allowed to call into both systems and the world. |

Additional top-level documentation (`*.md` files) captures subsystem specifications and design notes, while the `assets` directory stores sprite data referenced by the rendering adapter.

## Message-driven communication

All cross-layer communication happens through the message contracts defined in `maze_defence_core`:

1. **Adapters emit `Command` values** to request state changes. Examples include `Command::ConfigureTileGrid`, `Command::Tick`, `Command::SpawnBug`, and `Command::PlaceTower`.
2. **`world::apply` executes a single command at a time**, mutating the `World` and pushing resulting `Event` values into a caller-provided buffer. `Command::Tick` triggers most simulation work by advancing time, stepping projectiles, processing exits, and emitting `Event::TimeAdvanced`, `Event::BugAdvanced`, `Event::BugExited`, and tower-related events.
3. **Systems react to events and read-only snapshots**. For example, `systems::movement::Movement::handle` reads a `BugView`, the dense `NavigationFieldView`, occupancy information, and the `ReservationLedgerView` to plan deterministic paths. Systems return new commands that adapters immediately feed back into `world::apply`.
4. **Queries expose immutable state** so adapters and systems can reason about the world without breaking encapsulation. Key helpers include `world::query::play_mode`, `world::query::bug_view`, `world::query::navigation_field`, `world::query::bug_spawners`, and `world::query::is_cell_blocked`.

This loop keeps the simulation deterministic: given an initial world snapshot, a command stream, and an RNG seed, the resulting event sequence and world state are reproducible.

## Simulation lifecycle

The CLI adapter (`adapters/cli`) drives the simulation through the `Simulation` struct. The macroquad backend delivers a per-frame callback that:

1. Collects input, producing `TowerBuilderInput` data for builder interactions and caching cursor-derived placement previews.
2. Flushes any queued commands (typically persisted across frames, e.g., deferred `SetPlayMode` requests) and applies them via `world::apply`.
3. Issues a `Command::Tick` when the frame delta (`dt`) is non-zero, capturing the resulting events.
4. Runs `process_pending_events`, which iteratively:
   * Routes events to auxiliary bookkeeping (bug interpolation, tower feedback).
   * Lets the spawning system (`systems::spawning::Spawning::handle`) emit `Command::SpawnBug` when attack mode is active and the accumulated time exceeds the configured interval.
   * Invokes the movement system (`systems::movement::Movement::handle`) with navigation and occupancy snapshots, emitting `Command::StepBug` for ready bugs while respecting congestion limits and reservations.
   * Refreshes the target list using the tower targeting system (`systems::tower_targeting::TowerTargeting::handle`), then feeds those assignments into the tower combat system (`systems::tower_combat::TowerCombat::handle`) to emit `Command::FireProjectile` for towers whose cooldown snapshots show `ready_in == 0`.
   * Delegates builder interactions to `systems::builder::Builder::handle`, converting preview confirmations and removal gestures into `Command::PlaceTower` or `Command::RemoveTower`.
   * Applies every command immediately, folding resulting events back into the loop until no further events remain.
5. Updates presentation caches (`Scene`, projectile snapshots, interpolated bug positions) using the latest world queries so the renderer can draw deterministic frames.

Because systems work exclusively from snapshots and event streams, the adapter is free to control update ordering without risking hidden mutable sharing between crates.

## World model overview

`maze_defence_world` encapsulates the authoritative state:

* `World` stores the tile grid, bug registry, projectile map, wall layout, tower registry (behind the `tower_scaffolding` feature), navigation field, and reservation ledgers. It also owns bug spawner definitions and the global play mode.
* `world::apply` handles every `Command` variant. Examples include rebuilding the tile grid (`Command::ConfigureTileGrid`), advancing projectile travel during `Command::Tick`, validating placements for `Command::PlaceTower`, and emitting rejection events when removal or placement fails.
* Navigation data lives in `world/src/navigation.rs`, which provides pathfinding utilities that rebuild gradients whenever maze geometry changes. Systems read the resulting `NavigationFieldView` through queries, never the raw buffers.
* Tower-specific state is encapsulated in `world/src/towers.rs`, which tracks placement footprints, cooldown timers, and projectile spawning logic used by `Command::FireProjectile` handlers.

New mutations must always be expressed as commands; direct state changes from outside the world crate are forbidden.

## Systems catalogue

Each system crate focuses on a single responsibility:

* **`systems/bootstrap`** exposes lightweight helper methods (such as `Bootstrap::welcome_banner` and `Bootstrap::tile_grid`) that adapters use during start-up to populate UI state.
* **`systems/builder`** translates builder-mode inputs into placement and removal commands. It listens for `Event::PlayModeChanged` to determine when to accept input and relies on closures that mirror `world::query::tower_at` to identify hovered towers.
* **`systems/movement`** consumes `Event::TimeAdvanced` and navigation snapshots to emit `Command::StepBug`. Its internal `CrowdPlanner` tracks congestion, detour queues, and per-bug reservations so that simultaneous moves remain deterministic.
* **`systems/spawning`** accumulates elapsed time from `Event::TimeAdvanced`, advancing a simple linear congruential generator seeded at boot to choose spawn points and colours. It resets when switching to builder mode, ensuring deterministic waves.
* **`systems/tower_targeting`** reconstructs tower and bug workspaces each frame, deriving the closest eligible target within each tower’s range (computed from `TowerKind::range_in_cells`). Results are sorted deterministically thanks to pre-allocated scratch buffers.
* **`systems/tower_combat`** filters targeting results using `TowerCooldownView`, emitting `Command::FireProjectile` only when the cooldown snapshot reports readiness.

When introducing a new system, mirror this pattern: accept events plus immutable views, emit commands, and keep all scratch state within the struct.

## Adapters and rendering

Adapters provide the IO edges for the engine:

* **`adapters/cli`** wires everything together. It parses command-line options, instantiates the simulation, runs the main loop, and owns presentation state (`Scene`, `BugPresentation`, `TowerInteractionFeedback`). It also exposes utilities like `push_projectiles` and `push_tower_targets` to convert DTOs into rendering primitives.
* **`adapters/rendering`** defines rendering-agnostic DTOs (`Scene`, `Presentation`, `TileGridPresentation`) and transformation helpers that both CLI and engine systems consume.
* **`adapters/rendering_macroquad`** implements the `RenderingBackend` trait against Macroquad, handling window configuration (vsync toggling, FPS overlays) and delegating frame drawing to the DTOs built by the CLI adapter.

Adapter crates are the right place for new input pipelines or presentation layers. They may depend on systems and the world, but they must not introduce their own game logic.

## Deterministic replay and testing

The repository enforces determinism through per-system tests and workspace-wide conventions:

* Each system crate includes targeted unit tests, plus deterministic replay fixtures where applicable (for example, `systems/movement/tests/deterministic_replay.rs`).
* The CI contract requires running `cargo fmt --check`, `cargo clippy --deny warnings`, `cargo udeps`, `cargo hack check --each-feature`, and deterministic replay tests. Contributors should mirror these checks locally before opening a PR.
* Snapshot helpers in `adapters/cli` expose hooks like `Simulation::capture_layout_snapshot` so tests can assert encoded layouts, ensuring UI-visible behaviour stays reproducible.

When adding new behaviour, include message-level tests that drive the world through `world::apply` and assert on emitted events or query results.

## Extending the engine

To add a new component:

1. Decide whether it belongs in a system (pure logic reacting to events and snapshots) or an adapter (input/output). If it requires direct state mutation, add a new `Command` and extend `world::apply` accordingly.
2. If introducing a system, create a new crate under `systems/`, define a struct with `handle` methods that accept `&[Event]` plus the necessary query views, and register it in the adapter loop alongside existing systems.
3. Extend `maze_defence_core` with any new message types or DTOs, ensuring they remain serialisable and documented.
4. Provide deterministic tests that exercise the new behaviour through commands and events.

Strictly avoid cross-system imports or world mutations outside `world::apply`; the workspace is designed to make such patterns impossible.

## Workspace layout

```
├── AGENTS.md
├── Cargo.lock
├── Cargo.toml
├── README.md
├── adapters
│   ├── cli
│   │   ├── Cargo.toml
│   │   └── src
│   ├── rendering
│   │   ├── Cargo.toml
│   │   └── src
│   └── rendering_macroquad
│       ├── Cargo.toml
│       └── src
├── assets
│   ├── README.md
│   ├── manifest.toml
│   └── sprites
│       ├── README.md
│       ├── bugs
│       ├── ground
│       └── towers
├── build-mode.md
├── core
│   ├── Cargo.toml
│   └── src
│       └── lib.rs
├── movement.md
├── path-impl.md
├── path-spec.md
├── sprite-impl.md
├── sprite-spec.md
├── systems
│   ├── bootstrap
│   │   ├── Cargo.toml
│   │   └── src
│   ├── builder
│   │   ├── Cargo.toml
│   │   ├── src
│   │   └── tests
│   ├── movement
│   │   ├── Cargo.toml
│   │   ├── src
│   │   └── tests
│   ├── spawning
│   │   ├── Cargo.toml
│   │   ├── src
│   │   └── tests
│   ├── tower_combat
│   │   ├── Cargo.toml
│   │   └── src
│   └── tower_targeting
│       ├── Cargo.toml
│       ├── src
│       └── tests
├── target-impl.md
├── target-spec.md
├── tests
├── tile_cell.md
├── tower-impl.md
├── tower-shooting-impl.md
├── tower-shooting-spec.md
├── tower-spec.md
├── walls.md
└── world
    ├── Cargo.toml
    └── src
        ├── lib.rs
        ├── navigation.rs
        └── towers.rs
```

