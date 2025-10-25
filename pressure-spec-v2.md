# Wave Generation Specification (Pressure-Scaled, Distribution-Sampled, Species-Merged)

This document defines exactly how to generate an enemy wave for a given player difficulty level. This is the authoritative behavior. Implementations must match it deterministically given the same inputs.

This document supersedes and depricates `pressure-spec.md`.

The generator is responsible for creating a list of individual bug spawns, each with:

* `time_ms`: when it spawns (ms from wave start)
* `hp`: bug HP after final scaling
* `speed_mult`: bug movement speed multiplier after final scaling (1.0 = baseline tutorial movement speed; higher is faster)
* `species_id`: which generated component this bug belongs to for this wave

Lane/patch spawning location is explicitly out of scope and must not be implemented or improvised.

The generator must not use hardcoded enemy species tables. All species in a wave are procedurally sampled from statistical distributions as described in §4.

---

## 1. Inputs, Outputs, Determinism

### 1.1 Inputs

The generator takes:

* `game_seed`: 64-bit integer identifying the run/save.
* `level_id`: identifier for the level/map/layout.
* `wave_index`: which wave is being generated within this level.
* `difficulty D`: monotonic scalar representing the player’s chosen/escalating difficulty level. Higher D means harder. D is provided by game state; it is not sampled.

### 1.2 Output

The generator returns a list of spawn records. Each record must contain:

* `time_ms` (integer ms from wave start)
* `hp` (final HP after scaling)
* `speed_mult` (final speed multiplier after scaling)
* `species_id` (component index after merges; 0..K_final-1)

The list must be sorted by:

1. `time_ms` ascending
2. `species_id` ascending
3. index-within-species ascending

No other behavior (lane, sprite, rewards, drops) is in scope.

### 1.3 Determinism

Given the same `(game_seed, level_id, wave_index, D)`, the generator must produce byte-for-byte identical output.

To guarantee this:

* Initialize a deterministic PRNG (e.g. PCG or xoroshiro) with a hash of `(game_seed, level_id, wave_index, D)`. The hash function and concatenation order must be fixed and documented.
* All randomness must be consumed in exactly the order defined in this spec. No additional random draws or reordered draws are allowed.
* All tie-break rules in this spec must be followed exactly.
* All sorting must be stable for identical keys.

---

## 2. Wave Generation Stages (exact order)

Wave generation must proceed in this order:

1. Sample global difficulty latents — bug count, HP multiplier, speed multiplier (§3).
2. Compute the intended total wave pressure budget (`P_wave`) (§3.4).
3. Sample a provisional number of species components (`K`) and sample each component’s HP/speed center (§4.1–§4.2).
4. Allocate the total bug count (`Count`) across components using Dirichlet proportions and integer rounding (§4.3).
5. Enforce the “no tiny species” rule by merging undersized components deterministically (§4.4).
6. Uniformly scale all components’ stats with a single global factor `η` so total wave pressure matches `P_wave` (within clamps) (§5).
7. Assign per-component cadence and start offsets, generate timestamps for each bug, then build the full spawn list (§6.1–§6.3).
8. Enforce a maximum wave duration by compressing cadences if required (§6.4).
9. Sort and return.

This order is mandatory.

---

## 3. Difficulty Latents and Pressure Target

### 3.1 Baseline interpretation

We define tutorial baselines:

* Baseline HP per bug (`H_base`) = 10 HP.
* Baseline movement pacing is approximately 500 ms per movement step. We define that baseline speed as multiplier `1.0`. Faster bugs have higher multipliers: e.g. 400 ms per step ≈ 1.25×.

These baselines exist so we can talk about multipliers consistently. They are not changed dynamically during generation; tuning them is game balance, not generator logic.

### 3.2 Bug count curve (fast early growth, then tapering growth, not flat stop)

The expected number of bugs per wave is sampled from a difficulty-dependent distribution. This distribution must satisfy all of the following:

