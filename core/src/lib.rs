#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Core contracts shared across the Maze Defence engine.
//!
//! This crate defines the message surface that connects adapters, the
//! authoritative world, and pure systems. Adapters submit [`Command`] values
//! describing desired mutations, the world executes those commands via its
//! `apply` entry point, and then broadcasts [`Event`] values for systems to
//! react to deterministically. Systems consume event streams, query immutable
//! snapshots, and respond exclusively with new command batches.

use std::time::Duration;

/// Canonical banner emitted when the experience boots.
pub const WELCOME_BANNER: &str = "Welcome to Maze Defence.";

/// Commands that express all permissible world mutations.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Configures the world's tile grid using the provided dimensions.
    ConfigureTileGrid {
        /// Number of tile columns laid out in the grid.
        columns: TileCoord,
        /// Number of tile rows laid out in the grid.
        rows: TileCoord,
        /// Length of each square tile measured in world units.
        tile_length: f32,
    },
    /// Advances the simulation clock by the provided delta time.
    Tick {
        /// Duration of simulated time that elapsed since the previous tick.
        dt: Duration,
    },
    /// Supplies a freshly computed path for the bug to follow.
    SetBugPath {
        /// Identifier of the bug that should adopt the path.
        bug_id: BugId,
        /// Ordered list of cell coordinates that subdivide each tile.
        path: Vec<CellCoord>,
    },
    /// Requests that a bug advance a single step in the specified direction.
    StepBug {
        /// Identifier of the bug attempting to move.
        bug_id: BugId,
        /// Direction of travel for the attempted step.
        direction: Direction,
    },
}

/// Events broadcast by the world after processing commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Indicates that the simulation clock advanced.
    TimeAdvanced {
        /// Duration of simulated time that elapsed in the tick.
        dt: Duration,
    },
    /// Signals that a bug requires a new path to continue navigating.
    BugPathNeeded {
        /// Identifier of the bug that needs a path recomputed.
        bug_id: BugId,
    },
    /// Confirms that a bug successfully moved between two cells.
    BugAdvanced {
        /// Identifier of the bug that advanced.
        bug_id: BugId,
        /// Cell the bug occupied before moving. Cells subdivide individual tiles.
        from: CellCoord,
        /// Cell the bug occupies after completing the move. Cells subdivide individual tiles.
        to: CellCoord,
    },
}

/// Cardinal movement directions available to bugs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Movement toward decreasing row indices.
    North,
    /// Movement toward increasing column indices.
    East,
    /// Movement toward increasing row indices.
    South,
    /// Movement toward decreasing column indices.
    West,
}

/// Unique identifier assigned to a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BugId(u32);

impl BugId {
    /// Creates a new bug identifier with the provided numeric value.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the numeric representation of the identifier.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }
}

/// Location of a single grid cell expressed as column and row coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellCoord {
    column: u32,
    row: u32,
}

impl CellCoord {
    /// Creates a new grid cell coordinate.
    #[must_use]
    pub const fn new(column: u32, row: u32) -> Self {
        Self { column, row }
    }

    /// Zero-based column index of the cell.
    #[must_use]
    pub const fn column(&self) -> u32 {
        self.column
    }

    /// Zero-based row index of the cell.
    #[must_use]
    pub const fn row(&self) -> u32 {
        self.row
    }
}

/// Index within the tile grid measured in whole tiles rather than cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord(u32);

impl TileCoord {
    /// Creates a new tile coordinate wrapper.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the underlying tile index.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }
}
