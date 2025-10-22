# Sprite Rendering Technical Specification

## 1. Objective
Introduce sprite-based rendering for towers and bugs while keeping the current
primitive-based drawing path available for rapid prototyping. Towers should
render as square sprites composed of a static base and a turret that rotates to
track targets, and bugs should replace the existing circle primitive with a
sprite. The architecture must preserve the existing adapter boundaries and
maintain deterministic behaviour.

## 2. Current Rendering Mechanism Audit
* **Layered design remains clean.** `adapters/rendering` exposes declarative
  scene descriptions (`Scene`, `SceneTower`, `BugPresentation`, etc.) and knows
  nothing about the actual drawing API. `adapters/rendering_macroquad` converts
  those descriptors into Macroquad calls. No systems or world code reference the
  rendering backend directly, so injecting a sprite renderer does not require
  touching simulation logic.
* **Primitive assumptions are baked into descriptors.** `SceneTower` exposes a
  `CellRect` footprint with no visual styling, and the Macroquad backend always
  turns that into a filled rectangle with an outline. `BugPresentation` only
  conveys cell coordinates plus a fill colour, and the backend renders circles.
  Replacing primitives requires enriching the shared descriptors so that both
  the sprite and primitive code paths can be chosen explicitly.
* **Backend currently stateless regarding visuals.** Macroquad helper functions
  (`draw_towers`, inline bug loop) are free functions that read the scene each
  frame. No persistent state caches textures or per-tower orientation yet, which
  means sprite assets will need an explicit loader/caching layer inside the
  backend.
* **Flexibility verdict.** The data-driven scene contract makes the renderer
  swappable, but the descriptors themselves must gain enough information to
  request sprites. The change is limited to the rendering crates; systems and
  world state remain untouched except for optional helper queries to derive
  turret orientation defaults.

## 3. Goals & Non-Goals
* **Goals**
  * Support per-entity render styles so that sprites and primitives can coexist.
  * Load and draw tower base/turret sprites with turret rotation driven by the
    targeting system.
  * Replace bug circles with sprites while preserving deterministic positions
    derived from cell coordinates.
  * Provide an asset handling story that works inside Git and CI.
* **Non-Goals**
  * Introducing animation systems beyond turret rotation.
  * Refactoring systems/world to expose entirely new data flows; the scene
    contract remains a pure projection layer.

## 4. Shared Rendering Contract Changes (`adapters/rendering`)
1. **Introduce sprite descriptors**
   * Add a new `SpriteKey` enum listing built-in assets (`TowerBase`,
     `TowerTurret`, `BugBody`, with room for variants).
   * Add a `SpriteInstance` struct containing:
     * `sprite: SpriteKey`
     * `size: Vec2` in cells (allows uniform scaling from logical cell units)
     * `pivot: Vec2` in 0.0–1.0 texture space (default `(0.5, 0.5)`)
     * `rotation_radians: f32`
     * Optional `offset: Vec2` in cells for fine alignment.
2. **Render-style enums**
   * Extend `SceneTower` with a new `visual: TowerVisual` field:
     ```rust
     pub enum TowerVisual {
         PrimitiveRect,
         Sprite {
             base: SpriteInstance,
             turret: SpriteInstance,
         },
     }
     ```
     * `PrimitiveRect` keeps current behaviour; `Sprite` requests sprite
       rendering. The constructor of `SceneTower` should default to
       `PrimitiveRect` so existing callers compile.
   * Replace `BugPresentation` with:
     ```rust
     pub struct BugPresentation {
         pub column: u32,
         pub row: u32,
         pub style: BugVisual,
     }
     
     pub enum BugVisual {
         PrimitiveCircle { color: Color },
         Sprite(SpriteInstance),
     }
     ```
     * Update helper constructors (`BugPresentation::new_circle`,
       `BugPresentation::new_sprite`) to make the transition explicit.
3. **Scene compatibility helpers**
   * Provide conversion helpers that build sprite descriptors for existing tower
     kinds and bug styles, enabling the bootstrap/system layer to opt into
     sprites without duplicating math.
   * Document deterministic expectations (no runtime randomness when choosing
     sprites, rotation strictly derived from scene data).

## 5. Scene Population Updates
1. **Tower visuals**
   * Extend the adapter-side scene population (likely in
     `systems/bootstrap` or wherever `SceneTower` entries are emitted) to choose
     between `PrimitiveRect` and `Sprite` based on configuration or tower kind.
   * Default shipped towers should emit the sprite visual; future prototype
     towers can intentionally select `PrimitiveRect`.
   * Provide a deterministic helper that builds the turret `SpriteInstance` from
     the tower footprint (`CellRect`) plus a turret bearing angle. The base
     sprite remains axis-aligned with zero rotation.