* At low difficulty (`D` near 1): around 20–30 bugs on average.
* As difficulty increases, expected bug count increases rapidly at first (for early levels).
* As difficulty continues to increase, the growth in bug count slows down (tapers), but does not freeze at a fixed number — it can keep climbing.
* Designers must be able to push average counts into the hundreds at higher difficulty (e.g. ~500 by some mid/high difficulty).
* Designers must be able to approach a soft ceiling around ~1000 bugs per wave at extreme difficulty settings so waves don’t become unplayably long.
* The curve parameters must be tunable.

We express the mean expected count `μ_count(D)` using a logistic-style saturating function with adjustable ceiling:

`μ_count(D) = C_min + (C_cap - C_min) / (1 + exp(-a * (D - D_mid)))`

Where:

* `C_min`: baseline expected count at very low difficulty.
* `C_cap`: asymptotic upper limit (soft max / plateau).
* `D_mid`: the difficulty value around which the growth is steepest.
* `a`: steepness/slope of the curve.

**Required behavior of these knobs:**

* Increasing `C_cap` raises the long-term plateau. For example:

  * If `C_cap = 120`, the curve will taper near ~120 bugs.
  * If `C_cap = 500`, the same formula will taper near ~500 bugs.
  * If `C_cap = 1000`, it will taper near ~1000 bugs.
* Designers must be able to raise `C_cap` over time during balancing, without touching code logic.
* `C_min` must be set near ~20 so that tutorial waves are ~20-ish on average, matching “about 20 bugs on average” at the start.
* `a` and `D_mid` control how fast we ramp: smaller `D_mid` + larger `a` means we ramp bug count very fast in the first few difficulty steps; larger `D_mid` means slower early ramp.

**Default recommended starting values (these are defaults, not hardcoded constants):**

* `C_min = 20`
* `C_cap = 1000`
* `D_mid = 3`
* `a = 1.2`

Those defaults give:

* `D=1`: mean on the order of a few tens of bugs (~20–30 range).
* `D≈5–6`: mean in the hundreds (can be ~200–400 depending on the exact slope).
* As `D` continues to rise, it keeps increasing toward `C_cap` (1000), but each additional unit of `D` adds less than the previous, so growth is tapering, not linear explosion.

This matches the requirements:

* Early on we get 20 → 30 → 60 → etc.
* By some higher difficulty (not necessarily 5 anymore; depends on tuning), we might be seeing ~500.
* We can continue to increase bug counts with higher `D`, but they taper and approach `C_cap` rather than shooting to infinity.

The actual integer `Count` for this wave is then sampled stochastically around that mean:

`Count ~ TruncNormalInt(mean = μ_count(D), sd = 0.08 * μ_count(D), bounds = [5, C_cap])`

Behavioral requirements of this sampling:

* `sd = 0.08 * μ_count(D)` means ±8% wiggle room so waves on the same difficulty vary slightly, not identically.
* The lower bound is fixed at 5 (no wave may have fewer than 5 bugs unless design later explicitly changes that bound).
* The upper bound must not exceed `C_cap`. This ensures we never exceed tuning’s intended soft ceiling for wave length.
* The result is rounded to an integer after truncation.

`Count` is final for bug quantity. Later steps can redistribute those bugs across components, but must not change `Count`.

### 3.3 HP and speed latents

We define two more difficulty-driven latents: one for HP, one for speed.

These control how tough and how fast the wave “should feel” at difficulty `D`.

#### 3.3.1 Mean HP curve

We require:

* At D=1, typical HP should be around 10 HP.
* At D=2, typical HP ~15 HP.
* By higher difficulty, HP should keep increasing (e.g. ~25–30 HP around moderate difficulty, then higher beyond that).
* HP scaling must not stop increasing even if bug count growth tapers. HP can continue to rise indefinitely with `D`.

We model the expected mean HP `μ_hp(D)` (absolute HP, not multiplier) as:

