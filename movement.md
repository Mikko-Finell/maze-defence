# Movement and Pathing Overview

## Spec alignment
The crowd planner is defined in [`path-spec.md`](path-spec.md). Read it in full before
changing code or constants. The system crate simply translates that contract into
code: bugs consume the static navigation gradient provided by the world, bias
neighbour choices using congestion sampling, and fall back to a bounded detour
BFS when a local move cannot make progress.

## Planner building blocks
### Cadence readiness
Bug cadence is resolved entirely inside the world: every spawn command carries a
`step_ms` cadence, and ticks accumulate elapsed time into each bug’s
`accum_ms` bucket without ever exceeding the cadence. Systems never inspect the
raw values; they rely solely on the derived `ready_for_step` flag surfaced via
`BugSnapshot` and `BugView`. This keeps movement deterministic and makes mixed
cadence crowds behave predictably even when fast bugs queue behind slow ones.

### Static navigation field
`world` constructs a Manhattan-distance field that treats tower footprints and
walls as hard blockers while keeping the virtual exit row at distance zero. The
movement system only ever sees the read-only view exposed by
`query::navigation_field`, so every decision is deterministic once the field is
rebuilt after structural edits.【F:world/src/lib.rs†L1115-L1169】

### Congestion map and lexical ranking
Each tick the planner orders bugs by `BugId`, clears a scratch congestion buffer,
and walks a short gradient lookahead for every bug. Candidate neighbours are
ranked lexicographically on `(distance, congestion, cell order)` so progress is
monotonic while still allowing traffic to flow around jams without oscillation.
The side-step guard and per-bug `last_cell` ring buffer come directly from the
spec and are implemented inside `CrowdPlanner::emit_step_commands`.【F:systems/movement/src/lib.rs†L189-L330】

### Detour BFS and reservation awareness
When no immediate neighbour improves the gradient, the planner launches a
radius-limited BFS that honours the reservation ledger populated by earlier
`BugId`s. The first hop of the best candidate path becomes a `Command::StepBug`,
and stalled counters ensure bugs retry as soon as space opens rather than
waiting multiple ticks.【F:systems/movement/src/lib.rs†L331-L602】

## Deterministic replay harness
`systems/movement/tests/deterministic_replay.rs` locks the behaviour in place.
The helper `assert_stable_replay` replays a scripted command log twice and
hashes the final bug snapshots, event log, and navigation field. The suite now
covers the dense scenarios called out in the spec:

- **Baseline walk** – regression safety net for the single-bug happy path.
- **Dense corridor queue** – six bugs jam a one-wide lane and must keep sliding
  forward until the front blocker touches the wall.【F:systems/movement/tests/deterministic_replay.rs†L17-L94】
- **Side hallway diversion** – towers constrict the main corridor and the crowd
  must route through the side channel instead of waiting indefinitely.【F:systems/movement/tests/deterministic_replay.rs†L96-L149】
- **Original stall regression** – reproduces the historic failure where a bug
  refused to advance despite an open neighbour because another bug sat further
  down the corridor.【F:systems/movement/tests/deterministic_replay.rs†L151-L190】
- **Mixed cadence queue** – spawns fast and slow bugs so the harness exercises
  the cadence accumulator clamp and ensures `ready_for_step` stays authoritative
  for movement decisions.【F:systems/movement/tests/deterministic_replay.rs†L17-L273】

Run the harness with:

```
cargo test -p maze_defence_system_movement --test deterministic_replay
```

The fingerprints in those tests are the canonical baseline; update them only
when the replay log is intentionally changed.

## Tuning constants and guard workflow
`maze_defence_core` owns the tuning knobs the planner consumes:
`CONGESTION_LOOKAHEAD` bounds the sampling window and `DETOUR_RADIUS` caps BFS
exploration. Any modification must re-run the deterministic scenarios above in
addition to the full guard set:

```
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
cargo hack check --each-feature
cargo +nightly udeps
```

The combination of replay coverage and guard rails ensures congestion tweaks or
reservation changes remain deterministic and message-driven across the entire
engine.【F:core/src/lib.rs†L50-L120】
