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
    BugPresentation, Color, FrameInput, FrameSimulationBreakdown, Presentation, RenderingBackend,
    Scene, SceneTower, TileGridPresentation, TowerPreview, TowerTargetLine,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Rendering backend implemented on top of macroquad.
#[derive(Debug, Default)]
pub struct MacroquadBackend {
    swap_interval: Option<i32>,
}

impl MacroquadBackend {
    /// Returns a backend that requests the platform's default swap interval.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the backend to request a specific swap interval from the platform.
    #[must_use]
    pub fn with_swap_interval(mut self, swap_interval: Option<i32>) -> Self {
        self.swap_interval = swap_interval;
        self
    }

    /// Configures the backend to either synchronise presentation with the display refresh rate
    /// or render as fast as possible.
    #[must_use]
    pub fn with_vsync(self, enabled: bool) -> Self {
        let swap_interval = if enabled { Some(1) } else { Some(0) };
        self.with_swap_interval(swap_interval)
    }
}

/// Tracks the average frames-per-second produced by the render loop.
#[derive(Clone, Copy, Debug, Default)]
struct FrameBreakdown {
    frame: Duration,
    simulation: Duration,
    pathfinding: Duration,
    scene_population: Duration,
    render: Duration,
}

#[derive(Debug, Default)]
struct FpsCounter {
    elapsed: Duration,
    frames: u32,
    frame_times: VecDeque<Duration>,
    window_duration: Duration,
    simulation_accum: Duration,
    pathfinding_accum: Duration,
    scene_population_accum: Duration,
    render_accum: Duration,
}

#[derive(Clone, Copy, Debug)]
struct FpsMetrics {
    per_second: f32,
    trailing_ten_seconds: f32,
    avg_simulation: Duration,
    avg_pathfinding: Duration,
    avg_scene_population: Duration,
    avg_render: Duration,
}

impl FpsCounter {
    /// Records a rendered frame and returns the per-second and trailing ten-second averages once
    /// one second has elapsed.
    fn record_frame(&mut self, breakdown: FrameBreakdown) -> Option<FpsMetrics> {
        self.elapsed += breakdown.frame;
        self.frames = self.frames.saturating_add(1);

        self.simulation_accum += breakdown.simulation;
        self.pathfinding_accum += breakdown.pathfinding;
        self.scene_population_accum += breakdown.scene_population;
        self.render_accum += breakdown.render;

        self.frame_times.push_back(breakdown.frame);
        self.window_duration += breakdown.frame;

        let trailing_window = Duration::from_secs(10);
        while self.window_duration > trailing_window {
            if let Some(removed) = self.frame_times.pop_front() {
                self.window_duration = self.window_duration.saturating_sub(removed);
            } else {
                break;
            }
        }

        if self.elapsed < Duration::from_secs(1) {
            return None;
        }

        let seconds = self.elapsed.as_secs_f32();
        if seconds <= f32::EPSILON {
            self.elapsed = Duration::ZERO;
            self.frames = 0;
            self.simulation_accum = Duration::ZERO;
            self.pathfinding_accum = Duration::ZERO;
            self.scene_population_accum = Duration::ZERO;
            self.render_accum = Duration::ZERO;
            return None;
        }

        let per_second = self.frames as f32 / seconds;
        let window_seconds = self.window_duration.as_secs_f32();
        let trailing_ten_seconds = if window_seconds <= f32::EPSILON {
            per_second
        } else {
            self.frame_times.len() as f32 / window_seconds
        };
        let frames = self.frames;
        let avg_simulation = if frames == 0 {
            Duration::ZERO
        } else {
            self.simulation_accum / frames
        };
        let avg_pathfinding = if frames == 0 {
            Duration::ZERO
        } else {
            self.pathfinding_accum / frames
        };
        let avg_scene_population = if frames == 0 {
            Duration::ZERO
        } else {
            self.scene_population_accum / frames
        };
        let avg_render = if frames == 0 {
            Duration::ZERO
        } else {
            self.render_accum / frames
        };
        self.elapsed = Duration::ZERO;
        self.frames = 0;
        self.simulation_accum = Duration::ZERO;
        self.pathfinding_accum = Duration::ZERO;
        self.scene_population_accum = Duration::ZERO;
        self.render_accum = Duration::ZERO;
        Some(FpsMetrics {
            per_second,
            trailing_ten_seconds,
            avg_simulation,
            avg_pathfinding,
            avg_scene_population,
            avg_render,
        })
    }
}

