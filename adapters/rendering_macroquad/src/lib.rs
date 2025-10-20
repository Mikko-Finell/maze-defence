#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Macroquad-backed rendering adapter for Maze Defence.
//!
//! Macroquad's optional audio stack depends on native ALSA development
//! libraries, which are unavailable in the containerised CI environment.
//! To keep `cargo test` usable everywhere we depend on macroquad without its
//! default `audio` feature.  Consumers that need sound playback can opt back
//! in by enabling `macroquad/audio` in their own `Cargo.toml` dependency
//! specification.

use anyhow::Result;
use glam::Vec2;
use macroquad::{
    color::BLACK,
    input::{is_key_pressed, mouse_position, KeyCode},
};
use maze_defence_core::PlayMode;
use maze_defence_rendering::{
    BugPresentation, Color, FrameInput, PlacementPreview, Presentation, RenderingBackend, Scene,
    TileGridPresentation,
};
use std::time::Duration;

/// Rendering backend implemented on top of macroquad.
#[derive(Debug, Default)]
pub struct MacroquadBackend;

impl RenderingBackend for MacroquadBackend {
    fn run<F>(self, presentation: Presentation, mut update_scene: F) -> Result<()>
    where
        F: FnMut(Duration, FrameInput, &mut Scene) + 'static,
    {
        let Presentation {
            window_title,
            clear_color,
            scene,
        } = presentation;

        let mut scene = scene;

        let mut config = macroquad::window::Conf::default();
        config.window_title = window_title;
        config.window_width = 960;
        config.window_height = 960;

        macroquad::Window::from_config(config, async move {
            let background = to_macroquad_color(clear_color);

            loop {
                if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::Q) {
                    break;
                }

                macroquad::window::clear_background(background);

                let screen_width = macroquad::window::screen_width();
                let screen_height = macroquad::window::screen_height();

                let dt_seconds = macroquad::time::get_frame_time();
                let frame_dt = Duration::from_secs_f32(dt_seconds.max(0.0));
                let metrics_before = SceneMetrics::from_scene(&scene, screen_width, screen_height);
                let frame_input = gather_frame_input(&scene, &metrics_before);

                update_scene(frame_dt, frame_input, &mut scene);

                let tile_grid = scene.tile_grid;
                let wall = &scene.wall;
                let metrics = SceneMetrics::from_scene(&scene, screen_width, screen_height);

                let grid_color = to_macroquad_color(tile_grid.line_color);
                let subgrid_color = to_macroquad_color(tile_grid.line_color.lighten(0.6));

                draw_subgrid(&metrics, &tile_grid, subgrid_color);
                draw_tile_grid(&metrics, &tile_grid, grid_color);

                if let Some(preview) = active_builder_preview(&scene) {
                    draw_placement_preview(preview, &metrics);
                }

                draw_wall(&metrics, wall, grid_color, subgrid_color);

                let bug_radius = metrics.cell_step * 0.5;
                for BugPresentation { column, row, color } in &scene.bugs {
                    let bug_center_x =
                        metrics.grid_offset_x + (*column as f32 + 0.5) * metrics.cell_step;
                    let bug_center_y =
                        metrics.grid_offset_y + (*row as f32 + 0.5) * metrics.cell_step;
                    let border_thickness = (bug_radius * 0.2).max(1.0);
                    macroquad::shapes::draw_circle(
                        bug_center_x,
                        bug_center_y,
                        bug_radius,
                        to_macroquad_color(*color),
                    );
                    macroquad::shapes::draw_circle_lines(
                        bug_center_x,
                        bug_center_y,
                        bug_radius,
                        border_thickness,
                        BLACK,
                    );
                }

                macroquad::window::next_frame().await;
            }
        });

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct SceneMetrics {
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    grid_offset_x: f32,
    grid_offset_y: f32,
    grid_width_scaled: f32,
    grid_height_scaled: f32,
    bordered_grid_width_scaled: f32,
    bordered_grid_height_scaled: f32,
    tile_step: f32,
    cell_step: f32,
}

impl SceneMetrics {
    fn from_scene(scene: &Scene, screen_width: f32, screen_height: f32) -> Self {
        let tile_grid = scene.tile_grid;
        let world_width = tile_grid.bordered_width();
        let world_height = scene.total_height();
        let scale = if world_width == 0.0 || world_height == 0.0 {
            1.0
        } else {
            (screen_width / world_width).min(screen_height / world_height)
        };

        let scaled_width = world_width * scale;
        let scaled_height = world_height * scale;
        let tile_step = tile_grid.tile_length * scale;
        let cell_step = if tile_grid.cells_per_tile == 0 {
            0.0
        } else {
            tile_step / tile_grid.cells_per_tile as f32
        };
        let offset_x = (screen_width - scaled_width) * 0.5 + cell_step;
        let offset_y = (screen_height - scaled_height) * 0.5;

        let grid_width_scaled = tile_grid.width() * scale;
        let grid_height_scaled = tile_grid.height() * scale;
        let bordered_grid_width_scaled = tile_grid.bordered_width() * scale;
        let bordered_grid_height_scaled = tile_grid.bordered_height() * scale;
        let grid_offset_x =
            offset_x + TileGridPresentation::SIDE_BORDER_CELL_LAYERS as f32 * cell_step;
        let grid_offset_y =
            offset_y + TileGridPresentation::TOP_BORDER_CELL_LAYERS as f32 * cell_step;

        Self {
            scale,
            offset_x,
            offset_y,
            grid_offset_x,
            grid_offset_y,
            grid_width_scaled,
            grid_height_scaled,
            bordered_grid_width_scaled,
            bordered_grid_height_scaled,
            tile_step,
            cell_step,
        }
    }
}

fn gather_frame_input(scene: &Scene, metrics: &SceneMetrics) -> FrameInput {
    let mut input = FrameInput::default();
    input.mode_toggle = is_key_pressed(KeyCode::Space);

    if metrics.scale <= f32::EPSILON {
        return input;
    }

    let tile_grid = scene.tile_grid;
    if tile_grid.columns == 0 || tile_grid.rows == 0 {
        return input;
    }

    let (cursor_x, cursor_y) = mouse_position();

    let world_position = tile_grid.clamp_world_position(Vec2::new(
        (cursor_x - metrics.grid_offset_x) / metrics.scale,
        (cursor_y - metrics.grid_offset_y) / metrics.scale,
    ));

    input.cursor_world_space = Some(world_position);

    let inside = cursor_x >= metrics.grid_offset_x
        && cursor_x < metrics.grid_offset_x + metrics.grid_width_scaled
        && cursor_y >= metrics.grid_offset_y
        && cursor_y < metrics.grid_offset_y + metrics.grid_height_scaled;

    if inside {
        input.cursor_tile_space = tile_grid.snap_world_to_tile(world_position);
    }

    input
}

fn active_builder_preview(scene: &Scene) -> Option<PlacementPreview> {
    if scene.play_mode == PlayMode::Builder {
        scene.placement_preview
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_rendering::TileSpacePosition;

    fn base_scene(play_mode: PlayMode, placement_preview: Option<PlacementPreview>) -> Scene {
        let grid = TileGridPresentation::new(
            4,
            4,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(40, 40, 40),
        )
        .expect("valid grid");
        let wall = maze_defence_rendering::WallPresentation::new(
            8.0,
            Color::from_rgb_u8(64, 64, 64),
            maze_defence_rendering::TargetPresentation::new(Vec::new()),
        );

        Scene::new(grid, wall, Vec::new(), play_mode, placement_preview)
    }

    #[test]
    fn active_builder_preview_suppresses_attack_mode_preview() {
        let preview = PlacementPreview::new(TileSpacePosition::from_indices(1, 1), 1);
        let mut scene = base_scene(PlayMode::Attack, Some(preview));

        assert!(active_builder_preview(&scene).is_none());

        scene.play_mode = PlayMode::Builder;
        scene.placement_preview = Some(preview);

        assert_eq!(active_builder_preview(&scene), Some(preview));
    }
}

fn draw_subgrid(
    metrics: &SceneMetrics,
    tile_grid: &TileGridPresentation,
    subgrid_color: macroquad::color::Color,
) {
    let total_subcolumns = tile_grid.columns * tile_grid.cells_per_tile
        + 2 * TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
    for column in 0..=total_subcolumns {
        let x = metrics.offset_x + column as f32 * metrics.cell_step;
        macroquad::shapes::draw_line(
            x,
            metrics.offset_y,
            x,
            metrics.offset_y + metrics.bordered_grid_height_scaled,
            0.5,
            subgrid_color,
        );
    }

    let total_subrows = tile_grid.rows * tile_grid.cells_per_tile
        + TileGridPresentation::TOP_BORDER_CELL_LAYERS
        + TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS;
    for row in 0..=total_subrows {
        let y = metrics.offset_y + row as f32 * metrics.cell_step;
        macroquad::shapes::draw_line(
            metrics.offset_x,
            y,
            metrics.offset_x + metrics.bordered_grid_width_scaled,
            y,
            0.5,
            subgrid_color,
        );
    }
}

fn draw_tile_grid(
    metrics: &SceneMetrics,
    tile_grid: &TileGridPresentation,
    grid_color: macroquad::color::Color,
) {
    for column in 0..=tile_grid.columns {
        let x = metrics.grid_offset_x + column as f32 * metrics.tile_step;
        macroquad::shapes::draw_line(
            x,
            metrics.grid_offset_y,
            x,
            metrics.grid_offset_y + metrics.grid_height_scaled,
            1.0,
            grid_color,
        );
    }

    for row in 0..=tile_grid.rows {
        let y = metrics.grid_offset_y + row as f32 * metrics.tile_step;
        macroquad::shapes::draw_line(
            metrics.grid_offset_x,
            y,
            metrics.grid_offset_x + metrics.grid_width_scaled,
            y,
            1.0,
            grid_color,
        );
    }
}

fn draw_wall(
    metrics: &SceneMetrics,
    wall: &maze_defence_rendering::WallPresentation,
    grid_color: macroquad::color::Color,
    subgrid_color: macroquad::color::Color,
) {
    let wall_color = to_macroquad_color(wall.color);
    let wall_height = wall.thickness * metrics.scale;
    let wall_y = metrics.offset_y + metrics.bordered_grid_height_scaled;
    let wall_left = metrics.grid_offset_x;
    let wall_right = metrics.grid_offset_x + metrics.grid_width_scaled;

    let target = &wall.target;
    let target_cells = &target.cells;

    if target.is_empty() {
        macroquad::shapes::draw_rectangle(
            wall_left,
            wall_y,
            metrics.grid_width_scaled,
            wall_height,
            wall_color,
        );
    } else {
        let mut target_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column).collect();
        target_columns.sort_unstable();
        target_columns.dedup();

        if let (Some(&first_column), Some(&last_column)) =
            (target_columns.first(), target_columns.last())
        {
            let target_left = metrics.grid_offset_x + first_column as f32 * metrics.cell_step;
            let target_right = metrics.grid_offset_x + (last_column + 1) as f32 * metrics.cell_step;

            if target_left > wall_left {
                macroquad::shapes::draw_rectangle(
                    wall_left,
                    wall_y,
                    target_left - wall_left,
                    wall_height,
                    wall_color,
                );
            }

            if target_right < wall_right {
                macroquad::shapes::draw_rectangle(
                    target_right,
                    wall_y,
                    wall_right - target_right,
                    wall_height,
                    wall_color,
                );
            }

            let walkway_top = wall_y;
            let walkway_bottom = wall_y + wall_height;

            macroquad::shapes::draw_line(
                target_left,
                walkway_top,
                target_left,
                walkway_bottom,
                1.0,
                grid_color,
            );

            macroquad::shapes::draw_line(
                target_right,
                walkway_top,
                target_right,
                walkway_bottom,
                1.0,
                grid_color,
            );

            for &column in target_columns.iter().skip(1) {
                let boundary_x = metrics.grid_offset_x + column as f32 * metrics.cell_step;
                macroquad::shapes::draw_line(
                    boundary_x,
                    walkway_top,
                    boundary_x,
                    walkway_bottom,
                    0.5,
                    subgrid_color,
                );
            }

            macroquad::shapes::draw_line(
                target_left,
                walkway_bottom,
                target_right,
                walkway_bottom,
                1.0,
                grid_color,
            );
        }
    }
}

fn draw_placement_preview(preview: PlacementPreview, metrics: &SceneMetrics) {
    if preview.size_in_tiles == 0 {
        return;
    }

    let fill_color = to_macroquad_color(Color::new(0.32, 0.66, 0.98, 0.35));
    let outline_color = to_macroquad_color(Color::new(0.18, 0.44, 0.75, 0.6));

    let preview_x = metrics.grid_offset_x + preview.origin.column_in_tiles() * metrics.tile_step;
    let preview_y = metrics.grid_offset_y + preview.origin.row_in_tiles() * metrics.tile_step;
    let size = preview.size_in_tiles as f32 * metrics.tile_step;

    macroquad::shapes::draw_rectangle(preview_x, preview_y, size, size, fill_color);
    macroquad::shapes::draw_rectangle_lines(preview_x, preview_y, size, size, 1.0, outline_color);
}

fn to_macroquad_color(color: maze_defence_rendering::Color) -> macroquad::color::Color {
    macroquad::color::Color::new(color.red, color.green, color.blue, color.alpha)
}
