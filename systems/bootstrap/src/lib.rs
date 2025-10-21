#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Pure bootstrap system that prepares the Maze Defence experience.

use maze_defence_core::{BugView, Target, TileGrid, Wall};
use maze_defence_world::{query, World};

/// Produces data required to greet the player.
#[derive(Debug, Default)]
pub struct Bootstrap;

impl Bootstrap {
    /// Derives the banner that should be shown when the experience starts.
    #[must_use]
    pub fn welcome_banner<'world>(&self, world: &'world World) -> &'world str {
        query::welcome_banner(world)
    }

    /// Exposes the tile grid configuration required for rendering.
    #[must_use]
    pub fn tile_grid<'world>(&self, world: &'world World) -> &'world TileGrid {
        query::tile_grid(world)
    }

    /// Exposes the bugs currently inhabiting the maze for presentation purposes.
    #[must_use]
    pub fn bugs(&self, world: &World) -> BugView {
        query::bug_view(world)
    }

    /// Exposes the perimeter wall guarding the maze.
    #[must_use]
    pub fn wall<'world>(&self, world: &'world World) -> &'world Wall {
        query::wall(world)
    }

    /// Exposes the target carved into the wall for presentation.
    #[must_use]
    pub fn target<'world>(&self, world: &'world World) -> &'world Target {
        query::target(world)
    }
}
