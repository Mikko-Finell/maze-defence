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
