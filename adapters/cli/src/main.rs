#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

use anyhow::Result;
use maze_defence_rendering::{Color, Presentation, RenderingBackend};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_world::World;

/// Entry point for the Maze Defence command-line interface.
fn main() -> Result<()> {
    let world = World::new();
    let bootstrap = Bootstrap::default();
    let banner = bootstrap.welcome_banner(&world);

    let presentation = Presentation::new(banner.to_owned(), Color::from_rgb_u8(85, 142, 52));

    MacroquadBackend::default().run(presentation)
}
