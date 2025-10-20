# Movement and Pathing Implementation Plan

## Goals and Constraints
- Enable bugs to traverse the maze grid toward the wall target using four-directional (N/E/S/W) moves on grid cells.
- Maintain deterministic behaviour that respects the architectural rules in `AGENTS.md` (pure systems, single mutation point in the world, message-based coordination).
- Ensure multiple bugs can move concurrently without overlapping; re-plan paths whenever the intended next cell becomes occupied.
- Keep execution performant for many bugs by minimising redundant path computations and avoiding unnecessary allocations.

## Message Surface Changes (`core` crate)
1. [DONE] Extend `Command` with discrete mutations instead of embedding movement logic inside systems:
   - `Command::Tick { dt: Duration }` – issued by adapters each frame to request that the world advance the simulation clock.
   - `Command::SetBugPath { bug_id: BugId, path: Vec<CellCoord> }` – authoritative path assignment emitted by the pure movement system using the fine-grained cell lattice inside each tile.
   - `Command::StepBug { bug_id: BugId, direction: Direction }` – proposals for one-cell moves emitted after a tick makes a bug ready to advance.
2. [DONE] Introduce a new `Event` enum so the world can broadcast state transitions for systems to react to deterministically:
   - `Event::TimeAdvanced { dt: Duration }` emitted from `World::apply` after handling `Command::Tick`, preserving the “command in, event out” contract.
   - `Event::BugPathNeeded { bug_id: BugId }` whenever the world requires a new path (e.g., spawn, completed path, or failed reservation).
   - `Event::BugAdvanced { bug_id: BugId, from: CellCoord, to: CellCoord }` emitted after the world accepts a `StepBug` command during reservation resolution.
3. [DONE] Document the message contracts and update adapter/system wiring so adapters only send commands, the world only mutates through `apply`, and systems consume event streams while returning command batches.

## World State Authoritative Changes (`world` crate)
1. [DONE] Replace the immutable `Bug` data with a stateful struct tracking navigation:
   - Keep immutable presentation fields (`id`, `color`).
   - Store current cell, optional queued path (`VecDeque<CellCoord>`), and accumulated fractional time toward the next step.
2. [DONE] Maintain an occupancy map for fast lookup:
   - Use a dense `Vec<Option<BugId>>` sized to the grid (`width * height`) for cache-friendly O(1) membership.
   - Provide helper methods for translating `(row, column)` to indices; keep a `HashSet` fallback only if extremely sparse maps emerge in future use-cases.
   - Update occupancy atomically when bugs move.
3. [DONE] Track wall target cells as `CellCoord` equivalents:
   - Convert `TargetCell` values into traversable `CellCoord` nodes, treating the target row (`rows`) as a valid exit cell.
   - Maintain adjacency information to connect interior edge cells to the target cell so A* can path to it.
4. [DONE] Update `apply` logic:
   - `Tick` accumulates time on each bug and emits `Event::TimeAdvanced { dt }`; when a bug accrues at least one-second quantum and lacks a queued hop, emit `Event::BugPathNeeded` to request planning.
   - `SetBugPath` replaces the queued path (validating first hop against current position) and clears any stale progress.
   - `StepBug` commands enter a **reservation phase**: collect all requests for the tick, sort by `BugId`, and for each verify direction matches queued path. If the destination cell is free in the dense occupancy buffer, commit the move, update occupancy, subtract one second from the accumulator, and emit `Event::BugAdvanced`. If the cell is already reserved or occupied, mark the bug as needing a path refresh and emit `Event::BugPathNeeded` for that bug without advancing.
5. [DONE] Expose new queries for systems:
   - `query::bug_view(world) -> BugView` returning read-only DTOs that contain bug ids, cells, queued path heads, and readiness flags.
   - `query::occupancy_view(world) -> OccupancyView` with immutable access to the dense grid buffer (and helper iterators for free target cells).
   - Helper query to expose free target cells (`query::available_target_cells`).
6. [DONE] Ensure determinism:
   - Use fixed ordering when iterating bugs (e.g., ascending `BugId`).
   - Avoid floating-point drift by storing time as `FixedU32` microseconds or `Duration` accumulators and subtract exactly one-second quanta.

