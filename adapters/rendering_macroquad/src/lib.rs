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
    input::{is_key_pressed, is_mouse_button_pressed, mouse_position, KeyCode, MouseButton},
};
use maze_defence_core::PlayMode;
use maze_defence_rendering::{
    BugPresentation, Color, FrameInput, Presentation, RenderingBackend, Scene, SceneTower,
    TileGridPresentation, TowerPreview,
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

                draw_towers(&scene.towers, &metrics);

                if let Some(preview) = active_builder_preview(&scene) {
                    draw_tower_preview(preview, &metrics);
                }

                draw_wall(&metrics, wall, grid_color, subgrid_color);

                let bug_radius = metrics.cell_step * 0.5;
                for BugPresentation { column, row, color } in &scene.bugs {
                    let bug_center_x =
                        metrics.offset_x + (*column as f32 + 0.5) * metrics.cell_step;
                    let bug_center_y = metrics.offset_y + (*row as f32 + 0.5) * metrics.cell_step;
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
        let offset_x = (screen_width - scaled_width) * 0.5;
        let offset_y = (screen_height - scaled_height) * 0.5;

        let grid_width_scaled = tile_grid.width() * scale;
        let grid_height_scaled = tile_grid.height() * scale;
        let bordered_grid_width_scaled = tile_grid.bordered_width() * scale;
        let bordered_grid_height_scaled = tile_grid.bordered_height() * scale;
        let tile_step = tile_grid.tile_length * scale;
        let cell_step = if tile_grid.cells_per_tile == 0 {
            0.0
        } else {
            tile_step / tile_grid.cells_per_tile as f32
        };
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
    let (cursor_x, cursor_y) = mouse_position();
    let mode_toggle = is_key_pressed(KeyCode::Space);
    let confirm_click = is_mouse_button_pressed(MouseButton::Left);
    let remove_click = is_mouse_button_pressed(MouseButton::Right);
    let delete_pressed = is_key_pressed(KeyCode::Delete);

    gather_frame_input_from_observations(
        scene,
        metrics,
        Vec2::new(cursor_x, cursor_y),
        mode_toggle,
        confirm_click,
        remove_click,
        delete_pressed,
    )
}

fn gather_frame_input_from_observations(
    scene: &Scene,
    metrics: &SceneMetrics,
    cursor_position: Vec2,
    mode_toggle: bool,
    confirm_click: bool,
    remove_click: bool,
    delete_pressed: bool,
) -> FrameInput {
    let mut input = FrameInput::default();
    input.mode_toggle = mode_toggle;

    let preview_footprint = active_preview_footprint_tiles(scene);
    input.preview_footprint_in_tiles = preview_footprint;

    if metrics.scale <= f32::EPSILON {
        return input;
    }

    let tile_grid = scene.tile_grid;
    if tile_grid.columns == 0 || tile_grid.rows == 0 {
        return input;
    }

    let cursor_x = cursor_position.x;
    let cursor_y = cursor_position.y;

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
        input.cursor_tile_space = tile_grid.snap_world_to_tile(world_position, preview_footprint);
        input.confirm_action = confirm_click;
    }

    input.remove_action = remove_click || delete_pressed;

    input
}

fn active_preview_footprint_tiles(scene: &Scene) -> Vec2 {
    let default = Vec2::splat(1.0);
    let cells_per_tile = scene.tile_grid.cells_per_tile;
    if cells_per_tile == 0 {
        return default;
    }

    let Some(preview) = active_builder_preview(scene) else {
        return default;
    };

    let size = preview.region.size();
    if size.width() == 0 || size.height() == 0 {
        return default;
    }

    let tiles = cells_per_tile as f32;
    Vec2::new(size.width() as f32 / tiles, size.height() as f32 / tiles)
}

