Read `gameplay.md` (control panel section) and the Macroquad adapter modules in
`adapters/macroquad` before touching code so the rendering/input contracts stay
aligned.

This roadmap follows the other `*-impl.md` guides: every stage is mergeable on
its own, respects the architecture guardrails, and layers behaviour from crate
wiring → adapter integration → deterministic coverage. Each checkpoint tracks
progress with `[TODO]`/`[DONE]` markers.

# 1) [DONE] Enable the UI surface (Cargo + adapter scaffolding)

**Goal:** Confirm Macroquad's UI module is wired into the renderer and prepare a
safe entry point inside the adapter for immediate-mode widgets.

**Deliverables:**

* Document in `adapters/macroquad` that the backend now relies on the
  `macroquad::ui` module while keeping the dependency minimal (still disabling
  default features).
* Add a tiny helper module (e.g. `adapters::macroquad::ui`) that exposes a
  single `fn draw_control_panel_ui(ctx: &mut Ui)` hook, keeping all
  immediate-mode calls in one place and shielding the rest of the adapter from
  UI-specific imports.
* Thread a `Ui` handle into the adapter's main loop (most likely within
  `adapters::macroquad::run::frame`) without disturbing existing
  `FrameInput`/`FrameOutput` flow.

**Exit checks:** Workspace compiles with UI support, the helper module is
covered by docs explaining its contract, and no other crates import Macroquad
UI types directly. Completed by documenting the new dependency scope,
introducing the `ui` helper module, and calling it from the frame loop.

# 2) [DONE] Render a button in the control panel surface

**Goal:** Replace the hard-coded text rectangle in the control panel draw path
with a `macroquad::ui` layout that can host interactive widgets.

**Deliverables:**

* In the existing panel draw routine (see `ControlPanelView` usage), instantiate
  a Macroquad `root_ui()` block positioned using the existing panel bounds so
  layout remains consistent with the WIP design.
* Add a labelled button (`ui.button(None, "Toggle Mode")`) inside the panel and
  style it using the minimal shared theme helpers already present (colours,
  padding) to match the current look.
* Ensure the old text rendering (mode label, instructions) migrates into the UI
  block as label widgets, so the panel content remains visible.

**Exit checks:** Running the adapter shows the button rendered within the panel
area, layout matches the previous rectangle, and no regression appears in other
visual elements. Completed by returning panel bounds to the UI helper, pushing a
skinned window that draws the existing gold/mode labels, and placing the
`Toggle Mode` button within the same padded region that previously hosted the
text.

# 3) [DONE] Wire button interaction into simulation flags

**Goal:** Demonstrate integration by flipping the existing control-panel toggle
(`mode_toggle`) when the button is pressed, mirroring the keyboard shortcut.

**Deliverables:**

* When the button reports `true`, set the same field on the outgoing
  `FrameInput` struct that keyboard handling uses today, ensuring the value
  persists only for a single frame just like the existing edge-triggered
  behaviour.
* Update panel state display to reflect the simulation's current mode by
  reading from `FrameOutput` or the cached view that already informs the text
  labels.
* Add a replay harness test under `tests/` or `adapters/macroquad/tests` that
  mocks a sequence of button presses (via injected UI events if available or a
  shim around the control-panel helper) and asserts deterministic toggling.

**Exit checks:** The mode button toggles the same state as the keyboard path,
replay tests demonstrate deterministic input handling, and documentation (this
roadmap plus an addendum in `gameplay.md` if necessary) calls out the new UI
entry point.

Completed by latching UI presses in `ControlPanelInputState` so `FrameInput`
sees them once, returning a boolean from the UI helper, and adding an
integration-style replay test that feeds a scripted sequence into the latch to
prove deterministic toggling.

---

Following this sequence keeps Macroquad UI isolated to the adapter, preserves
existing input contracts, and proves determinism before expanding the control
panel with additional widgets.
