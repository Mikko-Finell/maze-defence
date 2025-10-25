//! Immediate-mode UI helpers for the Macroquad rendering backend.
//!
//! This module hosts all uses of `macroquad::ui` so the rest of the adapter can
//! remain agnostic of Macroquad's UI types. Future control-panel widgets should
//! be added here via `draw_control_panel_ui`.

use macroquad::{
    color::{Color, WHITE},
    math::{RectOffset, Vec2},
    ui::{hash, Ui},
};
use maze_defence_core::{PlayMode, WaveDifficulty};
use maze_defence_rendering::{GoldPresentation, TierPresentation};

/// Snapshot of the control panel's UI layout and data for the current frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlPanelUiContext {
    /// Top-left corner of the panel in screen coordinates.
    pub origin: Vec2,
    /// Panel dimensions in screen space.
    pub size: Vec2,
    /// Background colour applied to the window skin so the UI matches the
    /// adapter's solid rectangle.
    pub background: Color,
    /// Current play mode, displayed as a status label.
    pub play_mode: PlayMode,
    /// Presentable gold amount exposed by the simulation.
    pub gold: Option<GoldPresentation>,
    /// Presentable difficulty tier exposed by the simulation.
    pub tier: Option<TierPresentation>,
}

/// Captures the UI interactions emitted while drawing the control panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ControlPanelUiResult {
    /// Whether the play-mode toggle button was pressed this frame.
    pub mode_toggle: bool,
    /// Difficulty selected for the next wave launch, if any.
    pub start_wave: Option<WaveDifficulty>,
}

/// Renders the control panel's interactive elements for the current frame and
/// returns the resulting interactions.
pub(crate) fn draw_control_panel_ui(
    ui: &mut Ui,
    context: ControlPanelUiContext,
) -> ControlPanelUiResult {
    let mut skin = ui.default_skin();
    skin.margin = 0.0;

    let window_style = ui
        .style_builder()
        .color(context.background)
        .color_hovered(context.background)
        .color_clicked(context.background)
        .color_selected(context.background)
        .color_selected_hovered(context.background)
        .color_inactive(context.background)
        .text_color(WHITE)
        .text_color_hovered(WHITE)
        .text_color_clicked(WHITE)
        .margin(RectOffset::new(16.0, 16.0, 16.0, 16.0))
        .build();
    skin.window_style = window_style;

    let label_style = ui
        .style_builder()
        .text_color(WHITE)
        .text_color_hovered(WHITE)
        .text_color_clicked(WHITE)
        .margin(RectOffset::new(0.0, 0.0, 4.0, 4.0))
        .build();
    skin.label_style = label_style;

    let button_style = ui
        .style_builder()
        .text_color(WHITE)
        .text_color_hovered(WHITE)
        .text_color_clicked(WHITE)
        .color(Color::from_rgba(70, 70, 70, 255))
        .color_hovered(Color::from_rgba(96, 96, 96, 255))
        .color_clicked(Color::from_rgba(56, 56, 56, 255))
        .color_selected(Color::from_rgba(70, 70, 70, 255))
        .color_selected_hovered(Color::from_rgba(96, 96, 96, 255))
        .color_inactive(Color::from_rgba(56, 56, 56, 200))
        .margin(RectOffset::new(0.0, 0.0, 8.0, 8.0))
        .build();
    skin.button_style = button_style;

    ui.push_skin(&skin);

    let mut result = ControlPanelUiResult::default();
    let _ = ui.window(hash!("control_panel"), context.origin, context.size, |ui| {
        let tier_text = match context.tier {
            Some(tier) => format!("Tier: {}", tier.tier()),
            None => "Tier: –".to_string(),
        };
        ui.label(None, tier_text.as_str());

        let gold_text = match context.gold {
            Some(gold) => format!("Gold: {}", gold.amount().get()),
            None => "Gold: –".to_string(),
        };
        ui.label(None, gold_text.as_str());

        let mode_label = match context.play_mode {
            PlayMode::Attack => "Mode: Attack",
            PlayMode::Builder => "Mode: Builder",
        };
        ui.label(None, mode_label);
        ui.label(None, "Select the next wave difficulty.");

        if ui.button(None, "Normal") {
            result.start_wave = Some(WaveDifficulty::Normal);
        }
        ui.same_line(12.0);
        if ui.button(None, "Hard") {
            result.start_wave = Some(WaveDifficulty::Hard);
        }

        ui.label(None, "Use the button below to switch modes.");

        if ui.button(None, "Toggle Mode") {
            result.mode_toggle = true;
        }
    });

    ui.pop_skin();
    result
}