fn active_builder_preview(scene: &Scene) -> Option<TowerPreview> {
    if scene.play_mode == PlayMode::Builder {
        scene.tower_preview
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::{CellCoord, CellRect, CellRectSize, TowerKind};

    fn base_scene(play_mode: PlayMode, placement_preview: Option<TowerPreview>) -> Scene {
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

        Scene::new(
            grid,
            wall,
            Vec::new(),
            Vec::new(),
            play_mode,
            placement_preview,
            None,
        )
    }

    #[test]
    fn active_builder_preview_suppresses_attack_mode_preview() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(2, 2), CellRectSize::new(2, 2));
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        let mut scene = base_scene(PlayMode::Attack, Some(preview));

        assert!(active_builder_preview(&scene).is_none());

        scene.play_mode = PlayMode::Builder;
        scene.tower_preview = Some(preview);

        assert_eq!(active_builder_preview(&scene), Some(preview));
    }

    #[test]
    fn target_columns_are_normalized_by_side_margin() {
        let margin = TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
        let input = vec![margin, margin + 1, margin + 5];
        let normalized = normalize_target_columns(&input);

        assert_eq!(normalized, vec![0, 1, 5]);
    }

    #[test]
    fn active_preview_footprint_tiles_reflects_preview_size() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(0, 0), CellRectSize::new(2, 4));
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        let scene = base_scene(PlayMode::Builder, Some(preview));

        assert_eq!(active_preview_footprint_tiles(&scene), Vec2::new(0.5, 1.0));
    }

    #[test]
    fn confirm_action_only_set_when_cursor_inside_grid() {
        let scene = base_scene(PlayMode::Builder, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        let inside_cursor = Vec2::new(
            metrics.grid_offset_x + metrics.cell_step * 0.5,
            metrics.grid_offset_y + metrics.cell_step * 0.5,
        );
        let inside_input = gather_frame_input_from_observations(
            &scene,
            &metrics,
            inside_cursor,
            false,
            true,
            false,
            false,
        );
        assert!(
            inside_input.confirm_action,
            "left click inside the grid should be treated as a confirm action",
        );

        let outside_cursor = Vec2::new(metrics.grid_offset_x - 10.0, metrics.grid_offset_y - 10.0);
        let outside_input = gather_frame_input_from_observations(
            &scene,
            &metrics,
            outside_cursor,
            false,
            true,
            false,
            false,
        );
        assert!(
            outside_input.cursor_tile_space.is_none(),
            "cursor outside the grid must not snap to tile space",
        );
        assert!(
            !outside_input.confirm_action,
            "clicking outside the grid must not emit confirm actions",
        );
    }

    #[test]
    fn gather_frame_input_snaps_using_preview_footprint() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(0, 0), CellRectSize::new(2, 4));
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        let scene = base_scene(PlayMode::Builder, Some(preview));
        let metrics = SceneMetrics::from_scene(&scene, 640.0, 640.0);

        let world = Vec2::new(
            scene.tile_grid.width() - 1.0,
            scene.tile_grid.height() - 1.0,
        );
        let cursor = Vec2::new(
            metrics.grid_offset_x + world.x * metrics.scale,
            metrics.grid_offset_y + world.y * metrics.scale,
        );

        let input = gather_frame_input_from_observations(
            &scene, &metrics, cursor, false, false, false, false,
        );

        assert_eq!(input.preview_footprint_in_tiles, Vec2::new(0.5, 1.0));
        let snapped = input
            .cursor_tile_space
            .expect("cursor inside grid should produce tile position");
        let origin_column_tiles = snapped.column_half_steps() as f32 * 0.5;
        let origin_row_tiles = snapped.row_half_steps() as f32 * 0.5;

        assert!(origin_column_tiles + 0.5 <= scene.tile_grid.columns as f32);
        assert!(origin_row_tiles + 1.0 <= scene.tile_grid.rows as f32);
    }

    #[test]
    fn preview_world_rect_matches_preview_footprint() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(1, 2), CellRectSize::new(2, 4));
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        let scene = base_scene(PlayMode::Builder, Some(preview));
        let metrics = SceneMetrics::from_scene(&scene, 800.0, 800.0);

        let (x, y, width, height) =
            preview_world_rect(&preview, &metrics).expect("preview should yield rectangle");
        let cell_step = metrics.cell_step;
        let origin = preview.region.origin();

        assert_eq!(width, preview.region.size().width() as f32 * cell_step);
        assert_eq!(height, preview.region.size().height() as f32 * cell_step);
        assert_eq!(x, metrics.offset_x + origin.column() as f32 * cell_step);
        assert_eq!(y, metrics.offset_y + origin.row() as f32 * cell_step);
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

        let normalized_columns = normalize_target_columns(&target_columns);

        if let (Some(&first_column), Some(&last_column)) =
            (normalized_columns.first(), normalized_columns.last())
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

            for &column in normalized_columns.iter().skip(1) {
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

fn normalize_target_columns(columns: &[u32]) -> Vec<u32> {
    let margin = TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
    columns
        .iter()
        .map(|&column| column.saturating_sub(margin))
        .collect()
}

fn draw_towers(towers: &[SceneTower], metrics: &SceneMetrics) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let base_color = Color::from_rgb_u8(78, 52, 128);
    let outline_color = base_color.lighten(0.35);
    let fill = to_macroquad_color(Color::new(
        base_color.red,
        base_color.green,
        base_color.blue,
        1.0,
    ));
    let outline = to_macroquad_color(Color::new(
        outline_color.red,
        outline_color.green,
        outline_color.blue,
        1.0,
    ));
    let outline_thickness = (metrics.cell_step * 0.12).max(1.0);

    for SceneTower { region, .. } in towers {
        let size = region.size();
        if size.width() == 0 || size.height() == 0 {
            continue;
        }

        let origin = region.origin();
        let x = metrics.offset_x + origin.column() as f32 * metrics.cell_step;
        let y = metrics.offset_y + origin.row() as f32 * metrics.cell_step;
        let width = size.width() as f32 * metrics.cell_step;
        let height = size.height() as f32 * metrics.cell_step;

        macroquad::shapes::draw_rectangle(x, y, width, height, fill);
        macroquad::shapes::draw_rectangle_lines(x, y, width, height, outline_thickness, outline);
    }
}

