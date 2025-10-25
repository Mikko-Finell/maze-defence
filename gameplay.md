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
* Successful **Hard** waves provide the permanent tier growth; **Normal** wins leave the tier unchanged.
* Scale **gold reward** by tier.
* On loss:

  * Destroy X % of existing towers (world mutation).
  * Decrease tier by 1–2.
* Display tier and gold.

**Outcome:** Losing has cost, while permanent difficulty growth comes from future Hard victories. Still hand-authored wave.

**Phase 1 progression roadmap**

1. **Persist tier inside the world** [DONE]
   * Extend `maze_defence_world::World` with a `difficulty_tier` field initialised to zero and surface it through `maze_defence_world::query::difficulty_tier` plus a matching `Event::DifficultyTierChanged` in `maze_defence_core::Event` so adapters stay message-driven.
   * Update `world::apply` helpers (constructor paths, `Command::ConfigureTileGrid`, resets) to propagate the new field and emit the tier event whenever the value changes.
2. **Adjust rewards based on tier** [DONE]
   * Scale the bug death reward by `(tier + 1)` inside `World::handle_fire_projectile`, using saturation to avoid overflow, so higher tiers boost every `Event::BugDied` payout.
   * Covered the behaviour with deterministic world tests that execute scripted kills at multiple tiers and verify the saturation guard.
3. **Resolve round outcomes through commands** [DONE]
   * Introduce a `RoundOutcome` enum and `Command::ResolveRound { outcome }` in `maze_defence_core` and handle it inside `world::apply`, leaving the tier unchanged on wins, decrementing (with floor at zero) on losses, and emitting `DifficultyTierChanged` events accordingly.
   * Within the loss branch, remove a deterministic slice of towers (e.g., highest IDs first using `towers::TowerRegistry::iter`) and emit `Event::TowerRemoved` for each so the adapter reconciles state. The world remains the **only** locus of side-effects for these outcomes.
4. **Drive outcome commands from the CLI adapter** [DONE]
   * Enhance `Simulation::process_pending_events` in `adapters/cli/src/main.rs` to queue `Command::ResolveRound` when `Event::RoundLost` appears, and when a wave finishes (`WaveState::finished()` && `query::bug_view(&self.world).iter().next().is_none()`), covering both win paths. The adapter’s responsibility stops at **detecting** the outcome and issuing the command — it must not apply any consequences directly.
   * Gate subsequent wave starts on the absence of an active outcome command to keep the deterministic loop intact.
5. **Surface tier changes in the UI** [DONE]
   * Extend `adapters/rendering::Scene` (and the Macroquad panel) with a `TierPresentation` type so `Simulation::build_scene` can display the latest tier alongside the existing `GoldPresentation`.
   * Add a headless harness test (e.g., `tests/cli_round_resolution.rs`) that plays `build → start wave → clear → ResolveRound(Win)` to assert tier/gold deltas, then starts the next wave to confirm the adapter emits the proper commands and scene digests.

---

### Phase 2 — Player Difficulty Choice (Risk vs Reward)

* Before each wave, surface **Normal** and **Hard** buttons in the control panel.
  * **Normal** → run the wave at the current tier.
  * **Hard** → run the wave at `tier + 1`, award bonus gold on victory.
* If Hard is successfully cleared → permanently increment the base tier by 1.

**Outcome:** First strategic choice loop (risk/reward). Still uses hand-authored wave template.

**Phase 2 progression roadmap**

1. **Expose difficulty buttons in the UI** [DONE]
   * Extend the adapter panel layout so it renders side-by-side **Normal** and **Hard** buttons before the wave trigger controls.
   * Wire button presses to emit a new `Command::StartWave { difficulty }`, queuing it like existing manual actions so determinism is preserved.
2. **Record pending difficulty inside the world** [DONE]
   * Introduce a `PendingWaveDifficulty` enum stored on `maze_defence_world::World` and surface it through `world::query::pending_wave_difficulty` alongside an `Event::PendingWaveDifficultyChanged`.
   * Ensure configuration commands (`Command::ConfigureTileGrid`, resets) initialise the field and emit change events so adapters remain message-driven.