## Movement System (`systems/movement` crate)
1. [DONE] Create a new pure system crate that listens for events and produces commands:
   - Public API: `Movement::handle(events: &[Event], bug_view: BugView, occupancy_view: OccupancyView, targets: &[CellCoord], out: &mut Vec<Command>)` – systems receive DTOs, never the world itself.
   - Respond to `Event::TimeAdvanced` by scanning `bug_view` for bugs whose readiness flag is true (one-second quantum accrued).
   - For each bug needing a path (via `Event::BugPathNeeded` or DTO flag), compute the best path and issue `SetBugPath` command.
   - When a bug is ready to step and has a path, propose `StepBug` commands for the next direction, but only if the destination cell appears free in `occupancy_view`. Losers in the reservation phase will cause the world to emit a new `Event::BugPathNeeded`.
2. [DONE] Keep computation efficient:
   - Cache wall target cells once per handler call.
   - A* pathfinding on a per-bug basis using `BinaryHeap` for the frontier, Manhattan heuristic, and hashing grid coordinates.
   - Re-use allocation buffers (e.g., `Vec<CellCoord>`) via scratch workspace passed into helper functions to avoid repeated allocations on each tick.
3. [DONE] Respect non-overlap rule and reservation outcomes:
   - Before emitting `StepBug`, consult `occupancy_view` to ensure target cell is free (ignoring the bug's current cell).
   - Systems do not loop on failed reservations; they rely on the subsequent `Event::BugPathNeeded` to trigger fresh planning.
4. [DONE] Determine the nearest target cell:
   - Cache wall target cells derived from `Target::cells()` once the workspace dimensions are known.
   - Break ties deterministically (lowest manhattan distance, then lowest column/row) to maintain reproducibility while allowing
     exit cells to remain always enterable.

## Pathfinding Implementation Details
1. [DONE] Grid Model:
   - Graph nodes are `CellCoord` values inside the maze plus the `Target` exit nodes.
   - Legal moves: four directions; when on the interior cell adjacent to the target, allow moving into the target node.
2. [DONE] A* Mechanics:
   - Reconstruct the final path using a reusable `Vec<CellCoord>` buffer to avoid repeated allocations.
   - Occupancy map excludes the moving bug’s current cell; treat other bugs as static obstacles during the search.
   - Abort and return empty path when no exit cell reachable.
3. [DONE] Performance Considerations:
   - Limit search bounds to the grid plus the single extra row of exit cells.
   - Pre-allocate visited map sized to `columns * (rows + 1)`.
   - Avoid heap churn during neighbour enumeration by using a fixed-size stack buffer (`NeighborIter`).

## Tests (`tests/` harness)
1. [DONE] Add deterministic replay test covering multiple ticks:
   - Scripted commands: configure grid, emit `Command::Tick` pulses, and pass resulting events through the movement system using captured DTO snapshots.
   - Assert final positions hash equals expected snapshot and document that, given `(initial world + ordered commands + fixed RNG seed)`, replay yields **bit-identical** state/events enforced by CI’s deterministic replay test.
2. Scenario tests:
   - Two bugs starting on same column path toward exit verifying they queue without overlap.
   - Bug encountering newly occupied cell triggers replanning (simulate by positioning another bug in its path before next tick).
   - Validation that the nearest available target cell is selected when one is already reserved.
3. Unit tests:
   - A* pathfinding returns Manhattan-shortest path on simple grids and respects obstacles.
   - `World::apply` rejects invalid `StepBug` commands (wrong direction or occupied target).
4. Ensure tests avoid floating-point nondeterminism by using integer durations and verifying states via queries only.

## Adapter and Bootstrap Considerations
- Update runners/adapters to emit `Command::Tick { dt }` each frame, call `world.apply()` once per tick, capture the resulting DTO queries (`BugView`, `OccupancyView`), and feed those immutable snapshots into the movement system before submitting any follow-up commands.
- Extend bootstrap queries if presentation requires bug velocity/path information (e.g., expose next hop or progress for rendering interpolation).

## Rollout Steps Summary
1. [DONE] Update `core` crate with new commands/events/direction enum + docs.
2. [DONE] Refactor `world` crate bug representation, occupancy tracking, and `apply` logic; add queries.
3. [DONE] Implement A* utilities (likely under `world::navigation` or shared helper module) with tests.
4. [DONE] Build `systems/movement` crate for deterministic path planning and command emission.
5. [DONE] Wire movement system into existing execution path (tests/adapters).
   - Added a CLI simulation driver that pumps world ticks through the movement system before updating presentation state.
   - Extended the rendering adapter contract so backends refresh the scene each frame, allowing playtesters to observe live bug movement.
6. Author comprehensive tests ensuring deterministic, collision-free behaviour for multi-bug scenarios.
