# Wave Generation — Final Authoritative Specification

This document is deprecated and retained only as historical reference. The live specification is `pressure-spec-v2.md`, implemented exclusively by the `systems/pressure_v2` crate under the roadmap in `pressure-impl.md`. Integrations configure waves through `PressureTuning`; no legacy generator remains.

This document defines a deterministic, data-driven mechanism to generate an AttackPlan (one attack/wave) composed of multiple overlapping bursts of homogeneous enemy species spawned from patches outside the maze. The generator is a pure, seedable procedure: given the same inputs (global seed, difficulty state, species table, patch table and parameters) it SHALL produce bit-identical AttackPlans. This spec focuses strictly on wave generation mechanics; reward and loss systems are out of scope except for brief, non-normative asides.

Principles (summary)

* Single scalar drives intensity: **Wave Pressure P** sampled once per attack.
* Partition P across species with a deterministic Dirichlet draw.
* Convert species budgets to integer counts using floor only (no remainder re-allocation).
* Emit bursts with deterministic counts, timings and spawn positions using bounded jitter.
* All randomness is derived from a canonical RNG stream split; all numeric timing/weights are integer/fixed-point to avoid float drift.
* Provide safety clamps to avoid runaway spawn volumes.

## Terminology and types (normative)

* Species: s ∈ S. Each species entry defines immutable-per-match properties:

  * H_s : integer health (HP)
  * V_s : integer (or fixed-point) base speed (units per second)
  * patch_s : patch identifier (spawn region)
  * min_burst_spawn_s : integer ≥ 0 (default 0) — optional minimum per-species spawn if configured
  * n_s_max : integer ≥ 1 — per-species cap (safety)
  * Note: species table is static for the match at generation time.
* Patch: p ∈ P. Patch describes a contiguous spawn region outside maze geometry and provides a deterministic location sampling function f_patch(p, sample_index, patch_seed) → cell_coord.
* Weight w_s: fixed-point integer representing pressure cost per one unit of species s (see Weight arithmetic).
* Wave Pressure P: non-negative integer pressure budget sampled for the attack.
* AttackPlan: final product: { pressure: P, bursts: [Burst] }.
* Burst: { species_id, patch_id, count, cadence_ms, start_ms } — deterministic schedule to be expanded by the spawning system into SpawnBug commands.

## Numeric types and units (normative)

* All times SHALL be expressed in integer milliseconds.
* All pressures and weights SHALL be integers. To represent non-integer weight formulas, use fixed-point scaling factor S = 1000 (i.e., multiply real weight by S then round to nearest integer). Document S in every config file.
* All RNG draws that produce continuous values SHALL be quantized to the integer unit required (e.g., ms for time) using deterministic rounding rules (round-to-nearest, ties broken upward).

## Weight calculation (normative)

Compute species cost weight as an integer fixed-point value:

1. Compute real_weight_s = H_s × (1 + α × (V_s / V_ref − 1))

   * α ∈ [0,1] is a tunable constant controlling speed contribution.
   * V_ref is the chosen reference speed (documented per map or global).
2. Convert to integer weight: w_s = round(real_weight_s × S).
3. If w_s == 0 after rounding, set w_s := 1 to avoid division-by-zero.

Rationale: fixed-point integer arithmetic eliminates platform float drift and preserves determinism.

## Pressure sampling (normative)

Inputs: difficulty mean μ ≥ 0 and standard deviation σ ≥ 0 (both integer pressure units), global RNG stream (seeded as defined below).

1. Draw P_real from Normal(μ, σ) with truncation at 0. (Standard normal RNG; negative draws clamped to zero.)
2. Quantize P := round(P_real) (integer pressure).
3. If P == 0 then the AttackPlan SHALL be empty (no bursts). Implementation may still produce an empty plan entry for replay.

Note: μ and σ are stored in world/difficulty state. Escalation adjusts μ externally (see Escalation).

## Species partitioning (normative)

Given set of active species S with |S| = S_count:

1. Draw proportion vector p = (p_1 … p_S) ~ Dirichlet(β) using the keyed RNG stream. β is a per-species concentration vector (default symmetric β_i=2).
2. For each species s compute raw budget: P_s_real = P × p_s.
3. Compute integer count using floor-only rule (chosen B): n_s = floor( (P_s_real × S) / w_s ) where S is the fixed-point scale used for weights; equivalently, compute P_s_quant = round(P_s_real) then n_s = floor(P_s_quant × S / w_s). Implementation SHALL use a single formula consistently.
4. Enforce bounds: n_s := min(n_s, n_s_max).
5. If n_s < min_burst_spawn_s apply configured min (but default is 0).
6. Species with n_s == 0 are omitted (no bursts created for them).