impl RenderingBackend for MacroquadBackend {
    fn run<F>(self, presentation: Presentation, mut update_scene: F) -> Result<()>
    where
        F: FnMut(Duration, FrameInput, &mut Scene) -> FrameSimulationBreakdown + 'static,
    {
        let Presentation {
            window_title,
            clear_color,
            scene,
        } = presentation;

        let mut scene = scene;

        let mut config = macroquad::window::Conf {
            window_title,
            window_width: 960,
            window_height: 960,
            ..macroquad::window::Conf::default()
        };
        if let Some(swap_interval) = self.swap_interval {
            config.platform.swap_interval = Some(swap_interval);
        }

        macroquad::Window::from_config(config, async move {
            let background = to_macroquad_color(clear_color);
            let mut fps_counter = FpsCounter::default();

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

                let simulation_breakdown = update_scene(frame_dt, frame_input, &mut scene);

                let tile_grid = scene.tile_grid;
                let wall = &scene.wall;
                let metrics = SceneMetrics::from_scene(&scene, screen_width, screen_height);

                let grid_color = to_macroquad_color(tile_grid.line_color);
                let subgrid_color = to_macroquad_color(tile_grid.line_color.lighten(0.6));

                let render_start = Instant::now();
                draw_subgrid(&metrics, &tile_grid, subgrid_color);
                draw_maze_walls(&metrics, &scene.walls, wall.color);
                draw_tile_grid(&metrics, &tile_grid, grid_color);

                draw_towers(&scene.towers, &metrics);

                if let Some(preview) = active_builder_preview(&scene) {
                    draw_tower_preview(preview, &metrics);
                }

                draw_wall(&metrics, wall, grid_color, subgrid_color);

                draw_tower_targets(&scene.tower_targets, &metrics);

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

                let render_duration = render_start.elapsed();

                let frame_breakdown = FrameBreakdown {
                    frame: frame_dt,
                    simulation: simulation_breakdown.simulation,
                    pathfinding: simulation_breakdown.pathfinding,
                    scene_population: simulation_breakdown.scene_population,
                    render: render_duration,
                };

                if let Some(FpsMetrics {
                    per_second,
                    trailing_ten_seconds,
                    avg_simulation,
                    avg_pathfinding,
                    avg_scene_population,
                    avg_render,
                }) = fps_counter.record_frame(frame_breakdown)
                {
                    println!(
                        "FPS: {:.2} (10s avg: {:.2}) | sim: {:>6.2}ms (path: {:>6.2}ms) scene: {:>6.2}ms render: {:>6.2}ms",
                        per_second,
                        trailing_ten_seconds,
                        avg_simulation.as_secs_f64() * 1_000.0,
                        avg_pathfinding.as_secs_f64() * 1_000.0,
                        avg_scene_population.as_secs_f64() * 1_000.0,
                        avg_render.as_secs_f64() * 1_000.0,
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
    let mut input = FrameInput {
        mode_toggle,
        ..FrameInput::default()
    };

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
        let footprint = scene
            .active_tower_footprint_tiles
            .unwrap_or_else(|| Vec2::splat(1.0));
        input.cursor_tile_space = tile_grid.snap_world_to_tile(world_position, footprint);
        input.confirm_action = confirm_click;
    }

    input.remove_action = remove_click || delete_pressed;

    input
}

fn active_builder_preview(scene: &Scene) -> Option<TowerPreview> {
    if scene.play_mode == PlayMode::Builder {
        scene.tower_preview
    } else {
        None
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

fn draw_maze_walls(
    metrics: &SceneMetrics,
    walls: &[maze_defence_rendering::WallCellPresentation],
    wall_color: maze_defence_rendering::Color,
) {
    if walls.is_empty() || metrics.cell_step <= f32::EPSILON {
        return;
    }

    let fill = to_macroquad_color(wall_color);

    for wall in walls {
        let left = metrics.grid_offset_x + wall.column as f32 * metrics.cell_step;
        let top = metrics.grid_offset_y + wall.row as f32 * metrics.cell_step;
        macroquad::shapes::draw_rectangle(left, top, metrics.cell_step, metrics.cell_step, fill);
    }
}

fn normalize_target_columns(columns: &[u32]) -> Vec<u32> {
    let margin = TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
    columns
        .iter()
        .map(|&column| column.saturating_sub(margin))
        .collect()
}

fn draw_tower_targets(tower_targets: &[TowerTargetLine], metrics: &SceneMetrics) {
    for (start, end) in tower_target_segments(tower_targets, metrics) {
        macroquad::shapes::draw_line(start.x, start.y, end.x, end.y, 1.0, BLACK);
    }
}

fn tower_target_segments(
    tower_targets: &[TowerTargetLine],
    metrics: &SceneMetrics,
) -> Vec<(Vec2, Vec2)> {
    if tower_targets.is_empty() || metrics.cell_step <= f32::EPSILON {
        return Vec::new();
    }

    tower_targets
        .iter()
        .map(|line| {
            let start = Vec2::new(
                metrics.offset_x + line.from.x * metrics.cell_step,
                metrics.offset_y + line.from.y * metrics.cell_step,
            );
            let end = Vec2::new(
                metrics.offset_x + line.to.x * metrics.cell_step,
                metrics.offset_y + line.to.y * metrics.cell_step,
            );
            (start, end)
        })
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
    let Some((x, y, width, height)) = preview_rectangle(preview, metrics) else {
        return;
    };

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

fn preview_rectangle(
    preview: TowerPreview,
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::{BugId, CellCoord, CellRect, CellRectSize, TowerId, TowerKind};
    use std::time::Duration;

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
            Vec::new(),
            Vec::new(),
            play_mode,
            placement_preview,
            None,
            None,
        )
    }

    #[test]
    fn active_builder_preview_suppresses_attack_mode_preview() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(2, 2), CellRectSize::new(4, 4));
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
    fn confirm_action_only_set_when_cursor_inside_grid() {
        let mut scene = base_scene(PlayMode::Builder, None);
        scene.active_tower_footprint_tiles = Some(Vec2::new(1.5, 0.5));
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
    fn cursor_tile_space_respects_active_tower_footprint() {
        let mut scene = base_scene(PlayMode::Builder, None);
        let footprint = Vec2::new(1.5, 0.5);
        scene.active_tower_footprint_tiles = Some(footprint);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        let cursor = Vec2::new(
            metrics.grid_offset_x + metrics.grid_width_scaled - 1.0,
            metrics.grid_offset_y + metrics.grid_height_scaled - 1.0,
        );
        let input = gather_frame_input_from_observations(
            &scene, &metrics, cursor, false, false, false, false,
        );

        let tile = input
            .cursor_tile_space
            .expect("cursor inside grid should snap to tile space");
        let origin_column_tiles = tile.column_in_tiles();
        let origin_row_tiles = tile.row_in_tiles();

        assert!(origin_column_tiles + footprint.x <= scene.tile_grid.columns as f32 + 1e-5);
        assert!(origin_row_tiles + footprint.y <= scene.tile_grid.rows as f32 + 1e-5);
    }

    #[test]
    fn tower_target_segments_empty_when_no_targets() {
        let mut scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        assert!(tower_target_segments(&scene.tower_targets, &metrics).is_empty());

        scene.tower_targets.push(TowerTargetLine::new(
            TowerId::new(1),
            BugId::new(2),
            Vec2::new(3.0, 4.0),
            Vec2::new(5.5, 6.5),
        ));
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);
        let segments = tower_target_segments(&scene.tower_targets, &metrics);

        assert_eq!(segments.len(), 1);
        let (start, end) = segments[0];
        let expected_start = Vec2::new(
            metrics.offset_x + 3.0 * metrics.cell_step,
            metrics.offset_y + 4.0 * metrics.cell_step,
        );
        let expected_end = Vec2::new(
            metrics.offset_x + 5.5 * metrics.cell_step,
            metrics.offset_y + 6.5 * metrics.cell_step,
        );
        assert_eq!(start, expected_start);
        assert_eq!(end, expected_end);
    }

    #[test]
    fn preview_rectangle_matches_footprint_in_world_space() {
        let mut scene = base_scene(PlayMode::Builder, None);
        let origin = CellCoord::new(4, 2);
        let size = CellRectSize::new(6, 4);
        let preview_region = CellRect::from_origin_and_size(origin, size);
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        scene.tower_preview = Some(preview);
        let metrics = SceneMetrics::from_scene(&scene, 640.0, 640.0);

        let (_, _, width, height) =
            preview_rectangle(preview, &metrics).expect("preview geometry should be available");
        let footprint_tiles = Vec2::new(
            size.width() as f32 / scene.tile_grid.cells_per_tile as f32,
            size.height() as f32 / scene.tile_grid.cells_per_tile as f32,
        );
        let expected_width = footprint_tiles.x * scene.tile_grid.tile_length * metrics.scale;
        let expected_height = footprint_tiles.y * scene.tile_grid.tile_length * metrics.scale;

        assert!((width - expected_width).abs() < 1e-5);
        assert!((height - expected_height).abs() < 1e-5);
    }

    #[test]
    fn fps_counter_reports_average_frames_per_second() {
        let mut counter = FpsCounter::default();
        let frame = |millis| FrameBreakdown {
            frame: Duration::from_millis(millis),
            ..FrameBreakdown::default()
        };
        assert!(counter.record_frame(frame(250)).is_none());
        assert!(counter.record_frame(frame(250)).is_none());
        assert!(counter.record_frame(frame(250)).is_none());

        let metrics = counter
            .record_frame(frame(250))
            .expect("should report FPS after one second of samples");
        assert!((metrics.per_second - 4.0).abs() <= 1e-3);
        assert!((metrics.trailing_ten_seconds - 4.0).abs() <= 1e-3);
        assert!(counter.record_frame(frame(250)).is_none());
    }

    #[test]
    fn fps_counter_tracks_trailing_ten_second_average() {
        let mut counter = FpsCounter::default();
        let frame = |millis| FrameBreakdown {
            frame: Duration::from_millis(millis),
            ..FrameBreakdown::default()
        };

        for _ in 0..10 {
            for sample in 0..5 {
                let metrics = counter.record_frame(frame(200));
                if sample == 4 {
                    let metrics = metrics.expect("should report every second");
                    assert!((metrics.per_second - 5.0).abs() <= 1e-3);
                    assert!((metrics.trailing_ten_seconds - 5.0).abs() <= 1e-3);
                } else {
                    assert!(metrics.is_none());
                }
            }
        }

        for sample in 0..10 {
            let metrics = counter.record_frame(frame(100));
            if sample == 9 {
                let metrics = metrics.expect("should report every second");
                assert!((metrics.per_second - 10.0).abs() <= 1e-3);
                assert!((metrics.trailing_ten_seconds - 5.5).abs() <= 1e-3);
            } else {
                assert!(metrics.is_none());
            }
        }
    }
}
