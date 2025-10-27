# Proposal: Hierarchical per-species latents + count-first pressure alignment

## Goals

* Preserve strong intra-wave variety (fast/fragile vs slow/tanky) without post-scaling erasing it.
* Keep difficulty fidelity (pressure near target) with minimal identity distortion.
* Stay deterministic.

## Algorithm (single wave)

1. **Draw species count**

   * Sample `K ~ species_count(D)` (e.g., Poisson-like with floor/ceiling).
   * Draw mixture shares `w_i ~ Dirichlet(α_mix(D))`, enforce `w_i ≥ w_min(D)` (10% default) via standard trimming/renorm. If trimming collapses `K`, re-sample until `K ≥ K_min(D)`.

2. **Sample latents (hierarchical)**

   * Draw a **wave mood latent** `M ~ N(0, Σ_wave(D))` in log-space for `(log_hp, log_speed)`. Small magnitude (e.g., σ ≈ 0.08–0.12) to tilt the whole wave slightly tankier or faster.
   * For each species `i ∈ 1..K`, draw **residual** `ε_i ~ N(0, Σ_resid(D))` with **stronger variance** and negative correlation (e.g., ρ ≈ −0.6) to get HP/Speed trade-off.
   * Set species centre:
     `Z_i = μ(D) + M + ε_i` where `Z_i = (log_hp_i, log_speed_i)`.
   * Hard-clip to design bounds if any (rare).
   * Convert to multipliers: `hp_i = BASE_HP * exp(Z_i.hp)`, `speed_i = BASE_SPEED * exp(Z_i.speed)`.

3. **Sample cadence per species**

   * `cad_i ~ TruncNormal(μ_cad(D, speed_i), σ_cad(D), [cad_min, cad_max])`, round to ms.
   * Optionally tie `μ_cad` to `speed_i` monotically so faster speed nudges shorter cadence.

4. **Initial counts & timeline**

   * Propose counts `n_i = round(w_i * N_total(D))` (enforce `n_i ≥ 1`).
   * Build provisional spawn list (respecting per-species cadence/offsets).

5. **Pressure alignment — prefer counts over scaling**

   * Compute `P_hat` vs target `P_target(D)`, with tolerance band `ε(D)` (e.g., ±7–12%).
   * If `P_hat` within band → accept.
   * Else **adjust counts** (not HP/speed/cadences) to steer pressure:

     * If `P_hat < P_target_low`: increase counts, **preferentially add** from species with **higher pressure/second** (fast or tanky), keeping `w_i` roughly proportional and respecting caps.
     * If `P_hat > P_target_high`: decrease counts, **preferentially remove** from high-pressure species first.
     * Use a deterministic greedy step or small integer program with fixed tie-break order to maintain reproducibility.
   * Recompute spawn list/timeline after count changes.

6. **Duration cap (gentle, per-species)**

   * If total duration still exceeds `T_target(D)`:

     * Apply **proportional cadence compression** only to species **not at `cad_min`**, re-solve iteratively: pin any species that hits `cad_min`, redistribute remaining required compression across the rest. Round to ms at every step with a fixed ordering for determinism.
     * Do **not** scale HP or speed at this stage.

7. **Finalize**

   * Emit per-species `(hp_i, speed_i, cad_i)` and counts `n_i`. All spawns of a species inherit its cadence; speeds/HP remain unscaled.

## Why this works

* **Species identity is preserved**: HP/speed are never post-scaled by a single global η that compresses differences.
* **Variety comes from the residuals**: The hierarchical latent gives a wave “mood” (fast-ish night, tanky night), while per-species residuals provide the in-wave contrast you want.
* **Difficulty fidelity without sameness**: We align pressure by **counts**—the most “elastic” lever that doesn’t homogenize species parameters.
* **Cadence sameness avoided**: Compression is targeted and re-solved; many cadences survive above the floor.

## Potential issues & mitigations

* **Duration vs pressure tug-of-war**: Count increases to fix low pressure can lengthen duration. The iterative loop (counts → check duration → gentle compression) handles this. Keep `ε(D)` wide enough that you rarely need heavy compression.
* **Explosion of total spawns** (at high D with low per-unit pressure): Cap `N_total(D)` and prefer adding from high-pressure species first; if still low, allow a **small** global η (>1) as a **last resort** with a tight cap (e.g., ≤1.10) and log telemetry when used.
* **1-bug species edge cases**: Keep `w_min(D)` and `K_min(D)` so each species remains visible; if trimming would create micro-species, downsample `K` deterministically instead.
* **Determinism**: Fix RNG seeding, preserve stable species ordering, and use a canonical tie-break for count edits and compression re-solve.
* **Designer predictability**: Expose knobs: `σ_resid`, `ρ`, `Σ_wave`, `ε(D)`, `N_total(D)`, `w_min(D)`, `K_min/max(D)`, `cad_min/max`, and “η_last_resort_cap”.

## Minimal code delta map

* `populate_component_centres` → switch to hierarchical sampling (`M` + `ε_i`), drop any η-based per-species scaling.
* `align_pressure_with_eta` → replace with `align_pressure_with_counts` (small integer/greedy solver); keep a **tiny** `eta_last_resort` path behind a flag.
* `enforce_duration_caps` → new per-species proportional solver with pin-at-floor re-distribution.
* Telemetry → record: `M`, `ε_i`, pre/post counts, compression pins, and any last-resort η use.

## Test plan (goldens)

1. **Variety sanity**: At mid D, assert IQR of species `hp` and `speed` exceeds a threshold; assert negative correlation in final (hp, speed) pairs.
2. **Counts alignment**: Cases where `P_hat` is low/high; verify count adjustments bring pressure within `ε` without changing `(hp_i, speed_i)`.
3. **Duration compression**: (a) no cap, (b) some species pinned, (c) nearly all pinned—ensure multiple cadences remain when feasible.
4. **Determinism**: Same seed → identical plan; different seed → distributional properties hold.
5. **Last-resort η**: Force pathological under-pressure case; assert η applied ≤ cap and telemetry flags it.
