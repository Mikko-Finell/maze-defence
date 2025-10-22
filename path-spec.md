# Pathfinding Crowd Movement Spec

## Problem Statement
Bugs currently compute a full path to the exit each time they are ready to move. If any tile along that path is occupied by another bug, the search treats the route as blocked, returns `None`, and the caller refuses to move. As a result a bug can stall even though the next tile in front of it is empty. That stall cascades backwards and freezes entire columns of bugs despite there being free cells immediately ahead. We need a crowd-aware planner that only halts when the immediate ring of cells is blocked and that can route around temporary jams.

## Success Criteria
- A bug stops only when none of the four neighbouring cells can be chosen without violating occupancy or static maze walls.
- With open cells in front of them, bugs keep sliding forward even if several steps ahead are temporarily blocked by other bugs.
- When a corridor jam occurs but a detour exists, bugs explore the detour instead of waiting forever for the straight path to clear.
- Behaviour remains deterministic and message-driven per the global architecture rules.

## High-Level Approach
We replace the "path-to-exit or nothing" search with a two-tier crowd flow:
1. **Static flow field** – pre-compute the Manhattan-shortest distance from every traversable cell to the exit ignoring dynamic occupancy. This gives each bug a gradient to follow.
2. **Dynamic congestion map** – on each tick, measure how many bugs intend to occupy each cell in the near future to bias decisions away from crowded lanes.
3. **Local progress planner** – when a bug is ready, pick the best neighbour according to the gradient with congestion as a deterministic tie-breaker, falling back to a bounded detour search when the obvious step is blocked. The planner only requires a path to a *progress cell* (a cell with a strictly lower static distance), not to the final exit.

This flow guarantees that if there is an open neighbour that reduces distance (or keeps it steady while reducing congestion) the bug moves. Bugs only wait when the immediate ring of cells is all occupied or walled.

## Static Flow Field
- Compute once per maze configuration via a reverse breadth-first search seeded from every exit cell.
- Store in `world` as `NavigationField` (dense `Vec<u16>` covering interior cells plus the virtual exit row).
- Expose read-only via `query::navigation_field(&World) -> NavigationFieldView`.
- Rebuild only when the maze layout changes (e.g., editor mode) to avoid per-tick cost.

The field lets us compare any two neighbouring cells and know which one is closer to the exit without running A*.

