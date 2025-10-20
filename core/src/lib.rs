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

use serde::{Deserialize, Serialize};

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
    /// Requests that a bug spawner emit a new bug into the maze.
    SpawnBug {
        /// Location of the spawner responsible for creating the bug.
        spawner: CellCoord,
        /// Appearance to assign to the spawned bug.
        color: BugColor,
    },
    /// Requests placement of a tower anchored at the provided origin cell.
    PlaceTower {
        /// Type of tower to construct at the origin.
        kind: TowerKind,
        /// Upper-left cell that defines the tower's footprint.
        origin: CellCoord,
    },
    /// Requests removal of an existing tower from the world.
    RemoveTower {
        /// Identifier of the tower targeted for removal.
        tower: TowerId,
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
    /// Confirms that a bug was created by a spawner.
    BugSpawned {
        /// Identifier assigned to the newly spawned bug.
        bug_id: BugId,
        /// Cell the bug occupies after spawning.
        cell: CellCoord,
        /// Appearance applied to the bug.
        color: BugColor,
    },
    /// Confirms that a tower was placed into the world.
    TowerPlaced {
        /// Identifier assigned to the tower by the world.
        tower: TowerId,
        /// Type of tower that was placed.
        kind: TowerKind,
        /// Region of cells occupied by the tower.
        region: CellRect,
    },
    /// Confirms that a tower was removed from the world.
    TowerRemoved {
        /// Identifier of the tower that was removed.
        tower: TowerId,
        /// Region of cells previously occupied by the tower.
        region: CellRect,
    },
    /// Reports that a tower placement request was rejected.
    TowerPlacementRejected {
        /// Type of tower requested for placement.
        kind: TowerKind,
        /// Origin cell provided in the placement request.
        origin: CellCoord,
        /// Specific reason the placement failed.
        reason: PlacementError,
    },
    /// Reports that a tower removal request was rejected.
    TowerRemovalRejected {
        /// Identifier of the tower targeted for removal.
        tower: TowerId,
        /// Specific reason the removal failed.
        reason: RemovalError,
    },
}

/// Visual appearance applied to a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BugColor {
    red: u8,
    green: u8,
    blue: u8,
}

impl BugColor {
    /// Creates a new bug color from byte RGB components.
    #[must_use]
    pub const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Red component of the color.
    #[must_use]
    pub const fn red(&self) -> u8 {
        self.red
    }

    /// Green component of the color.
    #[must_use]
    pub const fn green(&self) -> u8 {
        self.green
    }

    /// Blue component of the color.
    #[must_use]
    pub const fn blue(&self) -> u8 {
        self.blue
    }
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

/// Unique identifier assigned to a tower.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TowerId(u32);

impl TowerId {
    /// Creates a new tower identifier with the provided numeric value.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the numeric representation of the tower identifier.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }
}

/// Location of a single grid cell expressed as column and row coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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

/// Axis-aligned rectangle expressed in cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellRect {
    origin: CellCoord,
    size: CellRectSize,
}

impl CellRect {
    /// Constructs a rectangle from an origin cell and size.
    #[must_use]
    pub const fn from_origin_and_size(origin: CellCoord, size: CellRectSize) -> Self {
        Self { origin, size }
    }

    /// Upper-left cell that anchors the rectangle.
    #[must_use]
    pub const fn origin(&self) -> CellCoord {
        self.origin
    }

    /// Dimensions of the rectangle measured in whole cells.
    #[must_use]
    pub const fn size(&self) -> CellRectSize {
        self.size
    }
}

/// Size of a [`CellRect`] measured in whole cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellRectSize {
    width: u32,
    height: u32,
}

impl CellRectSize {
    /// Creates a new size descriptor with explicit dimensions.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Width of the rectangle in cells.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height of the rectangle in cells.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }
}

/// Types of towers that can be constructed in the maze.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TowerKind {
    /// Basic tower with default attack parameters.
    Basic,
}

/// Reasons a tower placement request may be rejected by the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlacementError {
    /// The simulation is not in builder mode, so placement is disabled.
    InvalidMode,
    /// The requested region extends beyond the configured grid bounds.
    OutOfBounds,
    /// The provided origin cell does not satisfy alignment requirements.
    Misaligned,
    /// The requested footprint overlaps an occupied cell.
    Occupied,
}

/// Reasons a tower removal request may be rejected by the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemovalError {
    /// The simulation is not in builder mode, so removal is disabled.
    InvalidMode,
    /// No tower with the provided identifier exists.
    MissingTower,
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
    use super::{
        select_goal, CellCoord, CellRect, CellRectSize, Goal, PlacementError, RemovalError,
        TowerId, TowerKind,
    };
    use serde::{de::DeserializeOwned, Serialize};

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

    fn assert_round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let bytes = bincode::serialize(value).expect("serialize");
        let restored: T = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(&restored, value);
    }

    #[test]
    fn tower_id_round_trips_through_bincode() {
        let tower_id = TowerId::new(42);
        assert_round_trip(&tower_id);
    }

    #[test]
    fn tower_kind_round_trips_through_bincode() {
        assert_round_trip(&TowerKind::Basic);
    }

    #[test]
    fn placement_error_round_trips_through_bincode() {
        assert_round_trip(&PlacementError::Occupied);
    }

    #[test]
    fn removal_error_round_trips_through_bincode() {
        assert_round_trip(&RemovalError::MissingTower);
    }

    #[test]
    fn cell_rect_round_trips_through_bincode() {
        let origin = CellCoord::new(5, 7);
        let size = CellRectSize::new(2, 3);
        let rect = CellRect::from_origin_and_size(origin, size);
        assert_round_trip(&rect);
    }
}