* A soft early boost plus mild multiplicative growth beyond a pivot.
  One acceptable form:
  `μ_hp(D) = H_base * (1 + h_soft * (1 - exp(-k_h * (D - 1)))) * (g_h ^ max(0, D - D_h))`

Where:

* `H_base = 10`
* `h_soft = 0.6`
* `k_h = 1.0`
* `g_h = 1.08`
* `D_h = 4`

This satisfies:

* D=1: ~10 HP
* D=2: ~15 HP
* D=3: ~18–20 HP
* D~5: ~25–30 HP
* After D_h, it keeps scaling multiplicatively by `g_h` (~+8% per extra step), so HP can keep going up indefinitely with difficulty.

Convert that to an HP multiplier relative to tutorial:
`μ_HPmul(D) = μ_hp(D) / H_base`

Sample the actual HP multiplier for this wave:
`HPmul0 ~ TruncNormal(mean = μ_HPmul(D), sd = 0.05, bounds = [0.6, 2.2]]`

Clamp to [0.6, 2.2] so we don’t see “paper” or “unkillable wall” extremes in a single wave.

#### 3.3.2 Mean speed curve

We require:

* Tutorial speed ≈ 500 ms/step (speed multiplier ~1.0).
* Difficulty 2-ish: faster, e.g. ~400 ms/step (≈1.25×).
* Difficulty continues to increase speed further.
* Speed can keep rising indefinitely with difficulty, just like HP.

We define a difficulty-driven expected speed multiplier `μ_v(D)` that increases with D (faster movement is higher multiplier). Then we sample:

`SPDMul0 ~ TruncNormal(mean = μ_v(D), sd = 0.05, bounds = [0.6, 1.7]]`

Clamp to [0.6, 1.7] to avoid extreme outliers (too slow to matter or instantly teleport-fast). The actual `μ_v(D)` curve is tunable (same spirit as HP: soft early jump, mild multiplicative growth later).

### 3.4 Pressure target

We define a per-bug “pressure” function:
`pressure(hp, v) = α * hp + β * (v ^ γ)`

Where:

* `hp` = hit points of a bug
* `v` = speed multiplier (1.0 = baseline ~500 ms/step, 1.25 = ~400 ms/step, etc.)
* `α`, `β` > 0 and `γ` in [0.8, 1.2]
* Default recommended tuning:

  * `α = 1.0`
  * `β = 0.6`
  * `γ = 1.0`

Use the sampled HP and speed latents to compute a target per-bug threat:

* `hp_wave = H_base * HPmul0`
* `v_wave = SPDMul0`

Then define the target total wave pressure:

* `P_wave = round( Count * pressure(hp_wave, v_wave) )`

`P_wave` is the total pressure budget this wave “should” have. Later we will scale all components uniformly to line up their aggregate pressure with `P_wave`.

---

## 4. Procedural Species Components

This section defines how we create “species-like” components for this wave. These are not predefined types. They are sampled every wave.

### 4.1 Number of provisional components K

We first propose a number of components using difficulty, then clamp it so we never end up with silly 1-bug micro-species.

#### 4.1.1 Raw proposal

Sample:
`K_raw ~ Poisson( κ(D) )`

Where `κ(D)` is a difficulty-controlled mean. `κ(D)` is a tuning knob. Typical behavior:

* At low difficulty, `κ(D)` should be near ~1.2. That means most tutorial waves have one component, sometimes two.
* As difficulty increases, `κ(D)` can increase (e.g. toward ~3.5) so that higher waves are more likely to produce 2–3+ components.

Clamp:
`K_soft = min(K_raw, K_abs_max)`

Where:

* `K_abs_max = 6` (hard cap).

#### 4.1.2 Count-aware cap

We now compute how many components we can “afford” given `Count`, so that we don’t create species with almost no members.

Define:

* `min_share = 0.10` (10% minimum share per species)
* `m = max(1, ceil(min_share * Count))`
  This is the minimum acceptable count per species in this wave.

Then:

* `K_count_cap = max(1, floor(Count / m))`