## Dynamic Congestion Map
- During each movement system tick, allocate a `Vec<u8>` mirroring the navigation field.
- For every bug:
  - Follow the static gradient toward the exit for up to `CONGESTION_LOOKAHEAD` steps (4–6 cells works in tests).
  - Increment the congestion counter for each traversed cell (excluding the bug's current cell so it does not penalise itself).
- This approximates the queue length that waits ahead of every cell.
- Keep the data in-system; it is transient scratch state and never stored in `world`.

Congestion counts act purely as a tie-breaker, nudging bugs into alternative corridors when multiple neighbours offer the same progress.

## Local Progress Planner
The planner runs for each bug that accumulated enough time to step:

### Lexicographic neighbour ranking

The planner compares candidate neighbours using a lexicographic tuple of `(distance, congestion, cell order)`:

1. Prefer the option with the strictly smaller `navigation` distance; this guarantees we never choose a longer route just because it is empty.
2. If the distance matches, pick the cell with lower congestion to slide into the freer lane.
3. When still tied, fall back to the stable lexical ordering we already use for determinism.

This arrangement keeps the crowd moving monotonically toward the exit while leaving congestion as the secondary heuristic—no magic weights or tuning knobs required.

1. **Gather candidates.** Enumerate the four orthogonal neighbours that are inside the grid or the exit row. For each neighbour determine:
   - `distance_delta = navigation[neighbour] as i32 - navigation[current] as i32`.
   - `candidate_distance = navigation[neighbour]` and `candidate_congestion = congestion[neighbour]`.
   - Skip neighbours that are static walls or currently occupied (except by the moving bug itself).
2. **Immediate choice.** If at least one neighbour has `distance_delta < 0`, pick the lexicographically smallest `(candidate_distance, candidate_congestion, cell order)` tuple. This enforces strict progress while letting congestion break distance ties.
3. **Side-step relief.** If no decreasing neighbour is free but there is a neighbour with `distance_delta == 0` and low congestion, allow taking it only when it satisfies the anti-oscillation rule: its congestion must be lower than the current cell's count **and** the neighbour must differ from `last_cell` tracked via a two-tick ring buffer. Explicitly: *Flat side-steps (`distance_delta == 0`) are only allowed if `congestion[neighbour] < congestion[current_cell]` **and** cell ≠ `last_cell` (ring buffer 2 ticks).* This keeps lanes flowing sideways around clumps without ping-ponging between tiles.
4. **Detour search.** When neither of the above yields a move, run a bounded breadth-first search rooted at the current cell:
   - Depth limit: `DET0UR_RADIUS` (e.g., 6). Nodes deeper than the limit are pruned.
   - Success condition: reach any free cell whose `navigation` value is strictly lower than the start's or, if none exist within the radius, the free cell with the best lexicographic `(distance, congestion, cell order)` tuple.
   - The BFS treats occupied cells as walls *except* it allows the target cell to be the current occupant's cell when that occupant is already scheduled to vacate during the current tick according to the reservation ledger — using the reservation ledger only, **not** BugId ordering.
   - Reconstruct the first hop of the discovered path and emit `StepBug` toward that neighbour.
5. **Stall fallback.** Only if the BFS fails to find any free cell within the radius (i.e., the bug is boxed in) do we let the bug stay still this tick. Record a `stalled_for` counter so once a space opens the bug immediately re-enters the planner rather than waiting multiple ticks.

This planner ensures forward progress whenever a local route exists and explores small detours before giving up.

## Interaction with Reservations
- Keep the existing world-side reservation resolution: collect all `StepBug` commands, sort by `BugId`, and apply if the destination is still free.
- The movement system must respect deterministic ordering by iterating bugs in ascending `BugId` when generating commands.
- While building the detour BFS, treat destinations already chosen by lower-`BugId` bugs in the same tick as occupied to prevent cross-over swaps.

## Data & API Adjustments
- `core` crate: document a new `Query::navigation_field` DTO exposing the static distance grid.
- `world` crate: build the distance field during world initialisation and update it when tiles/targets change.
- `systems::movement`: store reusable buffers for the congestion map and BFS queue to avoid allocations.
- No changes to command/event enums are required; we only change movement heuristics inside the system.

## Determinism & Performance
- All derived data structures are deterministic given the same world state (static field) and iteration order (sorted bugs).
- Congestion map and BFS operate on fixed-size buffers (`width * (height + 1)` cells) and use integer arithmetic.
- Depth-limited BFS caps the per-bug work even in large maps and keeps complexity roughly `O(bugs * DET0UR_RADIUS^2)` per tick, which is acceptable for dozens of bugs.

## Testing Plan
1. **Unit tests** for the navigation field builder: verify distance gradients on hand-authored mazes.
2. **Unit tests** for the detour BFS: craft small grids where the straight path is blocked but a side corridor exists; ensure the chosen first hop matches expectations.
3. **System replay test** with a dense crowd:
   - Arrange a long corridor feeding into an exit with 10+ bugs.
   - Script ticks and assert that bugs advance until physically touching the front blocker.
   - Introduce a side hallway and confirm bugs divert into it when the main lane congests.
4. **Regression test** reproducing the original failure: set up a corridor where a bug several cells ahead pauses; verify followers continue stepping forward until they are adjacent to the blocker.

## Rollout Steps
1. Implement and expose the static navigation field in `world` + query DTO.
2. Extend the movement system with congestion tracking and the new planner.
3. Write unit tests for navigation and detour logic.
4. Update replay tests to cover dense-crowd behaviour and detours.
5. Profile on large maps and adjust constants (`CONGESTION_LOOKAHEAD`, `DET0UR_RADIUS`) if needed.
