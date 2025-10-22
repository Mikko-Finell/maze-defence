# Tower Shooting Specification

## Intent

Evolve the targeting foundation into full combat: towers should periodically fire projectiles at their current targets, projectiles must traverse the visual targeting line, and bugs should lose health (and eventually die) when hit. The plan preserves the existing architectural contracts—world owns authoritative state, systems stay pure, and adapters only render derived scene data—while adding deterministic combat that future tower or bug variants can extend.

---

## Guiding principles

* **World-timed combat:** All cooldowns, projectile travel, and damage resolution advance exclusively from the `Tick` command processed by the world. Systems never maintain hidden timers.
* **Message-first orchestration:** Systems express firing intent through commands. The world validates readiness, spawns projectiles, steps them forward, and emits events for downstream systems/adapters.
* **Deterministic arithmetic:** Use integer/fixed-point math in cell space (half-cell units where necessary) until the final presentation conversion to avoid float drift.
* **Transient targeting, persistent authority:** Target selection continues to be a pure system projection. Once the world accepts a firing command, it owns the projectile and bug health bookkeeping without reconsulting adapters.
* **Builder isolation:** Combat is disabled in Builder mode. Towers neither accrue cooldown progress nor fire while building to keep editing workflows stable.

---

## Domain rules

* **Fire rate:** Each `TowerKind` exposes a `fire_cooldown_ms` constant. `Basic` towers use `1000` so they can fire once per second. The world converts this integer into a `Duration` when resetting cooldowns.
* **Projectile pacing:** Every tower kind defines how long a projectile should take to cross its maximum range (e.g., `Basic` = `1_000` ms). The world keeps travel deterministic by scaling that duration in proportion to the cached half-cell distance for each projectile.
* **Damage:** Tower kinds declare an integer damage amount dealt per projectile. Damage is applied atomically on hit; no splash or over-time effects.
* **Projectile path:** Spawned projectiles cache the firing tower centre and the targeted bug centre at the moment of firing. They advance linearly along this vector until the cached distance is fully travelled. Missing is impossible—the projectile either damages the intended bug or expires at the destination if the bug has already died.
* **Bug health:** Bugs spawn with maximum and current health values (identical for now). Health only changes when damage resolves. When health reaches zero, the bug dies immediately and is removed from movement/pathing consideration.
* **Hit detection tolerance:** A projectile hits exactly when `travelled_half >= distance_half`; if the target bug (by id) is alive then damage is applied, otherwise the projectile expires without damage.
* **Play-mode gating:** Cooldown timers and projectile travel freeze when `PlayMode != Attack`. On resuming attack mode, timers continue from their stored values.

---

## Core contracts (`core` crate)

### Types

* `pub struct Health(u32);` – document that zero means dead and arithmetic saturates at zero.
* `pub struct Damage(u32);`
* `pub struct ProjectileId(u32);`
* `pub enum ProjectileRejection { InvalidMode, CooldownActive, MissingTower, MissingTarget }`.
* Extend `TowerKind` with:
  * `pub const fn fire_cooldown_ms(self) -> u32` (`Basic` → `1000`).
  * `pub const fn projectile_travel_time_ms(self) -> u32` (`Basic` → `1000`).
  * `pub const fn projectile_damage(self) -> Damage` (`Basic` → `Damage(1)`).
* Extend `BugSpawned` DTOs with health for presentation: add `health: Health` to the event, mirroring the command below.

### Commands

* `Command::FireProjectile { tower: TowerId, target: BugId }` – emitted by combat systems once a tower is ready and a target exists.
* Extend `Command::SpawnBug` with `health: Health` (adapters/simulations must supply it; default scenarios use a helper `Health::new(3)` etc.).

### Events

* `Event::ProjectileFired { projectile: ProjectileId, tower: TowerId, target: BugId }` – broadcast after world accepts a firing command.
* `Event::ProjectileHit { projectile: ProjectileId, target: BugId, damage: Damage }` – fired when damage is applied.
* `Event::ProjectileExpired { projectile: ProjectileId }` – emitted when a projectile reaches its destination but the bug already died.
* `Event::ProjectileRejected { tower: TowerId, target: BugId, reason: ProjectileRejection }` – documents why a firing attempt failed.
* `Event::BugDamaged { bug: BugId, remaining: Health }` – sent after subtracting damage but before potential death removal.
* `Event::BugDied { bug: BugId }` – announces removal so systems/adapters update state.