Finally:

* `K = min(K_soft, K_count_cap)`
* `K = max(1, K)`

This ensures that if `Count` is small, `K` is forced to be small (often 1). If `Count` is large, `K` can be larger, but is still capped so that we don't expect species below ~10% share.

### 4.2 Component stat centers (no hardcoded list)

For each component `s` in `[0 .. K-1]`, we sample its “center” stats (HP and speed) from a correlated 2D distribution in log-space. This replaces any concept of predefined species tables.

We work in `(log HP multiplier, log speed multiplier)` space.

Let:

* `μ_log = ( log(μ_HPmul(D)), log(μ_v(D)) )`
  This centers the distribution around the difficulty-driven averages from §3.3.
* `Σ(D)` = covariance matrix in log space with:

  * standard deviations `σ_h` and `σ_v` ≈ 0.10 (log units),
  * correlation ρ ≈ -0.5 (negative correlation so tanky bugs tend to be slower and fast bugs tend to be fragile).

For each component:

1. Sample `(log_HPmult_s, log_SPDMul_s)` ~ TruncatedBivariateNormal(μ_log, Σ(D), bounds).

   * Bounds:

     * After exponentiation, `HPmult_s` must lie in [0.6, 2.2].
     * After exponentiation, `SPDmult_s` must lie in [0.6, 1.7].
   * If the draw falls outside those bounds, clamp to the boundary.
2. Compute:

   * `hp_s_pre = H_base * HPmult_s` (absolute HP for this component, pre-scaling)
   * `v_s_pre  = SPDMul_s`         (speed multiplier for this component, pre-scaling)
3. Compute the component’s unit pressure weight:

   * `w_s_pre = pressure(hp_s_pre, v_s_pre)`

Store `(hp_s_pre, v_s_pre, w_s_pre)` for each component `s`. These are pre-scale values. Final values will be scaled by η in §5.

### 4.3 Allocate total bug count across components

We need to split the total wave bug count `Count` across the `K` components.

1. Sample a symmetric Dirichlet of length K:

   * `p ~ Dirichlet( α_mix * 1⃗_K )`
   * `α_mix` should default to ~1.5

     * Lower `α_mix` → more lopsided (one dominant species).
     * Higher `α_mix` → more even splits.

2. Compute the ideal fractional assignment:

   * `c_s* = Count * p_s`

3. Convert to integers using Hamilton / largest remainder:

   * `n_s = floor(c_s*)`
   * `L = Count - Σ_s n_s`
   * Sort components by fractional remainder `frac(c_s*)` descending.

     * Tie-break by ascending component index.
   * For the first `L` components in that sorted order, increment `n_s` by 1.
   * After this, `Σ_s n_s == Count`, all `n_s` are integers, and the distribution is deterministic.

At this point:

* We have components, each with `(hp_s_pre, v_s_pre, w_s_pre, n_s)`.

Small components may still exist. We now apply the merge rule.

### 4.4 Minimum-share enforcement via deterministic merging

We require that after finalization, every component has either:

* At least 10% of total bugs in this wave, or
* We are down to exactly one component.

To enforce this, we perform a deterministic merge loop.

Definitions:

* `m = max(1, ceil(0.10 * Count))` (same meaning as above).
* A component `s` is “too small” if `n_s < m`.

Distance metric between two components `s` and `t`:

* Work in log space using pre-scale values.

  * `lh_s = log(hp_s_pre / H_base)`  (log HP multiplier)
  * `lv_s = log(v_s_pre)`            (log speed multiplier)
* Normalize by the same `σ_h`, `σ_v` used in Σ(D):

  * `d(s,t) = sqrt( ((lh_s - lh_t) / σ_h)^2 + ((lv_s - lv_t) / σ_v)^2 )`

Merge loop:

