# Builder Mode Foundation Plan (Final Architecture Proposal)

## Intent

Introduce a **first-class play mode system** (Attack vs Builder) into the engine in a way that:

* Preserves the existing **message-driven, adapter-agnostic flow**.
* Keeps **World as the only source of authoritative state**.
* Allows adapters to **render a UI preview** purely from projection data — never by mutating simulation.
* Lays a stable foundation for future builder capabilities (tower placement, multiplayer, rollback, replay).

---

## Mode as a core message concept

* Introduce a `PlayMode` enum in the **core contract layer** — side-by-side with other cross-layer types.
* Add a new core command, e.g. `SetPlayMode`, and emit a matching `PlayModeChanged` event.
* This ensures **all layers (World, systems, adapters) react explicitly**, rather than inferring or guessing mode state.

---

## World-owned mode and lifecycle

* The **World stores the active mode** as explicit state.
* `SetPlayMode` is handled solely through the World’s existing `apply` path — no adapters reach inside.
* On entering Builder mode:

  * **remove bugs and clear occupancy** deterministically.
* On returning to Attack mode:

  * **deterministically reseed bugs**, as already done on world reset/boot.
* No “scratch buffers” or dual representations — state is either visible or gone.
* Further world commands (like `Tick` and `StepBug`) should **simply no-op at the world layer** while in Builder mode, as a safety barrier.
* The **movement system may independently guard** based on the `PlayModeChanged` event for clarity — a redundant guard here is appropriate.

---

## Input as data, not behavior

* Adapters (e.g. macroquad) should **only report per-frame inputs** — never directly “switch modes”.
* A minimal per-frame input struct is sufficient — e.g.:

  * Whether a toggle action (space press) occurred.
  * The current cursor position in world/tile space.
* The **simulation/CLI layer decides** when to emit `SetPlayMode`, using that input.
* Cursor position is merely **cached alongside mode** — not applied or mutated anywhere outside the World.

---

## Scene projection & builder overlay

* The output `Scene` gains **two optional projections**:

  * The **current play mode** (for adapters to branch rendering).
  * A **builder overlay / placement preview**, expressed in **tile-space coordinates** — never pixels.
* The snapping logic (half-tile increments, clamped to grid bounds) is computed in the **simulation populate path**, not in the renderer and not in the world.
* The rendering layer receives **pure, declarative instructions** — e.g. “draw a tile-sized translucent square at x,y”. No new logic needs to exist there beyond drawing.

---

## Determinism and future-proofing

* Switching modes is **idempotent** — issuing the same `SetPlayMode` twice in a row is harmless, and emits no duplicate events.
* All future builder actions (tower placement, cost previews, material checks, etc.) can piggyback on the **same play-mode gating + projection pipeline** — no need to rethink the architecture later.
* The state is always explicitly replayable and serializable — no transient caches, no ambiguous “hidden state”.

---

## Testability guarantees

* **World tests** should confirm bugs and occupancy vanish in Builder mode and reappear deterministically in Attack mode.
* **Simulation/system tests** ensure no movement output is ever produced in Builder mode.
* **Snapping tests** validate half-tile snapping + clamping at grid borders, with no drift.
* This architecture is enforceable — if a future developer attempts to “cheat” around the contract, tests will break immediately.

---

## Summary

This proposal:

* Keeps **all logic explicit** via commands, events, and projection.
* Introduces exactly **one new concept** (PlayMode) into the shared contract layer.
* Protects the **directional layering**: adapter → command → world → projection → render.
* Ensures **Builder mode is not special-cased or backdoored**, just another first-class play state — fully replayable and observable.
* Leaves **implementation decisions** (data shapes, helper naming, tile snap math details) to the devs who live in the codebase.

---

# Builder Mode Foundation — Implementation Deliverables

## Phase 0 

**Deliverables**

* ✅ Write a spec for builder mode.
* ✅ Add a clear roadmap for how to implement builder mode.

**Goal**

* Contributors can easily understand and start working on the implementation of builder mode.

## Phase 1 — Introduce explicit play mode as a first-class concept

**Deliverables**

* [x] Add `PlayMode` enum to core contract layer (Attack, Builder).
* [x] Add `Command::SetPlayMode { mode: PlayMode }`.
* [x] Add `Event::PlayModeChanged { mode: PlayMode }`.
* [x] Add/update any `#[derive]` or enum match exhaustiveness needed.