fn draw_tower_preview(preview: TowerPreview, metrics: &SceneMetrics) {
    let (fill_color, outline_color) = if preview.placeable {
        let base = Color::from_rgb_u8(78, 52, 128);
        let outline = base.lighten(0.4);
        (
            Color::new(base.red, base.green, base.blue, 0.35),
            Color::new(outline.red, outline.green, outline.blue, 0.7),
        )
    } else {
        let base = Color::from_rgb_u8(176, 52, 68);
        let outline = base.lighten(0.3);
        (
            Color::new(base.red, base.green, base.blue, 0.45),
            Color::new(outline.red, outline.green, outline.blue, 0.8),
        )
    };

    let Some((x, y, width, height)) = preview_world_rect(&preview, metrics) else {
        return;
    };

    macroquad::shapes::draw_rectangle(x, y, width, height, to_macroquad_color(fill_color));
    macroquad::shapes::draw_rectangle_lines(
        x,
        y,
        width,
        height,
        (metrics.cell_step * 0.1).max(1.0),
        to_macroquad_color(outline_color),
    );
}

fn preview_world_rect(
    preview: &TowerPreview,
    metrics: &SceneMetrics,
) -> Option<(f32, f32, f32, f32)> {
    if metrics.cell_step <= f32::EPSILON {
        return None;
    }

    let size = preview.region.size();
    if size.width() == 0 || size.height() == 0 {
        return None;
    }

    let origin = preview.region.origin();
    let x = metrics.offset_x + origin.column() as f32 * metrics.cell_step;
    let y = metrics.offset_y + origin.row() as f32 * metrics.cell_step;
    let width = size.width() as f32 * metrics.cell_step;
    let height = size.height() as f32 * metrics.cell_step;

    Some((x, y, width, height))
}

fn to_macroquad_color(color: maze_defence_rendering::Color) -> macroquad::color::Color {
    macroquad::color::Color::new(color.red, color.green, color.blue, color.alpha)
}