Document each addition to maintain the message contract clarity.

---

## World (`world` crate)

### State

* `towers` entries gain `cooldown_remaining: Duration` updated only in attack mode.
* Bugs store `health: Health`. Movement occupancy removal leverages health to skip dead bugs.
* New `projectiles: BTreeMap<ProjectileId, ProjectileState>` where `ProjectileState` includes:
  * `id`, `tower`, `target`.
  * `start: CellPointHalf` / `end: CellPointHalf` (half-cell integer coordinates for precision).
  * `distance_half: u128` (precomputed line length in half-cell units).
  * `travelled_half: u128` accumulated during ticks.
  * `travel_time_ms: u128` derived from tower kind + `cells_per_tile` so maximum-range shots take the declared duration.
  * `elapsed_ms: u128` accumulated during ticks.
* `next_projectile_id: ProjectileId` monotonic allocator.

### Handling `Command::FireProjectile`

1. Reject if play mode ≠ Attack by emitting `Event::ProjectileRejected { tower, target, reason: ProjectileRejection::InvalidMode }` without mutating state.
2. Verify tower exists and has `cooldown_remaining == Duration::ZERO`. Emit `ProjectileRejected { reason: ProjectileRejection::MissingTower }` if the tower is absent, or `ProjectileRejected { reason: ProjectileRejection::CooldownActive }` if its cooldown is still running.
3. Verify bug exists and `health > 0`. Otherwise emit `ProjectileRejected { reason: ProjectileRejection::MissingTarget }`.
   Use the command's `tower`/`target` pair for every rejection event so logs stay traceable.
4. Compute projectile endpoints:
   * Tower centre from tower region (use same helper as targeting system).
   * Bug centre from bug cell (with +0.5 offsets).
5. Calculate half-cell distance and initialise state with `travelled_half = 0`.
6. Allocate a projectile id, insert into the map, set the tower cooldown to `Duration::from_millis(tower.kind.fire_cooldown_ms() as u64)`, and emit `Event::ProjectileFired`.

### Tick integration

* When processing `Command::Tick` in Attack mode:
  * Reduce `cooldown_remaining` for every tower by `dt`, saturating at zero.
  * For each projectile, add `dt.as_millis()` to `elapsed_ms` (clamped to `travel_time_ms`) and recompute `travelled_half = distance_half * elapsed_ms / travel_time_ms`.
  * When `travelled_half >= distance_half`:
    * Look up the target bug by id. If alive, subtract damage, emit `BugDamaged`, and if health hits zero remove the bug, clear occupancy, and emit `BugDied`. Then emit `ProjectileHit`.
    * If the bug is already dead, emit `ProjectileExpired` without applying damage.
    * Remove the projectile from the map in either case.
* In Builder mode, skip cooldown/position advancement but keep projectiles frozen.

### Bug death handling

* Removal clears occupancy grid, deletes bug entry, and ensures movement/path queries omit the bug before the next system tick.
* Consider cascading effects (e.g., if bug dies while scheduled to move) handled by next tick's queries naturally.

### Queries

* Extend `query::bug_view` DTOs to expose `health: Health` so systems can filter dead bugs without reading events.
* `TowerCooldownView { tower: TowerId, ready_in: Duration }` – ensures systems can decide when to fire.
* `ProjectileSnapshot { projectile: ProjectileId, tower: TowerId, target: BugId, origin_half: CellPointHalf, dest_half: CellPointHalf, travelled_half: u128, distance_half: u128, progress: f32 }` for adapters. Snapshots expose integer progress data for deterministic replay, while adapters can derive float positions from the half-cell values.
* Document that `projectiles` iteration is ordered by `ProjectileId` (via `BTreeMap`) for deterministic replay.

### Events for rejection

* Introduce `Event::ProjectileRejected { tower: TowerId, target: BugId, reason: ProjectileRejection }` with reasons `{ InvalidMode, CooldownActive, MissingTower, MissingTarget }`.

---

## Systems (`systems` crate)

### Tower combat system

Create `systems/tower_combat` with a pure type:

```rust
pub struct TowerCombat {
    scratch: Vec<Command>,
}

impl TowerCombat {
    pub fn handle(
        &mut self,
        play_mode: PlayMode,
        tower_cooldowns: TowerCooldownView,
        tower_targets: &[TowerTarget],
        out: &mut Vec<Command>,
    );
}
```

