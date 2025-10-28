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
use maze_defence_rendering::{
    AnalyticsPresentation, DifficultyPresentation, DifficultySelectionPresentation,
    GoldPresentation,
};

/// Snapshot of the control panel's UI layout and data for the current frame.
#[derive(Clone, Debug)]
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
    /// Presentable difficulty level exposed by the simulation.
    pub difficulty: Option<DifficultyPresentation>,
    /// Presentation data for the difficulty selection buttons.
    pub difficulty_selection: Option<DifficultySelectionPresentation>,
    /// Most recent analytics snapshot published by the simulation, if any.
    pub analytics: Option<AnalyticsPresentation>,
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

    let selection = context.difficulty_selection;

    let max_label_width = (context.size.x - 32.0).max(0.0);

    let mut result = ControlPanelUiResult::default();
    let _ = ui.window(hash!("control_panel"), context.origin, context.size, |ui| {
        let difficulty_text = match context.difficulty {
            Some(level) => format!("Difficulty: {}", level.level()),
            None => "Difficulty: –".to_string(),
        };
        ui.label(None, difficulty_text.as_str());

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
        if context.play_mode == PlayMode::Builder {
            match context.analytics {
                Some(analytics) => {
                    let report = analytics.report();
                    ui.label(None, "Analytics:");
                    label_wrapped(
                        ui,
                        format!(
                            "Path coverage: {}",
                            format_basis_points(report.tower_coverage_mean_bps())
                        )
                        .as_str(),
                        max_label_width,
                    );
                    label_wrapped(
                        ui,
                        format!(
                            "Firing completion: {}",
                            format_basis_points(report.firing_complete_percent_bps())
                        )
                        .as_str(),
                        max_label_width,
                    );
                    label_wrapped(
                        ui,
                        format!(
                            "Shortest path: {} cells",
                            report.shortest_path_length_cells()
                        )
                        .as_str(),
                        max_label_width,
                    );
                    label_wrapped(
                        ui,
                        format!("Tower count: {}", report.tower_count()).as_str(),
                        max_label_width,
                    );
                    label_wrapped(
                        ui,
                        format!("Total DPS: {}", report.total_tower_dps()).as_str(),
                        max_label_width,
                    );
                }
                None => {
                    label_wrapped(ui, "Analytics: waiting for first report…", max_label_width);
                }
            }
        }
        label_wrapped(ui, "Select the next wave difficulty.", max_label_width);

        let mut normal_label = "Normal".to_string();
        let mut hard_label = "Hard".to_string();
        let mut normal_preview = None;
        let mut hard_preview = None;

        if let Some(selection) = selection {
            let normal = selection.normal();
            let hard = selection.hard();

            if normal.selected() {
                normal_label.push_str(" ★");
            }
            if hard.selected() {
                hard_label.push_str(" ★");
            }

            normal_preview = Some(format!(
                "Normal rewards: x{} gold (Difficulty {})",
                normal.reward_multiplier(),
                normal.effective_level()
            ));
            hard_preview = Some(format!(
                "Hard rewards: x{} gold (Difficulty {})",
                hard.reward_multiplier(),
                hard.effective_level()
            ));
        }

        if ui.button(None, normal_label.as_str()) {
            result.start_wave = Some(WaveDifficulty::Normal);
        }
        if ui.button(None, hard_label.as_str()) {
            result.start_wave = Some(WaveDifficulty::Hard);
        }

        if let Some(text) = normal_preview {
            label_wrapped(ui, text.as_str(), max_label_width);
        }
        if let Some(text) = hard_preview {
            label_wrapped(ui, text.as_str(), max_label_width);
            label_wrapped(
                ui,
                "Hard victory grants +1 permanent difficulty.",
                max_label_width,
            );
        }

        label_wrapped(ui, "Use the button below to switch modes.", max_label_width);

        if ui.button(None, "Toggle Mode") {
            result.mode_toggle = true;
        }
    });

    ui.pop_skin();
    result
}

fn label_wrapped(ui: &mut Ui, text: &str, max_width: f32) {
    for line in wrap_text(ui, text, max_width) {
        ui.label(None, line.as_str());
    }
}

fn format_basis_points(value: u32) -> String {
    let whole = value / 100;
    let fractional = value % 100;
    format!("{whole}.{fractional:02}%")
}

fn wrap_text(ui: &mut Ui, text: &str, max_width: f32) -> Vec<String> {
    let effective_width = max_width.max(0.0);
    if effective_width <= f32::EPSILON {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        let candidate = if current_line.is_empty() {
            word.to_string()
        } else {
            format!("{} {}", current_line, word)
        };

        let candidate_width = ui.calc_size(candidate.as_str()).x;
        if candidate_width <= effective_width || current_line.is_empty() {
            current_line = candidate;
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}