1. While there exists any component with `n_s < m` **and** `K > 1`:

   * Pick the “smallest” component `s*`:

     * The one with the smallest `n_s`.
     * Tie-break by lowest component index.
   * Pick its nearest neighbor `t*`:

     * The one (t != s*) with the smallest `d(s*, t)`.
     * Tie-break by lowest component index.
   * Merge `s*` into `t*`:

     * `n_t* = n_t* + n_s*`
     * Remove `s*` from the component list.
     * Decrement `K`.
     * Do not average or blend stats. After the merge, all bugs that used to be `s*` are now considered to be of type `t*`, with `t*`’s stats `(hp_t_pre, v_t_pre)`.
2. Continue until either:

   * `K == 1`, or
   * All remaining components satisfy `n_s >= m`.

After merging, reindex remaining components to consecutive `species_id` values 0..K-1.

This guarantees:

* For small waves, you typically end up with 1 component, sometimes 2, and you never get a “1 bug special snowflake” unless the whole wave is 1 species anyway.
* For large waves (e.g. hundreds of bugs), you can get multiple stable components, each with meaningful share (≥10%).

At the end of §4.4:

* `Σ_s n_s` must still equal `Count`.
* Each remaining component has `(hp_s_pre, v_s_pre, n_s)` and is guaranteed non-trivial by share, unless there is exactly one component.

---

## 5. Pressure Alignment via Global Scaling η

At this point:

* We have `K` merged components.
* Each component `s` has:

  * `hp_s_pre`
  * `v_s_pre`
  * `n_s`
* We have a target total wave pressure `P_wave` from §3.4.

We now apply one uniform global scaling factor `η` to bring the total realized pressure in line with `P_wave`.

### 5.1 Scaling definition

Final per-component stats are:

* `hp_s_final = η * hp_s_pre`
* `v_s_final  = η * v_s_pre`

The realized pressure after scaling is:
`P_actual(η) = Σ_s [ n_s * ( α * (η * hp_s_pre) + β * (η * v_s_pre)^γ ) ]`

We must choose `η` so that `P_actual(η)` equals `P_wave` as closely as possible, within clamps.

### 5.2 Solving for η

We find `η` by deterministic monotone bisection over a fixed range:

* Allowed range: `η_min = 0.75`, `η_max = 1.5`.
* Perform exactly N iterations of bisection (N must be fixed across builds; 24 is acceptable).

  * No early exit. Always run the full number of steps to guarantee deterministic float rounding behavior.
* After bisection, clamp `η` to `[η_min, η_max]`.

If the clamped `η` does not yield `P_actual(η) == P_wave`, accept the clamped result anyway. Do not introduce per-component scaling hacks.

After this step:

* Store `hp_s_final`, `v_s_final` for each remaining component `s`.

These final values are what will be put into spawn records.

---

## 6. Temporal Layout (Spawn Schedule)

This section defines how bugs are placed in time.

### 6.1 Difficulty-driven cadence and start offsets

Each component `s` gets a cadence (ms between spawns of that component) and an initial start offset.

We define:

* `μ_cad(D)`: mean per-component spawn cadence (ms between bugs of that component). This should generally *decrease* with difficulty. A reasonable initial functional form is:

  * `μ_cad(D) = clamp(180, 600 - 40*(D-1), 600)`
  * That means ~600ms between spawns at very low difficulty, trending toward ~180ms at higher difficulty.
  * This function is a tuning knob. Designers may change constants later.
* `μ_start(D)`: mean per-component initial delay before that component starts spawning. For example, ~1000ms at most difficulties. This is also a tuning knob.

We now sample per component:

* `Cad_s ~ TruncNormalInt(mean = μ_cad(D), sd = 0.08 * μ_cad(D), bounds = [cad_min, cad_max]]`

  * Required bounds: `cad_min = 120 ms`, `cad_max = 2000 ms`
  * Clamp to `[cad_min, cad_max]`.
* `Start_s ~ TruncNormalInt(mean = μ_start(D), sd = 0.15 * μ_start(D), bounds = [0, start_max]]`

  * Required bound: `start_max = 10000 ms`
  * Clamp to `[0, start_max]`.

