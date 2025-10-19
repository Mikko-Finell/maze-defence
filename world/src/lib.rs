#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

use maze_defence_core::WELCOME_BANNER;

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;

/// Describes the discrete tile layout of the world.
#[derive(Debug)]
pub struct TileGrid {
    columns: u32,
    rows: u32,
    tile_length: f32,
}

impl TileGrid {
    /// Creates a new tile grid description.
    #[must_use]
    pub const fn new(columns: u32, rows: u32, tile_length: f32) -> Self {
        Self {
            columns,
            rows,
            tile_length,
        }
    }

    /// Number of columns contained in the grid.
    #[must_use]
    pub const fn columns(&self) -> u32 {
        self.columns
    }

    /// Number of rows contained in the grid.
    #[must_use]
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    /// Side length of a single square tile expressed in world units.
    #[must_use]
    pub const fn tile_length(&self) -> f32 {
        self.tile_length
    }

    /// Total width of the grid measured in world units.
    #[must_use]
    pub const fn width(&self) -> f32 {
        self.columns as f32 * self.tile_length
    }

    /// Total height of the grid measured in world units.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.rows as f32 * self.tile_length
    }
}

/// Represents the authoritative Maze Defence world state.
#[derive(Debug)]
pub struct World {
    banner: &'static str,
    tile_grid: TileGrid,
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a Maze Defence world with the provided tile grid definition.
    #[must_use]
    pub fn with_tile_grid(columns: u32, rows: u32, tile_length: f32) -> Self {
        Self {
            banner: WELCOME_BANNER,
            tile_grid: TileGrid::new(columns, rows, tile_length),
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::with_tile_grid(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS, DEFAULT_TILE_LENGTH)
    }
}

/// Query functions that provide read-only access to the world state.
pub mod query {
    use super::{TileGrid, World};

    /// Retrieves the welcome banner that adapters may display to players.
    #[must_use]
    pub fn welcome_banner(world: &World) -> &'static str {
        world.banner
    }

    /// Provides read-only access to the world's tile grid definition.
    #[must_use]
    pub fn tile_grid(world: &World) -> &TileGrid {
        &world.tile_grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_world_matches_new() {
        let default_world = World::default();
        let new_world = World::new();

        assert_eq!(
            query::welcome_banner(&default_world),
            query::welcome_banner(&new_world)
        );

        let default_grid = query::tile_grid(&default_world);
        let new_grid = query::tile_grid(&new_world);

        assert_eq!(default_grid.columns(), new_grid.columns());
        assert_eq!(default_grid.rows(), new_grid.rows());
        assert!((default_grid.tile_length() - new_grid.tile_length()).abs() < f32::EPSILON);
    }

    #[test]
    fn default_world_uses_expected_dimensions() {
        let world = World::default();
        let grid = query::tile_grid(&world);

        assert_eq!(grid.columns(), DEFAULT_GRID_COLUMNS);
        assert_eq!(grid.rows(), DEFAULT_GRID_ROWS);
        assert!((grid.tile_length() - DEFAULT_TILE_LENGTH).abs() < f32::EPSILON);
    }
}