**Goal**

* All mode switching is now explicit and message-driven — nothing implicit, no adapter direct access.

---

## Phase 2 — World becomes authoritative owner of play mode

**Deliverables**

* [x] Add `play_mode: PlayMode` to `World`, default = `Attack`.
* [x] Update `World::apply` to handle `SetPlayMode`:

  * Toggle modes only when actually changed (idempotent).
  * On Builder → clear **all** bugs + occupancy deterministically.
  * On Attack → reseed bugs deterministically (existing mechanism).
  * Emit `PlayModeChanged` after mutation.
* [x] Add public query accessor: `query::play_mode(world)`.

**Goal**

* The world is always the single source of truth for mode.
* Nothing else is allowed to “fake it”.

---

## Phase 3 — Enforce mode behavior guarantees (deterministic shields)

**Deliverables**

* [x] Add **early bail** inside world `Tick` / `StepBug` handlers if mode = Builder.
* [x] Optionally, have the movement system also check mode before emitting commands.

**Goal**

* Nothing can make bugs move while builder mode is active — even if a layer is misbehaving.

---

## Phase 4 — Adapter input is data, not behavior

**Deliverables**

* [x] Add a shared `FrameInput` type to `adapters/rendering` exposing **only** the information the simulation may consume:

  * `mode_toggle: bool` — true on the frame the adapter detected a toggle press.
  * `cursor_world_space: Option<glam::Vec2>` — world-space cursor, already clamped to the playable grid bounds.
  * `cursor_tile_space: Option<TileSpacePosition>` — tile snapped position supplied as data, never derived inside the simulation.

* [x] Change `RenderingBackend::run` to pass `FrameInput` into the callback (`FnMut(Duration, FrameInput, &mut Scene)`).
* [x] Update `MacroquadBackend` to populate `FrameInput` (space bar edge detection + mouse position translation) while remaining **purely observational**.
* [x] Have the CLI adapter fabricate a `FrameInput::default()` so headless tests keep compiling.

**Goal**

* Adapters only **observe and report**, never decide.

---

## Phase 5 — Simulation drives mode transitions and preview

**Deliverables**

* [x] Extend `Simulation` (in `adapters/cli`) with a `pending_input: FrameInput` cache consumed once per frame.
* [x] Add `Simulation::handle_input(&mut self, input: FrameInput)` that:

  * Detects toggle edges and pushes `Command::SetPlayMode { … }` into the command queue.
  * Stores the most recent cursor positions in tile/world space for rendering.

* [x] Call `handle_input` from the adapter right before `advance`.
* [x] Update `Simulation::populate_scene` to:

  * Always attach the current `PlayMode` to the outgoing `Scene`.
  * When in Builder mode, emit a `PlacementPreview` descriptor built solely from cached input + grid metrics.

**Goal**

* Correct place to combine world + adapter signals into renderable projection — purely declarative.

---

## Phase 6 — Rendering consumes projection only

**Deliverables**

* [x] Add `PlayMode` passthrough and a `PlacementPreview` struct (tile coordinate + size) to `Scene` in `adapters/rendering`.
* [x] Ensure `Scene::new`/`Scene` tests cover the new fields without introducing builder-only defaults.
* [x] Macroquad (and any future renderer) draws previews strictly from `Scene.placement_preview` — **no additional math or grid queries** performed in the adapter.

**Goal**

* Render layer never guesses, only follows.

---

## Phase 7 — Integration test coverage

**Deliverables**

* [x] World unit test: SetPlayMode → bugs/occupancy vanish & reappear correctly.
* [x] Mode idempotence test (same mode twice = no extra events).
* [x] Movement/system test: no commands in builder mode.
* [x] Simulation test covering `handle_input` edge detection and preview caching (headless, no renderer).
* [x] Rendering crate unit tests validating snapping/clamping math for `PlacementPreview`.
* [x] (Optional) Macroquad smoke test ensuring preview defaults to `None` while in Attack mode.

**Goal**

* The architecture enforces its own correctness.
  If someone tries to cheat in the future — it breaks immediately.

---

### End Condition

✅ Everything above is implemented and tested.
✅ Bugs vanish instantly when entering builder mode.
✅ Preview square appears, snaps, never leaves grid.
✅ State is fully deterministic, future tower placement can plug into it directly.
✅ No layer violates the AGENTS architecture.
