#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Macroquad-backed rendering adapter for Maze Defence.

use anyhow::Result;
use macroquad::input::{is_key_pressed, KeyCode};
use maze_defence_rendering::{Presentation, RenderingBackend};

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

                let tile_grid = scene.tile_grid;
                let wall = scene.wall;

                let world_width = tile_grid.width();
                let world_height = scene.total_height();
                let scale = (screen_width / world_width).min(screen_height / world_height);

                let scaled_width = world_width * scale;
                let scaled_height = world_height * scale;
                let offset_x = (screen_width - scaled_width) * 0.5;
                let offset_y = (screen_height - scaled_height) * 0.5;

                let grid_height_scaled = tile_grid.height() * scale;
                let grid_width_scaled = tile_grid.width() * scale;
                let tile_step = tile_grid.tile_length * scale;
                let grid_color = to_macroquad_color(tile_grid.line_color);

                for column in 0..=tile_grid.columns {
                    let x = offset_x + column as f32 * tile_step;
                    macroquad::shapes::draw_line(
                        x,
                        offset_y,
                        x,
                        offset_y + grid_height_scaled,
                        1.0,
                        grid_color,
                    );
                }

                for row in 0..=tile_grid.rows {
                    let y = offset_y + row as f32 * tile_step;
                    macroquad::shapes::draw_line(
                        offset_x,
                        y,
                        offset_x + grid_width_scaled,
                        y,
                        1.0,
                        grid_color,
                    );
                }

                let wall_color = to_macroquad_color(wall.color);
                let wall_height = wall.thickness * scale;
                let wall_y = offset_y + grid_height_scaled;
                macroquad::shapes::draw_rectangle(
                    offset_x,
                    wall_y,
                    grid_width_scaled,
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