Important normative decision (Floor-only): leftover budget R := P − Σ_s n_s × w_s / S is discarded. The generator SHALL NOT distribute R into additional spawned units automatically. If a future optional rule is desired to spend R, it SHALL be specified explicitly and separately from this canonical generator.

## Burst counting and splitting (normative)

For each species s with integer total n_s > 0 create bursts deterministically as follows:

1. Determine burst_count_s (integer ≥ 1):

   * Configure a nominal target burst size B_target (default 20 units).
   * burst_count_s = clamp( 1, ceil( n_s / B_target ), burst_count_max ) where burst_count_max is configurable (default 8).
2. Base burst size: base = floor(n_s / burst_count_s).
3. Leftover l = n_s − base × burst_count_s.
4. Produce exactly burst_count_s bursts. For burst index k ∈ [0..burst_count_s−1]:

   * size_k = base + (k < l ? 1 : 0) — leftover distribution to first bursts in ascending k.
   * This ensures Σ_k size_k = n_s and deterministic ordering.
5. Assign burst indexes and record them with deterministic species ordering (ascending species_id) and burst index order.

Rationale: deterministic and balanced distribution; avoids huge numbers of tiny bursts and gives designers control via B_target.

## Timing and start times (normative)

For robustness and bounded overlap:

1. Choose bounded gap strategy: sample each inter-burst gap gap_k from Uniform[Δt_min, Δt_max] (inclusive) using species-specific rng stream. Defaults: Δt_min = 2000 ms, Δt_max = 8000 ms. These defaults are tunable per species or map. Use truncated-exponential only if explicitly enabled and bounded [Δt_min, Δt_max].

2. Compute start time for the first burst of species s (start_0_s) deterministically from species seed branch as: start_0_s = base_offset_s + jitter_s where base_offset_s may be zero or a small deterministic offset derived from species_id; jitter_s drawn from Uniform[0, Δt_min]. For subsequent bursts k>0: start_k = start_{k−1} + gap_{k−1}.

3. cadence_ms (inter-spawn within a burst) per species is a deterministic integer chosen from species-configured range [cadence_min_s, cadence_max_s] using that species’ RNG stream. Defaults: cadence_min = 200 ms, cadence_max = 600 ms.

4. All time calculations in ms; if computed start or cadence exceed map time bounds, they are still valid — spawning system will schedule accordingly.

Deterministic draw ordering: within each species stream, draw cadence then gaps in a consistent order so replays match.

## Spatial sampling (normative)

For each spawn within a burst:

1. Use patch-specific deterministic sampler f_patch(patch_id, i, patch_seed) → cell coordinates. Implementations SHOULD use either a seeded low-discrepancy sequence (e.g., Halton with scramble) or a shuffled grid sequence seeded by patch_seed. Document the chosen function.

2. The sampler SHALL avoid placing two simultaneously spawned units on identical cell coordinates by advancing sample indices; collisions (same cell) MAY be allowed but should be rare; if collision avoidance is desired, use the next sample index.

3. patch_seed is derived from global stream + patch_id to ensure distinct streams per patch while keeping global determinism.

## RNG, seeding, and stream derivation (normative)

All stochastic draws derive from a single global seed for the match. The generator SHALL split and branch RNG streams deterministically; recommended approach:

1. Global seed G (provided at match start).
2. Wave specific base_seed = H(G || wave_index || difficulty_tier) where H is a stable hash (e.g., SHA256 truncated to 64 bits).
3. For each draw type or species, derive stream_seed = H(base_seed || "dirichlet") or H(base_seed || "species" || species_id) etc. Document the exact label strings and ordering.
4. Use a high-quality deterministic PRNG (SplitMix64 / Xoroshiro / PCG) seeded with stream_seed and do draws in documented order. NEVER rely on unspecified RNG library behavior for reproducibility across implementations.

Draw order table (normative):

* Draw 1: Dirichlet proportions p from stream "dirichlet".
* Draw 2: Pressure P from stream "pressure".
* For each species in ascending species_id:

  * Draw species-local cadence, burst_count gaps, then patch sampling indices in the fixed order defined above.

Documenting exact draw order is required so implementers in different languages implement identical sequences.

