Read `pressure-spec-v2.md` start to finish before touching code. This roadmap replaces
the legacy attack wave generator outright with a new implementation that conforms to
the v2 contract. Every checkpoint below is mergeable, keeps the architecture guards
intact, and assumes we are ripping out the old system immediately—no shims, no dual
writes, no long migrations.

# 1) [DONE] Cut over to empty v2 scaffolding (core + systems)

**Goal:** Remove the existing pressure generator entirely and replace it with stub
surfaces that match the v2 data flow.

**Deliverables:**

- [x] Delete the old wave/pressure modules, messages, and tests. Remove `tier` naming and
  replace call sites with the new `difficulty` terminology even if values are temporary.
- [x] Introduce new core message types that mirror the `pressure-spec-v2.md` inputs/outputs
  (seed, level id, wave index, difficulty, plus spawn records with `hp`, `speed_mult`,
  `species_id`, and `time_ms`). Leave constructors unimplemented (`todo!()`) so no code
  depends on behaviour yet.
- [x] Create a fresh `systems/pressure_v2` crate exposing a deterministic `PressureV2`
  struct with `generate(&mut self, inputs, out: &mut Vec<SpawnRecord>)`. Stub it to
  clear the output and `todo!()` for now.
- [x] Update any adapters or tests that referenced the old system to fail fast (panic with
  "pressure v2 not implemented") so there is no lingering legacy path.

**Exit checks:** No references to the legacy generator remain; all compile errors stem
from intentionally unimplemented v2 stubs only.

# 2) [DONE] Deterministic RNG + telemetry spine

**Goal:** Establish the deterministic PRNG wiring, telemetry emission, and the single
configuration surface that every knob flows through.

**Deliverables:**

- [x] Add a `PressureTuning` struct owned by the system that aggregates every adjustable
  knob (count curve params, HP/speed curves, component caps, cadence bounds). Expose a
  `pub fn tuning_mut(&mut self) -> &mut PressureTuning` so designers have one obvious
  surface. Document each field with comments explaining how it affects bug count,
  health, speed, species allocation, and cadence.
- [x] Replace any scattered config files with direct construction of `PressureTuning` in
  adapters. Document inline that the struct is the only supported entry point for
  tweaking wave behaviour.
- [x] Implement deterministic seeding per §1.3 (hash of `(game_seed, level_id, wave_index,
  difficulty)`) and keep the RNG in the `PressureV2` struct.
- [x] Introduce telemetry record builders for the required streams (`difficulty_latents`,
  `species_merge`, `eta_scaling`, `cadence_compression`) even if they currently emit
  placeholder values. Ensure the RNG consumption order is fixed and documented in
  comments next to each draw.

**Exit checks:** System compiles with deterministic seeding, tuning struct has exhaustive
comments, and telemetry structs exist with unit tests verifying seed hashing and RNG
usage order.

# 3) [DONE] Difficulty latents + pressure budget

**Goal:** Implement §3 of the spec end-to-end using the new tuning surface.

**Deliverables:**

- [x] Encode the logistic bug count curve, truncated normal sampling, and HP/speed latent
  draws using the deterministic RNG. Document which tuning fields control ceilings,
  slopes, and variance.
- [x] Compute `P_wave` exactly as defined, store it on the work buffer, and emit
  `difficulty_latents` telemetry with sampled values and tuning parameters.
- [x] Ensure all RNG draws are clamped to the documented bounds and record their order in
  comments.

**Exit checks:** Unit tests cover low/high difficulty cases, confirming tuning knobs
produce monotonic count/HP/speed changes and that telemetry serialises expected data.

# 4) [TODO] Species sampling, allocation, and merging

**Goal:** Fulfil §4 of the spec with deterministic merges and unique sprite tint
assignment for each species.

**Deliverables:**

- [x] Sample provisional species count `K`, per-species HP/speed centres, and Dirichlet
  proportions using the RNG spine. Allocate integer bug counts via Hamilton
  apportionment and store intermediate state in a work buffer.
- [x] Implement the “no tiny species” merge algorithm with deterministic nearest-neighbour
  selection. Emit `species_merge` telemetry for every merge (or an explicit no-merge
  event) and update indices accordingly.
- [ ] Generate a distinct `macroquad::Color` per final species by sampling hues/saturations
  with the same RNG. Assign the tint alongside species stats and ensure colours are
  deduplicated (retry draws if necessary) so every species renders differently.

**Exit checks:** Unit tests assert count preservation, minimum share guarantees, and
colour uniqueness for up to the configured species cap. Telemetry tests cover merge and
no-merge cases.

# 5) [TODO] Global scaling + cadence realisation

**Goal:** Apply the global `η` scaling, cadence sampling, and duration compression from
§5–§6 while keeping RNG usage deterministic.

**Deliverables:**

- [ ] Implement the bisection scaling loop that enforces total pressure alignment, emitting
  `eta_scaling` telemetry with pre/post pressure values and clamp flags.
- [ ] Sample per-species cadence and start offsets (document which tuning fields control
  ranges), generate arithmetic progression spawn times, and store them in per-species
  buffers.
- [ ] Enforce duration caps with global compression and `cad_min` logic, emitting
  `cadence_compression` telemetry even when no compression occurs.
- [ ] Produce the final sorted spawn list and write it into the caller-provided buffer with
  stable ordering.

**Exit checks:** Determinism tests confirm identical output across seeds; compression
coverage tests verify both clamped and unclamped flows.

# 6) [TODO] World + adapter integration

**Goal:** Wire the new generator into the world tick and rendering without reintroducing
legacy pieces.

**Deliverables:**

- [ ] Update world/apply code to request waves from `PressureV2`, replace `tier` naming with
  `difficulty` everywhere, and persist per-bug spawn records using the new types.
- [ ] Spawn bugs from multiple map locations by assigning each species a band of 5–10
  consecutive spawner cells sampled (deterministically) around the map instead of the
  fixed top-left cell. Ensure spawn centres fire at the maximum allowed cadence (each
  tick once the previous bug vacates) by respecting the generated cadence and per-cell
  queueing.
- [ ] Extend adapters/renderers to use the species tint when drawing bugs and to place spawn
  effects at the assigned spawner cells.

**Exit checks:** Integration tests confirm bugs spawn from multiple cells with matching
colours, replay harness stays deterministic, and no references to the removed legacy
system remain.

# 7) [TODO] Documentation + harness coverage

**Goal:** Lock in determinism and document the final architecture.

**Deliverables:**

- [ ] Add headless replay tests covering representative waves (low/high difficulty,
  compression engaged, multi-species merges) and assert telemetry streams.
- [ ] Update `architecture.md`, `pressure-spec.md` (legacy pointer), and any onboarding docs
  to reference the new `pressure-impl.md` plan, the `PressureTuning` entry point, and
  the v2-only implementation.
- [ ] Document the RNG ordering and tuning field meanings directly in code comments so
  future tweaks know exactly which knob affects HP, speed, counts, and cadence.

**Exit checks:** All docs are updated, replay tests are deterministic, and designers have
clear guidance on modifying wave behaviour exclusively through the new tuning struct.
