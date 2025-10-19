#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Core contracts shared across the Maze Defence engine.

/// Canonical banner emitted when the experience boots.
pub const WELCOME_BANNER: &str = "Welcome to Maze Defence.";

/// Commands that express all permissible world mutations.
#[derive(Debug)]
pub enum Command {
    /// Configures the world's tile grid using the provided dimensions.
    ConfigureTileGrid {
        /// Number of columns laid out in the grid.
        columns: u32,
        /// Number of rows laid out in the grid.
        rows: u32,
        /// Length of each square tile measured in world units.
        tile_length: f32,
    },
}
