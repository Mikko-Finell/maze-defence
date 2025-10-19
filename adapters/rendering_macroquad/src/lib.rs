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
use maze_defence_rendering::{Presentation, RenderingBackend};

/// Rendering backend implemented on top of macroquad.
#[derive(Debug, Default)]
pub struct MacroquadBackend;

impl RenderingBackend for MacroquadBackend {
    fn run(self, presentation: Presentation) -> Result<()> {
        let mut config = macroquad::window::Conf::default();
        config.window_title = presentation.window_title.clone();

        let clear_color = presentation.clear_color;

        macroquad::Window::from_config(config, async move {
            let background = macroquad::color::Color::new(
                clear_color.red,
                clear_color.green,
                clear_color.blue,
                clear_color.alpha,
            );

            loop {
                macroquad::window::clear_background(background);
                macroquad::window::next_frame().await;
            }
        });

        Ok(())
    }
}