* Early out unless mode is `Attack`.
* For each `TowerTarget`:
  * Look up cooldown via the view; skip when `ready_in > 0`.
  * Emit `Command::FireProjectile { tower, target }`.
* Maintain deterministic ordering by iterating tower targets sorted by `tower` id (the targeting system already outputs stable order; document expectation/test).
* Do not track internal timing—rely exclusively on world cooldown queries.

### Event consumers

* Existing systems (movement, tower targeting) listen for `BugDied` to refresh cached views/flush assignments as needed (e.g., targeting should ignore dead bugs because queries already omit them; optional event handling for additional cleanup).
* No other systems mutate combat state.

---

## Simulation & adapters

### Execution order (CLI simulation)

1. Process pending world events (including new combat events) and feed them to systems.
2. Run `tower_targeting.handle(...)` to compute current targets.
3. Run `tower_combat.handle(...)` with play mode, cooldown view, and targeting results. Append commands to the outbound queue.
4. Submit commands to world (`FireProjectile` requests plus others like movement) before the next tick.
5. Cache projectile snapshots via `query::projectiles` for rendering.

### Scene updates

* Extend `Scene` with:

```rust
pub struct SceneProjectile {
    pub id: ProjectileId,
    pub from: Vec2,
    pub to: Vec2,
    pub position: Vec2,
    pub progress: f32,
}
```

  * `from`/`to` are static endpoints in cell coordinates (with +0.5 offsets) for adapters to draw the targeting line reused from targeting.
  * `position` is the current projectile dot location (cell-space floats).
* Populate `scene.projectiles: Vec<SceneProjectile>` from the cached projectile snapshots. Convert `origin_half`/`dest_half`/`travelled_half` into float positions (divide by two, then scale by tile metrics) before storing in the scene.

### Rendering

* Macroquad adapter: draw projectiles as filled circles (`radius = 0.1 * cell_length`) at `position`. Optionally reuse existing target lines as faint trails.
* CLI/text adapter: optionally render a `*` character along the line or simply log projectile ids; keep adapters deterministic.
* Ensure adapters gracefully handle empty lists (no allocation churn).

### Input handling

* No new adapter inputs are required. Existing tick cadence drives combat.

---

## Determinism & observability

* Rely solely on integer math for projectile travel: store `speed_half_per_ms` and multiply by `dt.as_millis()` to increment `travelled_half`.
* Emit events strictly in `ProjectileId` order to ensure replay stability.
* Document that `fire_cooldown_ms` values must be integer millisecond periods; choose values that keep cooldown math deterministic when converted to `Duration`.
* Provide helper logging in adapters/tests summarising damage and kills for debugging but keep it behind deterministic data (event logs).

---

## Testing plan

1. **World unit tests**
   * Firing in attack mode emits `ProjectileFired`, sets cooldown, and rejects follow-up commands until cooldown elapses.
   * Cooldown reduction matches elapsed ticks (including pause in builder mode).
   * Projectiles advance deterministically with various `dt` values and land exactly on the cached destination.
   * Bug damage reduces health and emits `BugDamaged`/`BugDied` events; dead bugs are removed from occupancy.
   * Projectiles targeting already-dead bugs emit `ProjectileExpired` without damage.
   * Rejection paths emit `ProjectileRejected` without mutating state.
2. **System tests**
   * Tower combat system emits `FireProjectile` only when cooldown view reports ready towers.
   * Integration with targeting ensures deterministic ordering when multiple towers share a bug.
3. **Simulation tests**
   * Scenario wiring verifies `Scene` contains projectile entries with accurate endpoints and interpolated positions.
   * Builder/attack mode toggles freeze/unfreeze cooldowns and projectile movement.
4. **Replay test**
   * Scripted scenario with multiple towers/bugs verifying identical `ProjectileFired`/`ProjectileHit`/`BugDied` timelines across replays (hash of final world + event log).

---

## Rollout sequence

1. Extend `core` contracts (types, commands, events, doc comments) with tests.
2. Update world state for bug health, tower cooldowns, projectile storage, and `Tick` handling; add queries.
3. Implement `systems/tower_combat` plus unit tests.
4. Wire combat flow into simulation/adapters, extend scene structures, and add rendering.
5. Add world/system/integration/replay tests covering firing, projectile travel, damage, and bug death.
