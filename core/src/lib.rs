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

/// Describes the active gameplay mode for the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayMode {
    /// Standard attack mode where bugs advance toward the target.
    Attack,
    /// Builder mode that pauses simulation to enable planning and placement.
    Builder,
}

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
        /// Number of navigation cells subdividing each tile edge.
        cells_per_tile: u32,
    },
    /// Updates the duration a bug must accumulate before attempting another step.
    ConfigureBugStep {
        /// Minimum simulated time required between successive bug steps.
        step_duration: Duration,
    },
    /// Advances the simulation clock by the provided delta time.
    Tick {
        /// Duration of simulated time that elapsed since the previous tick.
        dt: Duration,
    },
    /// Requests that a bug advance a single step in the specified direction.
    StepBug {
        /// Identifier of the bug attempting to move.
        bug_id: BugId,
        /// Direction of travel for the attempted step.
        direction: Direction,
    },
    /// Requests that the world transition to the provided play mode.
    SetPlayMode {
        /// Mode the world should activate.
        mode: PlayMode,
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
    /// Confirms that a bug successfully moved between two cells.
    BugAdvanced {
        /// Identifier of the bug that advanced.
        bug_id: BugId,
        /// Cell the bug occupied before moving. Cells subdivide individual tiles.
        from: CellCoord,
        /// Cell the bug occupies after completing the move. Cells subdivide individual tiles.
        to: CellCoord,
    },
    /// Announces that the simulation entered a new play mode.
    PlayModeChanged {
        /// Mode that became active after processing commands.
        mode: PlayMode,
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

    /// Computes the Manhattan distance between two cell coordinates.
    #[must_use]
    pub fn manhattan_distance(self, other: CellCoord) -> u32 {
        self.column().abs_diff(other.column()) + self.row().abs_diff(other.row())
    }
}

/// Canonical representation of "The Goal" for a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Goal {
    cell: CellCoord,
}

impl Goal {
    /// Creates a goal anchored at the provided cell coordinate.
    #[must_use]
    pub const fn at(cell: CellCoord) -> Self {
        Self { cell }
    }

    /// Returns the cell that defines the goal.
    #[must_use]
    pub const fn cell(&self) -> CellCoord {
        self.cell
    }
}

/// Selects the goal cell nearest to the provided origin.
#[must_use]
pub fn select_goal(origin: CellCoord, candidates: &[CellCoord]) -> Option<Goal> {
    candidates
        .iter()
        .copied()
        .min_by(|left, right| {
            let left_distance = origin.manhattan_distance(*left);
            let right_distance = origin.manhattan_distance(*right);
            left_distance
                .cmp(&right_distance)
                .then_with(|| left.column().cmp(&right.column()))
                .then_with(|| left.row().cmp(&right.row()))
        })
        .map(Goal::at)
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

#[cfg(test)]
mod tests {
    use super::{select_goal, CellCoord, Goal};

    #[test]
    fn manhattan_distance_matches_expectation() {
        let origin = CellCoord::new(1, 1);
        let destination = CellCoord::new(4, 3);
        assert_eq!(origin.manhattan_distance(destination), 5);
        assert_eq!(destination.manhattan_distance(origin), 5);
    }

    #[test]
    fn select_goal_prefers_closest_cell() {
        let origin = CellCoord::new(3, 2);
        let candidates = [
            CellCoord::new(0, 5),
            CellCoord::new(3, 5),
            CellCoord::new(4, 4),
        ];

        let goal = select_goal(origin, &candidates);
        assert_eq!(goal, Some(Goal::at(CellCoord::new(3, 5))));
    }
}
