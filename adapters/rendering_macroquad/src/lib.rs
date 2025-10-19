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
use maze_defence_rendering::{Presentation, RenderingBackend, TileGridPresentation};

/// Rendering backend implemented on top of macroquad.
#[derive(Debug, Default)]
pub struct MacroquadBackend;

impl RenderingBackend for MacroquadBackend {
    fn run(self, presentation: Presentation) -> Result<()> {
        let Presentation {
            window_title,
            clear_color,
            scene,
        } = presentation;

        let config = macroquad::window::Conf {
            window_title,
            window_width: 960,
            window_height: 960,
            ..macroquad::window::Conf::default()
        };

        macroquad::Window::from_config(config, async move {
            let background = to_macroquad_color(clear_color);

            loop {
                if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::Q) {
                    break;
                }

                macroquad::window::clear_background(background);

                let screen_width = macroquad::window::screen_width();
                let screen_height = macroquad::window::screen_height();

                let tile_grid = scene.tile_grid;
                let wall = scene.wall;

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
                let subcell_step = tile_step / tile_grid.subdivisions_per_tile as f32;
                let grid_offset_x = offset_x
                    + TileGridPresentation::SIDE_BORDER_SUBCELL_LAYERS as f32 * subcell_step;
                let grid_offset_y = offset_y
                    + TileGridPresentation::TOP_BORDER_SUBCELL_LAYERS as f32 * subcell_step;
                let grid_color = to_macroquad_color(tile_grid.line_color);

                let subgrid_color = to_macroquad_color(tile_grid.line_color.lighten(0.6));

                let total_subcolumns = tile_grid.columns * tile_grid.subdivisions_per_tile
                    + 2 * TileGridPresentation::SIDE_BORDER_SUBCELL_LAYERS;
                for column in 0..=total_subcolumns {
                    let x = offset_x + column as f32 * subcell_step;
                    macroquad::shapes::draw_line(
                        x,
                        offset_y,
                        x,
                        offset_y + bordered_grid_height_scaled,
                        0.5,
                        subgrid_color,
                    );
                }

                let total_subrows = tile_grid.rows * tile_grid.subdivisions_per_tile
                    + TileGridPresentation::TOP_BORDER_SUBCELL_LAYERS
                    + TileGridPresentation::BOTTOM_BORDER_SUBCELL_LAYERS;
                for row in 0..=total_subrows {
                    let y = offset_y + row as f32 * subcell_step;
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
                macroquad::shapes::draw_rectangle(
                    offset_x,
                    wall_y,
                    bordered_grid_width_scaled,
                    wall_height,
                    wall_color,
                );

                macroquad::window::next_frame().await;
            }
        });

        Ok(())
    }
}

fn to_macroquad_color(color: maze_defence_rendering::Color) -> macroquad::color::Color {
    macroquad::color::Color::new(color.red, color.green, color.blue, color.alpha)
}