3. **Resolve wave launches based on difficulty** [DONE]
   * Update the spawning system to consume the new enum and treat **Hard** as `tier + 1` when generating wave contents and gold multipliers, keeping **Normal** unchanged.
   * When `Command::StartWave` is applied, compute the effective parameters (difficulty, `tier_effective`, `reward_multiplier`, any pressure scalar) and emit a factual `Event::WaveStarted { … }` carrying them alongside a `wave_id` so downstream systems can react without re-querying.
4. **Apply Hard victory promotions** [DONE]
   * Extend `Command::ResolveRound` handling to detect victories tagged as **Hard**, using the stored `wave_id` / difficulty context from `Event::WaveStarted`, increment the permanent tier by one, and emit a new `Event::HardWinAchieved` for UI feedback.
   * Saturate tier increases at the desired cap (if any) while leaving loss handling untouched.
5. **Display difficulty state to the player** [DONE]
   * Enrich `adapters/rendering::Scene` with difficulty selection feedback by highlighting the active button and surfacing the Normal/Hard reward previews.
   * Added CLI adapter tests to prove the scene exposes the reward multipliers and highlights Hard when it is the pending selection.

---

### Phase 3 — Deterministic Pressure-Based Wave Generation

* Replace manual template with the deterministic **pressure spec** generator.
* Tier now maps directly to **pressure scalar P**.
* Integrate species registry and burst/pacing mechanics per spec.
* Wave content now scales naturally with progression.

**Outcome:** Wave system becomes fully systemic and scalable.

**Phase 3 progression roadmap**

1. **Promote AttackPlan contracts to core** [TODO]
   * Add `AttackPlan`, `BurstPlan`, and supporting enums (`SpeciesId`, `SpawnPatchId`) to `maze_defence_core`, plus serialisable config structs for species weights and timing clamps so all layers share the canonical data model.
   * Extend `Event`/`Command` with message variants (`Command::GenerateAttackPlan`, `Event::AttackPlanReady`) to keep adapters and systems message-driven while preventing world-side bespoke calls.
2. **Authoritative registries inside the world** [TODO]
   * Persist the species table, patch definitions, and pressure tuning knobs (`pressure_mean`, `pressure_std_dev`, `dirichlet_beta`, burst sizing limits) on `maze_defence_world::World`, initialising them during configuration/reset commands.
   * Expose read-only queries (`world::query::species_table`, `world::query::patch_table`, `world::query::pressure_config`) plus `Event::PressureConfigChanged` so UI/tests can inspect state and deterministic snapshots cover registry edits.
3. **Pure wave generator system** [TODO]
   * Create a dedicated `systems/wave_generation` crate that consumes `Command::GenerateAttackPlan`, the world queries from step 2, and the current tier/seed context to output a deterministic `AttackPlan` following `pressure-spec.md` (Dirichlet sampling, burst splitting, timing jitter, RNG stream derivation).
   * Include exhaustive unit tests validating replay identity, budget closure, burst splitting, and safety clamps using scripted seeds; wire the crate into the workspace with feature flags matching existing systems conventions.
4. **Integrate generator with spawning pipeline** [TODO]
   * Update the adapter/CLI flow so the **Normal/Hard** button enqueues `Command::GenerateAttackPlan { difficulty }` before `Command::StartWave`, awaiting the resulting `Event::AttackPlanReady { wave_id, plan }` and storing the plan snapshot for presentation (previewing pressure, species mix).
   * Modify `maze_defence_world::apply` to persist the latest plan per `wave_id` and emit deterministic `Event::WaveStarted` payloads referencing that plan so downstream systems never resample.
5. **Deterministic burst execution** [TODO]
   * Refactor `systems/spawning` to consume `AttackPlan` bursts instead of hard-coded templates: schedule `Command::SpawnBug` events respecting cadence, gap timing, and global spawn-per-tick caps while reporting completion via `Event::BurstDepleted`.
   * Add headless integration tests (`tests/wave_generation_replay.rs`) that drive `GenerateAttackPlan → StartWave → Tick` loops for multiple seeds and assert identical event timelines, ensuring the new pipeline remains replay-safe.

---

### Phase 4+ — Tower Variety and Upgrades

* Introduce differentiated tower types, unlocks, or upgrade trees.
* Expand economic and strategic depth incrementally.

---

This ordering guarantees **continuous playtestability** and avoids speculative design.
Each phase is strictly forward-compatible with later systems (no rewrites required).