The sampling order (Cad_s first, Start_s second or vice versa) must be fixed and consistent for determinism.

### 6.2 Per-bug timestamps

For each component `s` with `n_s` bugs:

* The i-th bug of that component (0-based index `i`) spawns at:

  * `time_ms = Start_s + i * Cad_s`

`time_ms` must be integer milliseconds.

### 6.3 Construct global spawn list

For each bug in each component:

* Create one spawn record with:

  * `time_ms` from §6.2
  * `hp = hp_s_final`
  * `speed_mult = v_s_final`
  * `species_id = s` (the component index after merges/reindexing)

Collect all spawn records across all components.

Sort the entire list by:

1. `time_ms` ascending
2. `species_id` ascending
3. index-within-that-species ascending

This sorted list is the working schedule.

### 6.4 Wave duration cap and cadence compression

We cap how long a wave is allowed to take to fully deploy.

Define a difficulty-dependent max deploy duration:

* `T_target(D)` (in ms). Example default: 60,000 ms (60 seconds). This is a tuning knob.

Compute:

* `T_end = max(time_ms)` over all spawn records.

If `T_end <= T_target(D)`, do nothing.

If `T_end > T_target(D)`:

1. Compute a global compression factor:

   * `c = T_end / T_target(D)` (note `c > 1` when we’re too long).
2. For each component’s cadence:

   * `Cad_s_compressed = max(cad_min, floor(Cad_s / c))`
   * `cad_min` is the same lower bound from §6.1 (120 ms).
   * Apply the same compression factor `c` to every component. Do not vary per component.
3. Recompute each `time_ms` for each bug using `Cad_s_compressed` and the original `Start_s`.
4. Re-sort the list as in §6.3.
5. Recompute `T_end`.

   * If `T_end` is still `> T_target(D)` only because `cad_min` prevented further compression, accept it anyway.

No other temporal manipulation is allowed.

---

## 7. Telemetry Requirements

The generator must emit deterministic telemetry metadata for debugging/balancing. This does not affect gameplay, but must be available to development tooling.

Telemetry must include:

1. `wave_overview`

   * `D`
   * `Count`
   * `P_wave`
   * `K_final` (number of components after merges)
   * `share_min` = `min_s(n_s) / Count`
   * `C_cap` and other current tuning knobs used for Count (so design can review curves)

2. `species_merge`
   Emit one record per merge in §4.4, containing:

   * `from_component`
   * `to_component`
   * `n_from`
   * `n_to_before`
   * `n_to_after`
   * the log-space distance used to choose nearest neighbor

3. `eta_scaling`

   * `eta_final`
   * `eta_clamped` (boolean; true if `η` hit `[η_min, η_max]`)
   * `P_wave`
   * `P_actual_after_eta`

4. `cadence_compression`

   * `T_end_before`
   * `T_target(D)`
   * `compression_factor c`
   * `any_component_hit_cad_min` (boolean)
   * `T_end_after`

If a certain event didn’t happen (e.g. no merge, no compression), telemetry still needs to be emitted with flags indicating that it did not trigger. Consumers must be able to assume the presence of these records.

---

## 8. Invariants and CI Assertions

All implementations must pass CI tests that assert the following invariants:

1. **Determinism**

   * For fixed `(game_seed, level_id, wave_index, D)` two runs must serialize to byte-identical output.

2. **Count preservation**

   * After merging (§4.4), the sum of bug counts across all remaining components must equal the original sampled `Count`.
   * No bug count may be created or destroyed by merging.

3. **Minimum-share or single-component**

   * After merging:

     * Either there is exactly one component, OR
     * Every remaining component `s` has `n_s >= ceil(0.10 * Count)`.

