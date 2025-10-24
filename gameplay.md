# Gameplay Progression Roadmap (Engine → Playable Game)

The engine is stable (deterministic simulation, towers, pathing, damage).
The following roadmap introduces gameplay in strictly layered stages, each playable and testable before moving on.

---

### Phase 0 — Minimal Playable Loop

Objective: enable a trivial but repeatable “wave → kill → reward → build more” loop.

**Phase 0 kickoff roadmap**

1. **UI control hub** [DONE]
   * Add a right-side adapter panel (fixed 200 px width) to host build controls and wave buttons.
   * Route existing manual commands (build, spawn) through this panel so future buttons have a consistent home.
2. **Initial game state** [DONE]
   * Start sessions in build mode by default so the player can place towers before any action begins.
   * Disable automatic enemy spawning; waves should only start from explicit player input.
3. **Economy groundwork** [DONE]
   * Introduce global **gold resource** owned by the world state.
   * Award **gold per bug kill** (flat value is sufficient) and charge **tower placement cost**, rejecting placement when funds are insufficient.
4. **Wave scaffolding** [DONE]
   * Keep the existing manual “Spawn Wave” trigger but hardcode a basic wave (e.g. N slow bugs).
   * Define an initial `AttackPlan` representation that captures wave intent (per `pressure-spec.md`) without yet generating waves systemically.
5. **Failure condition** [DONE]
   * If any bug reaches exit → round is **lost** (no reset logic yet).

**Outcome:** The game now has agency, reward, and pacing. Usable for economy tuning.

---

### Phase 1 — Win/Loss Consequence and Tier Progression

* Maintain integer **difficulty tier** (starts at 0).
* After each successful wave → increment tier by 1.
* Scale **gold reward** by tier.
* On loss:

  * Destroy X % of existing towers (world mutation).
  * Decrease tier by 1–2.
* Display tier and gold.

**Outcome:** Losing has cost, winning has long-term benefit. Still hand-authored wave.

**Phase 1 progression roadmap**

1. **Persist tier inside the world**
   * Extend `maze_defence_world::World` with a `difficulty_tier` field initialised to zero and surface it through `maze_defence_world::query::difficulty_tier` plus a matching `Event::DifficultyTierChanged` in `maze_defence_core::Event` so adapters stay message-driven.
   * Update `world::apply` helpers (constructor paths, `Command::ConfigureTileGrid`, resets) to propagate the new field and emit the tier event whenever the value changes.
2. **Adjust rewards based on tier**
   * Modify `World::handle_fire_projectile` in `world/src/lib.rs` to scale the `Gold::new(1)` reward by `(tier + 1)`, saturating on overflow, before calling `update_gold` so every `Event::BugDied` benefits from higher tiers.
   * Add targeted headless replay coverage (e.g., under `tests/`) that scripts deterministic kills at tiers 0–n to assert `(tier + 1)` scaling and the saturation guard.
3. **Resolve round outcomes through commands**
   * Introduce a `RoundOutcome` enum and `Command::ResolveRound { outcome }` in `maze_defence_core` and handle it inside `world::apply`, incrementing the tier on wins, decrementing (with floor at zero) on losses, and emitting `DifficultyTierChanged` events accordingly.
   * Within the loss branch, remove a deterministic slice of towers (e.g., highest IDs first using `towers::TowerRegistry::iter`) and emit `Event::TowerRemoved` for each so the adapter reconciles state. The world remains the **only** locus of side-effects for these outcomes.
4. **Drive outcome commands from the CLI adapter**
   * Enhance `Simulation::process_pending_events` in `adapters/cli/src/main.rs` to queue `Command::ResolveRound` when `Event::RoundLost` appears, and when a wave finishes (`WaveState::finished()` && `query::bug_view(&self.world).iter().next().is_none()`), covering both win paths. The adapter’s responsibility stops at **detecting** the outcome and issuing the command — it must not apply any consequences directly.
   * Gate subsequent wave starts on the absence of an active outcome command to keep the deterministic loop intact.
5. **Surface tier changes in the UI**
   * Extend `adapters/rendering::Scene` (and the Macroquad panel) with a `TierPresentation` type so `Simulation::build_scene` can display the latest tier alongside the existing `GoldPresentation`.
   * Add a headless harness test (e.g., `tests/cli_round_resolution.rs`) that plays `build → start wave → clear → ResolveRound(Win)` to assert tier/gold deltas, then starts the next wave to confirm the adapter emits the proper commands and scene digests.

---

### Phase 2 — Player Difficulty Choice (Risk vs Reward)

* Before spawning wave, prompt for:

  * **Normal** → same tier.
  * **Hard** → +1 or +2 tier, with bonus gold reward.
* If Hard is successfully cleared → permanently increment base tier by 1.

**Outcome:** First strategic choice loop (risk/reward). Still uses hand-authored wave template.

---

### Phase 3 — Deterministic Pressure-Based Wave Generation

* Replace manual template with the deterministic **pressure spec** generator.
* Tier now maps directly to **pressure scalar P**.
* Integrate species registry and burst/pacing mechanics per spec.
* Wave content now scales naturally with progression.

**Outcome:** Wave system becomes fully systemic and scalable.

---

### Phase 4+ — Tower Variety and Upgrades

* Introduce differentiated tower types, unlocks, or upgrade trees.
* Expand economic and strategic depth incrementally.

---

This ordering guarantees **continuous playtestability** and avoids speculative design.
Each phase is strictly forward-compatible with later systems (no rewrites required).
