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
use macroquad::input::{is_key_pressed, KeyCode};
use maze_defence_rendering::{
    BugPresentation, Presentation, RenderingBackend, Scene, TileGridPresentation,
};
use std::time::Duration;

/// Rendering backend implemented on top of macroquad.
#[derive(Debug, Default)]
pub struct MacroquadBackend;

impl RenderingBackend for MacroquadBackend {
    fn run<F>(self, presentation: Presentation, mut update_scene: F) -> Result<()>
    where
        F: FnMut(Duration, &mut Scene) + 'static,
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
                update_scene(frame_dt, &mut scene);

                let tile_grid = scene.tile_grid;
                let wall = &scene.wall;

                let world_width = tile_grid.bordered_width();
                let world_height = scene.total_height();
                let scale = (screen_width / world_width).min(screen_height / world_height);

                let scaled_width = world_width * scale;
                let scaled_height = world_height * scale;
                let offset_x = (screen_width - scaled_width) * 0.5;
                let offset_y = (screen_height - scaled_height) * 0.5;

                let grid_height_scaled = tile_grid.height() * scale;
                let grid_width_scaled = tile_grid.width() * scale;
                let bordered_grid_height_scaled = tile_grid.bordered_height() * scale;
                let bordered_grid_width_scaled = tile_grid.bordered_width() * scale;
                let tile_step = tile_grid.tile_length * scale;
                let cell_step = tile_step / tile_grid.cells_per_tile as f32;
                let grid_offset_x =
                    offset_x + TileGridPresentation::SIDE_BORDER_CELL_LAYERS as f32 * cell_step;
                let grid_offset_y =
                    offset_y + TileGridPresentation::TOP_BORDER_CELL_LAYERS as f32 * cell_step;
                let grid_color = to_macroquad_color(tile_grid.line_color);

                let subgrid_color = to_macroquad_color(tile_grid.line_color.lighten(0.6));

                let total_subcolumns = tile_grid.columns * tile_grid.cells_per_tile
                    + 2 * TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
                for column in 0..=total_subcolumns {
                    let x = offset_x + column as f32 * cell_step;
                    macroquad::shapes::draw_line(
                        x,
                        offset_y,
                        x,
                        offset_y + bordered_grid_height_scaled,
                        0.5,
                        subgrid_color,
                    );
                }

                let total_subrows = tile_grid.rows * tile_grid.cells_per_tile
                    + TileGridPresentation::TOP_BORDER_CELL_LAYERS
                    + TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS;
                for row in 0..=total_subrows {
                    let y = offset_y + row as f32 * cell_step;
                    macroquad::shapes::draw_line(
                        offset_x,
                        y,
                        offset_x + bordered_grid_width_scaled,
                        y,
                        0.5,
                        subgrid_color,
                    );
                }

                for column in 0..=tile_grid.columns {
                    let x = grid_offset_x + column as f32 * tile_step;
                    macroquad::shapes::draw_line(
                        x,
                        grid_offset_y,
                        x,
                        grid_offset_y + grid_height_scaled,
                        1.0,
                        grid_color,
                    );
                }

                for row in 0..=tile_grid.rows {
                    let y = grid_offset_y + row as f32 * tile_step;
                    macroquad::shapes::draw_line(
                        grid_offset_x,
                        y,
                        grid_offset_x + grid_width_scaled,
                        y,
                        1.0,
                        grid_color,
                    );
                }

                let wall_color = to_macroquad_color(wall.color);
                let wall_height = wall.thickness * scale;
                let wall_y = offset_y + bordered_grid_height_scaled;
                let wall_left = offset_x;
                let wall_right = offset_x + bordered_grid_width_scaled;

                let target = &wall.target;
                let target_cells = &target.cells;

                if target.is_empty() {
                    macroquad::shapes::draw_rectangle(
                        wall_left,
                        wall_y,
                        bordered_grid_width_scaled,
                        wall_height,
                        wall_color,
                    );
                } else {
                    let mut target_columns: Vec<u32> =
                        target_cells.iter().map(|cell| cell.column).collect();
                    target_columns.sort_unstable();
                    target_columns.dedup();

                    if let (Some(&first_column), Some(&last_column)) =
                        (target_columns.first(), target_columns.last())
                    {
                        let target_left = grid_offset_x + first_column as f32 * tile_step;
                        let target_right = grid_offset_x + (last_column + 1) as f32 * tile_step;

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

                        for column in target_columns {
                            let start_x = grid_offset_x + column as f32 * tile_step;
                            macroquad::shapes::draw_line(
                                start_x,
                                walkway_top,
                                start_x,
                                walkway_bottom,
                                1.0,
                                grid_color,
                            );

                            let end_x = grid_offset_x + (column + 1) as f32 * tile_step;
                            macroquad::shapes::draw_line(
                                end_x,
                                walkway_top,
                                end_x,
                                walkway_bottom,
                                1.0,
                                grid_color,
                            );

                            for subdivision in 1..tile_grid.cells_per_tile {
                                let subdivision_x = start_x + subdivision as f32 * cell_step;
                                macroquad::shapes::draw_line(
                                    subdivision_x,
                                    walkway_top,
                                    subdivision_x,
                                    walkway_bottom,
                                    0.5,
                                    subgrid_color,
                                );
                            }
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

                let bug_radius = cell_step * 0.5;
                for BugPresentation { column, row, color } in &scene.bugs {
                    let bug_center_x = grid_offset_x + (*column as f32 + 0.5) * cell_step;
                    let bug_center_y = grid_offset_y + (*row as f32 + 0.5) * cell_step;
                    macroquad::shapes::draw_circle(
                        bug_center_x,
                        bug_center_y,
                        bug_radius,
                        to_macroquad_color(*color),
                    );
                }

                macroquad::window::next_frame().await;
            }
        });

        Ok(())
    }
}

fn to_macroquad_color(color: maze_defence_rendering::Color) -> macroquad::color::Color {
    macroquad::color::Color::new(color.red, color.green, color.blue, color.alpha)
}