4. **Pressure alignment**

   * Let `η_final` be the final η from §5.
   * Compute `P_actual_final = Σ_s [ n_s * ( α * (η_final * hp_s_pre) + β * (η_final * v_s_pre)^γ ) ]`.
   * If `η_final` is strictly inside `(η_min, η_max)` then `P_actual_final` must match `P_wave` within numerical tolerance implied by the fixed-step bisection procedure.
   * If `η_final` equals `η_min` or `η_max`, `P_actual_final` must be on the correct side of `P_wave` (if clamped low, `P_actual_final <= P_wave`; if clamped high, `P_actual_final >= P_wave`) and telemetry must indicate clamp.

5. **Timing monotonicity**

   * After final cadence compression (§6.4), all spawn times `time_ms` must be ≥ 0 and integer.
   * After sorting (§6.3), for any two consecutive spawn records `(A,B)` in the final output:

     * Either `A.time_ms < B.time_ms`, or
     * `A.time_ms == B.time_ms` and `A.species_id <= B.species_id`, or
     * `A.time_ms == B.time_ms` and `A.species_id == B.species_id` and A’s index-within-species ≤ B’s.

6. **Duration cap handling**

   * After compression, `max(time_ms)` must be ≤ `T_target(D)` unless prevented by `cad_min`.
   * If `cad_min` prevented full compression, telemetry `cadence_compression.any_component_hit_cad_min` must be true.

7. **Bounds compliance**

   * All truncated normal and truncated bivariate normal draws must be clamped to their documented ranges before being used.
   * All final `hp_s_final` must be > 0.
   * All final `v_s_final` must be > 0.
   * All final cadences after compression must be ≥ `cad_min`.

8. **No improvisation**

   * The implementation must not introduce:

     * hardcoded species tables,
     * per-lane or per-patch routing logic,
     * performance-based adaptive difficulty in this generator,
     * per-wave hand-authored scripts.
   * All variety must come from the sampling and merge rules in this spec and from the tunable curves/knobs defined here (`C_cap`, `κ(D)`, `μ_v(D)`, `μ_hp(D)`, etc.).

---

## 9. Summary of Required Behavior

* Each wave is generated by seeded randomness using a fixed draw order and becomes reproducible for that `(game_seed, level_id, wave_index, D)`.
* Bug count is sampled from a truncated normal around a logistic-style mean `μ_count(D)` which ramps rapidly early in difficulty and continues to increase with D but with slowing growth toward a configurable ceiling `C_cap`. `C_cap` is a designer knob and can be set to values like 500 or 1000+.
* Per-wave average HP and speed multipliers are sampled from truncated normals around difficulty-driven curves that keep increasing with difficulty. HP and speed scaling continue to rise with D without any built-in stop.
* The wave’s total pressure budget `P_wave` is computed from the sampled bug count and sampled average HP/speed for that wave.
* A provisional number of components (species) is sampled with a Poisson (`κ(D)`) and clamped by both an absolute max (`K_abs_max`) and by the total bug count to prevent trivial species.
* Each component’s HP and speed center is sampled from a correlated bivariate normal in log space, giving organically different “types” without hardcoded species tables.
* The total bug count is allocated across components via Dirichlet proportions and deterministic Hamilton apportionment.
* Components that fall below ~10% of total bug count are deterministically merged into their nearest neighbor in stat-space until each remaining component has at least that ~10% share or only one component is left. This guarantees meaningful components and prevents “1-bug” filler species, while still allowing genuine single-species waves.
* All component stats are uniformly scaled by a single global factor `η` chosen so that the sum of per-bug pressures across all spawned bugs matches the intended total wave pressure `P_wave`, within defined clamps.
* Each component gets a cadence and start offset sampled from truncated normals. Individual spawn timestamps are generated as arithmetic progressions per component.
* If the wave would take longer than the allowed per-difficulty duration `T_target(D)`, all cadences are globally compressed (respecting a global `cad_min`) so the wave finishes within the time budget when possible.
* The final spawn list (time_ms, hp, speed_mult, species_id) is sorted deterministically and returned.

This specification is the single source of truth for wave generation behavior. All implementations must follow it exactly.