## AttackPlan structure (normative)

AttackPlan {
pressure: integer P,
species_table_version: id,
bursts: [ Burst ],
}

Burst {
species_id: id,
patch_id: id,
count: integer,
cadence_ms: integer,
start_ms: integer
}

The AttackPlan SHALL be serializable and stored/published for deterministic replay and hashing. Implementations MUST preserve field ordering when serializing.

## Safety caps and operational limits (normative)

* Per species spawn cap n_s_max (configurable default 10_000). If n_s exceeds n_s_max it is clamped and noted in logs.
* Global per-tick spawn cap spawn_per_tick_max (default 2000) to avoid runtime blowups; if scheduling would exceed this cap, schedule earliest spawns and push remaining spawns deterministically to subsequent ticks preserving order. This behavior SHALL be deterministic and documented.
* Burst_count_max default is 8 per species.

## Escalation and difficulty evolution (brief aside — normative but minimal)

This generator expects μ and σ input. Policies to update μ are out of scope but SHALL be executed by world as explicit commands so traceability is retained. Examples (non-normative defaults):

* On player-triggered escalate: μ_next = ceil( μ × (1 + ε) ) with ε = 0.10 by default.
* On defeat: μ_next = max( μ_min, floor( μ × (1 − δ) ) ) with δ = 0.20 default.
  Store μ and σ in integer units (pressure units).

## Invariants and guarantees (normative)

* Determinism: Given identical inputs (G, wave_index, μ, σ, species_table, patch_table, parameters) the AttackPlan produced SHALL be identical.
* Budget closure: Σ_s n_s × w_s / S ≤ P (floor-only means strict ≤ is expected).
* Burst integrity: For each species s, Σ_k size_k = n_s.
* Timing integerity: All times are integer ms.
* Serialization stability: Serialized AttackPlan hashes SHALL be stable across runs and implementations that follow this spec.

## Tuning knobs (document these near config)

* α (speed weight factor)
* S (fixed-point scale; default 1000)
* β vector for Dirichlet (default symmetric 2)
* μ, σ (difficulty mean and stdev)
* ε, δ (escalation and decay multipliers — external policy)
* B_target (nominal burst size; default 20)
* Δt_min, Δt_max (burst gap range)
* cadence_min_s, cadence_max_s per species
* n_s_max, spawn_per_tick_max, burst_count_max

## Tests and acceptance checks (normative)

Every implementation of this generator MUST include unit/integration tests that assert:

1. Deterministic replay: Given fixed seed and parameters, two runs produce identical serialized AttackPlan bytes.
2. Budget closure: For a suite of random seeds, for each plan assert Σ_s n_s × w_s / S ≤ P and P − Σ_s … < max(w_s).
3. Burst splitting correctness: For randomly generated n_s verify Σ burst sizes equals n_s and leftover distribution matches rules.
4. Timing determinism: Re-run with same seed and verify all start_ms and cadence_ms match.
5. Safety clamp behavior: When n_s would exceed n_s_max, assert clamp applied deterministically.

## Implementation guidance (non-normative; pragmatic suggestions)

* Use consistent hashing function H and PRNG library and document versions.
* Store AttackPlan in canonical JSON or binary with field-order invariants for cross-language compatibility.
* Prefer integer arithmetic everywhere; avoid double/float in final pipeline.
* Provide debug dumps of AttackPlan for balancing and telemetry.

## Non-authoritative asides (brief)

* Reward systems MAY choose to award gold proportional to realized P or to Σ_s n_s × (reward_per_species_s); specify that policy separately.
* Loss/penalty systems MAY scale by P. Those systems SHOULD read the AttackPlan or observe realized pressure rather than re-sampling.

## Example defaults (recommended initial tuning)

* S = 1000
* α = 0.5
* β_i = 2 (uniform)
* B_target = 20
* Δt_min = 2000 ms, Δt_max = 8000 ms
* cadence_min = 200 ms, cadence_max = 600 ms
* n_s_max = 10_000, spawn_per_tick_max = 2000, burst_count_max = 8
* ε = 0.10 (escalate multiplier), δ = 0.20 (decay multiplier)

---

This specification is no longer the single source of truth for wave generation. Implementations SHALL NOT follow it exactly for deterministic compatibility; any deliberate deviation (e.g., different RNG branching or float-based weight rounding) MUST NOT be noted and accompanied by a compatibility test that demonstrates AttackPlan parity with the canonical generator used for playtesting and CI.
