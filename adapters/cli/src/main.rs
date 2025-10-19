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
use maze_defence_rendering::{
    Color, Presentation, RenderingBackend, Scene, TileGridPresentation, WallPresentation,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_world::World;

/// Entry point for the Maze Defence command-line interface.
fn main() -> Result<()> {
    let world = World::new();
    let bootstrap = Bootstrap::default();
    let banner = bootstrap.welcome_banner(&world);

    let tile_grid = bootstrap.tile_grid(&world);

    let grid_scene = TileGridPresentation::new(
        tile_grid.columns(),
        tile_grid.rows(),
        tile_grid.tile_length(),
        Color::from_rgb_u8(31, 54, 22),
    );

    let wall_scene = WallPresentation::new(
        tile_grid.tile_length() / 2.0,
        Color::from_rgb_u8(68, 45, 15),
    );

    let scene = Scene::new(grid_scene, wall_scene);

    let presentation = Presentation::new(banner.to_owned(), Color::from_rgb_u8(85, 142, 52), scene);

    MacroquadBackend::default().run(presentation)
}
