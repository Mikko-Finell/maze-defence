Read `sprite-spec.md` first so the rendering contracts and asset expectations are
fully understood before touching any code.

Here’s the lowest-risk sequence for landing sprite rendering alongside the
existing primitive pipeline. Each step is mergeable on its own and builds new
functionality only after the foundational types, configuration, and asset
plumbing exist.

# 1) Rendering contract scaffolding (`adapters/rendering`)

**Status:** DONE

**Goal:** Extend the shared rendering crate with sprite-aware descriptors while
keeping all existing call sites compiling.

**Deliverables:**

* Introduce `SpriteKey`, `SpriteInstance`, `TowerVisual`, and `BugVisual` as
  described in `sprite-spec.md`.
* Extend `SceneTower` with a `visual: TowerVisual` field and default existing
  constructors (and tests) to `TowerVisual::PrimitiveRect`.
* Replace `BugPresentation` with the struct/enum split outlined in the spec and
  update helper constructors so primitive behaviour is explicit.
* Adjust `Scene::new` and any other constructors/tests that expect the previous
  `BugPresentation` layout.
* Document deterministic expectations on the new enums/structs, especially the
  fact that `SpriteInstance` values are in cell units and may not reference
  external assets directly.

**Exit checks:** `maze_defence_rendering` compiles, all doctests/unit tests pass,
existing adapters compile without behaviour changes when they continue to choose
primitive visuals.

# 2) Descriptor helpers & orientation math (`adapters/rendering`)

**Status:** DONE

**Goal:** Provide deterministic helpers that systems/adapters can call to build
sprite descriptors without duplicating geometry math or rotation logic.

**Deliverables:**

* Add a `visuals` (or similar) module exporting helpers such as
  `tower_sprite_visual(region: CellRect, heading_radians: f32) -> TowerVisual`
  and `bug_sprite_visual(column: u32, row: u32, key: SpriteKey) -> BugVisual`.
* Implement `SpriteInstance::square(size_cells: Vec2)` and a helper that turns a
  `TowerTargetLine` into a rotation angle using `atan2`, clamped to `[−π, π]`.
* Document and unit-test the helpers so that pivot defaults, scaling math, and
  rotation calculations stay deterministic.

**Exit checks:** New helper tests cover pivot defaults, scaling math, and angle
rounding. Downstream crates still compile (helpers are additive only).

# 3) Asset manifest & Git plumbing (`assets/` + repo root)

**Status:** DONE

**Goal:** Stand up the repository structure that backs `SpriteKey` values with
on-disk assets and ensures Git handles binaries deterministically.

**Deliverables:**

* Create `assets/sprites/` with placeholder README stubs documenting expected
  files (`towers/base.png`, `towers/turret.png`, `bugs/bug.png`, etc.).
* Add `assets/manifest.toml` mapping each `SpriteKey` to a relative asset path
  and include comments about future extension points.
* Update `.gitattributes` to route `*.png` (and other binary sprite formats) via
  Git LFS unless a documented size exemption is used.
* Extend `assets/README.md` (or create it) to describe licensing expectations and
  the manifest editing process.

**Exit checks:** `git lfs track` (if used) reports the new patterns, and the repo
builds/tests unchanged since no code depends on the manifest yet. Placeholder
README files, a versioned manifest, and Git LFS rules now live alongside the
asset directories so future commits can drop in art without structural churn.

# 4) Visual style configuration (`adapters/cli`)

**Status:** DONE

**Goal:** Allow the CLI adapter to request either primitive or sprite visuals so
feature flags and fallback paths stay easy to exercise.

**Deliverables:**

* Introduce a `VisualStyle` enum (e.g. `Sprites` vs `Primitives`) parsed from a
  new `--visual-style` CLI option and default it to sprites.
* Thread the chosen style into `Simulation` so `populate_scene` can decide which
  `TowerVisual`/`BugVisual` variant to emit.
* Update CLI help text and any README references documenting the new flag.

**Exit checks:** CLI argument parsing tests cover both styles, and running with
`--visual-style primitives` continues to render rectangles/circles exactly as
before.

# 5) Scene population upgrade (`adapters/cli::Simulation`)

**Status:** DONE

**Goal:** Emit sprite-aware visuals from the scene population path while keeping
primitive fallbacks available.

**Deliverables:**

* Thread the selected `VisualStyle` into `populate_scene` so sprite vs primitive
  output can be chosen per frame.
* When sprite mode is active, call the helpers from step 2 to build base/turret
  `SpriteInstance`s sized from each tower’s `CellRect`, defaulting turret
  rotation to face up (backend orientation cache will adjust the heading after
  scene population).
* Emit `BugVisual::Sprite` for bugs while still supporting
  `BugVisual::PrimitiveCircle { .. }` when the fallback style is requested or
  assets are unavailable.
* Preserve the primitive constructors when the fallback style is requested.

**Exit checks:** Adapter unit tests cover both styles. Manual smoke test shows
sprite assets appearing with correct sizing, and primitive mode matches the
previous frames pixel-for-pixel.

# 6) Macroquad sprite module & state (`adapters/rendering_macroquad`)

**Status:** TODO

**Goal:** Teach the Macroquad backend how to load sprite assets once and retain
per-frame orientation state required for smooth turret rotation.

**Deliverables:**

* Add `sprites.rs` exposing `SpriteAtlas` with synchronous loading from
  `assets/manifest.toml`, `DrawParams` mirroring Macroquad’s `DrawTextureParams`,
  and a `draw` method that applies scaling/pivots as described in the spec.
* Extend `MacroquadBackend` with fields for `SpriteAtlas` and a
  `HashMap<TowerId, f32>` reused across frames to remember the last turret
  orientation whenever no target is active.
* Initialise the atlas during backend startup; fail fast (with `Anyhow` errors)
  if assets are missing so determinism is preserved.

**Exit checks:** Backend unit tests mock the manifest to prove loading order is
stable and repeated draws reuse cached textures. Existing CLI launch paths still
compile after wiring the new struct fields.

# 7) Drawing integration & runtime switching (`adapters/rendering_macroquad`)

**Status:** TODO

**Goal:** Render the new visuals while keeping the primitive code path intact
for fallback mode.

**Deliverables:**

* Update `draw_towers` to branch on `TowerVisual`, using the atlas for sprite
  towers (base + turret) and keeping the existing rectangle routine for
  primitives. Ensure turret headings consult the cache from step 6.
* Extract the bug loop into `draw_bugs` that respects `BugVisual` and reuses the
  atlas when sprites are requested.
* Thread the CLI style selection into the backend (e.g. via presentation or a
  shared config) so tests can toggle modes at runtime without recompilation.

**Exit checks:** Backend tests validate that sprite towers honour cached
headings, and rendering smoke tests (golden frame or parameter assertions)
exercise both sprite and primitive branches.

# 8) Determinism, tests & documentation

**Status:** TODO

**Goal:** Lock in the new behaviour with deterministic coverage and contributor
guidance.

**Deliverables:**

* Extend existing deterministic replay/golden scene tests to cover sprite and
  primitive modes, ensuring sprite-specific data (like turret headings) remains
  stable across runs.
* Add unit tests for any new helper modules in the Macroquad crate (e.g.
  verifying draw parameter calculations) and update CI scripts if additional
  asset checks are required.
* Update README/architecture docs to mention sprite assets, manifest workflow,
  and the runtime flag for toggling primitive fallback.

**Exit checks:** `cargo fmt --check`, `cargo clippy --deny warnings`, `cargo test`
(pass), and any sprite-specific checks (like manifest validation) succeed. Docs
are updated to point contributors at the new asset pipeline.