2. **Turret rotation input**
   * Reuse `TowerTargetLine` data to derive the turret angle. When a tower has a
     target line, compute `atan2(to.y - from.y, to.x - from.x)` (convert from
     cell units to radians). When a tower lacks a target, fall back to the last
     known orientation stored in adapter state (see §6) or a default facing up.
   * Optionally expose a helper in `adapters/rendering` that computes this angle
     from `TowerTargetLine` so both backends and tests share the logic.
3. **Bug visuals**
   * During scene population, emit `BugVisual::Sprite` for all bugs once assets
     exist. Provide a feature flag / configuration knob (e.g. CLI `--bug-style
     primitives`) to force `PrimitiveCircle` when art is missing for new bug
     types.

## 6. Macroquad Backend Changes (`adapters/rendering_macroquad`)
1. **Sprite module**
   * Create `sprites.rs` that owns asset loading and draw helpers.
     ```rust
     pub struct SpriteAtlas {
         textures: HashMap<SpriteKey, Texture2D>,
     }
     impl SpriteAtlas {
         pub fn new() -> AnyResult<Self> { /* load textures once */ }
         pub fn draw(&self, key: SpriteKey, params: DrawParams) { /* wraps macroquad */ }
     }
     ```
     * `DrawParams` mirrors Macroquad's `DrawTextureParams` (position, scale,
       rotation, pivot, tint). The module isolates Macroquad-specific concerns
       (async loading, path resolution) from the main loop.
2. **Backend state extension**
   * Extend `MacroquadBackend` to hold:
     * `sprite_atlas: SpriteAtlas`
     * `turret_headings: HashMap<TowerId, f32>` (last known rotation radians)
   * Initialise the atlas before entering the render loop. Asset loading should
     be synchronous at startup to avoid frame hitches and ensure deterministic
     readiness.
3. **Drawing logic**
   * Update `draw_towers`:
     * For `PrimitiveRect`, call the existing rectangle routine.
     * For sprite towers:
       1. Compute world-space origin from `CellRect` as today.
       2. Draw base sprite using atlas at the rectangle bounds (scale by cell
          width/height).
       3. Resolve turret rotation: look for an entry in `tower_targets`; if
          found, compute angle and store it in `turret_headings`. If not, re-use
          cached heading or default to 0 radians.
       4. Draw turret sprite using atlas with `rotation` set to heading and
          pivot at sprite centre. The turret shares the tower centre as its
          position.
   * Replace inline bug loop with a `draw_bugs` helper:
     * For `PrimitiveCircle`, keep current circle drawing.
     * For sprite style, position the sprite so that its centre aligns with the
       cell centre, scaling it to `SpriteInstance.size * cell_step`.
4. **Runtime switching**
   * Add a renderer-level configuration (env var or CLI flag consumed via the
     existing adapter options) to default all towers/bugs to primitive visuals.
     The scene population helpers must respect this flag. This keeps the
     fallback path testable.
5. **Determinism considerations**
   * Ensure rotation calculations operate purely on scene data and cached values
     updated deterministically with frame order. Avoid relying on wall-clock
     time or floating-point easing that depends on dt — turret rotation should
     snap immediately to target bearing to remain deterministic.
   * Sprite loading paths must return errors deterministically (e.g. missing
     file) so tests can fail predictably.

## 7. Asset Layout & Git Handling
* Store textures under `assets/sprites/` with clear subdirectories:
  * `assets/sprites/towers/base.png`, `assets/sprites/towers/turret.png`
  * `assets/sprites/bugs/bug.png`
* Add a repository-level `.gitattributes` entry to route `*.png` (and future
  binary art assets) through Git LFS. CI should validate that LFS pointers are
  present. If we intentionally keep assets tiny (<100KB), document when plain
  Git blobs are acceptable to avoid unnecessary LFS churn.
* Document asset licensing/attribution in `assets/README.md` and reference it
  from the main README so contributors know how to add or replace sprites.
* Add a simple `assets/manifest.toml` (or JSON) listing sprite keys and file
  paths so the `SpriteAtlas` loader does not hardcode file names; this keeps
  replacements straightforward and avoids recompilation when swapping art.

## 8. Testing & Tooling
* Extend backend smoke tests to cover both render styles: ensure the helper that
  maps `SceneTower` visuals to draw commands produces expected parameters.
  (Macroquad integration tests remain headless by comparing computed draw
  batches where feasible.)
* Add unit tests in `adapters/rendering` to confirm `SpriteInstance` helpers
  compute consistent pivots, sizes, and rotation defaults.
* Update documentation (README or dedicated rendering guide) once sprites land,
  noting how to toggle primitive fallback for prototyping.

## 9. Rollout Plan
1. Land contract changes in `adapters/rendering` alongside temporary primitive
   constructors to keep callers compiling.
2. Update scene population to opt into sprite visuals behind a feature flag.
3. Introduce the Macroquad sprite atlas and drawing helpers, keeping the old
   primitive code path alongside it.
4. Add assets and asset manifest with Git LFS configuration.
5. Default the game to sprite visuals once the art pipeline and tests stabilise;
   retain a CLI flag for primitive fallback for ongoing prototyping.
