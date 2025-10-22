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

use std::{num::NonZeroU32, time::Duration};

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
        /// Health assigned to the spawned bug.
        health: Health,
    },
    /// Requests that a tower fire a projectile at a targeted bug.
    FireProjectile {
        /// Identifier of the tower attempting to shoot.
        tower: TowerId,
        /// Identifier of the targeted bug.
        target: BugId,
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
    /// Indicates that a bug triggered the exit and left the maze.
    BugExited {
        /// Identifier of the bug that exited the maze.
        bug_id: BugId,
        /// Cell that triggered the exit when the bug entered it.
        cell: CellCoord,
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
        /// Health assigned to the bug on spawn.
        health: Health,
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
    /// Confirms that a projectile was fired at a target bug.
    ProjectileFired {
        /// Identifier of the spawned projectile.
        projectile: ProjectileId,
        /// Tower responsible for firing the projectile.
        tower: TowerId,
        /// Bug targeted by the projectile.
        target: BugId,
    },
    /// Reports that the projectile reached its target and applied damage.
    ProjectileHit {
        /// Identifier of the projectile that connected.
        projectile: ProjectileId,
        /// Bug that was struck by the projectile.
        target: BugId,
        /// Damage applied to the bug.
        damage: Damage,
    },
    /// Reports that a projectile expired before hitting a living bug.
    ProjectileExpired {
        /// Identifier of the projectile that expired.
        projectile: ProjectileId,
    },
    /// Reports that a firing attempt was rejected by the world.
    ProjectileRejected {
        /// Tower that attempted to fire.
        tower: TowerId,
        /// Intended bug target.
        target: BugId,
        /// Specific reason the request failed.
        reason: ProjectileRejection,
    },
    /// Reports that a bug took damage from a projectile hit.
    BugDamaged {
        /// Bug that took damage.
        bug: BugId,
        /// Remaining health after damage was applied.
        remaining: Health,
    },
    /// Announces that a bug died because its health reached zero.
    BugDied {
        /// Identifier of the bug that died.
        bug: BugId,
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

/// Amount of health a bug currently possesses.
///
/// A `Health` value of zero represents a dead bug. Health never becomes
/// negative—arithmetic performed with [`Damage`] saturates at zero so callers
/// can rely on monotonic, deterministic behaviour when subtracting damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Health(u32);

impl Health {
    /// Canonical zero value representing a dead bug.
    pub const ZERO: Self = Self(0);

    /// Creates a new health value from the provided raw integer.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the underlying health amount.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }

    /// Reports whether the health value equals zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Subtracts the provided damage while saturating at zero.
    #[must_use]
    pub const fn saturating_sub(self, damage: Damage) -> Self {
        Self(self.0.saturating_sub(damage.get()))
    }
}

/// Fixed amount of damage applied to a bug in a single hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Damage(u32);

impl Damage {
    /// Creates a new damage value from the provided raw integer.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the underlying damage amount.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }
}

/// Unique identifier assigned to a projectile fired by a tower.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProjectileId(u32);

impl ProjectileId {
    /// Creates a new projectile identifier with the provided numeric value.
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

/// Reasons the world may reject a projectile firing request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectileRejection {
    /// Towers cannot fire while the simulation is in builder mode.
    InvalidMode,
    /// The tower's cooldown has not yet elapsed.
    CooldownActive,
    /// The targeted tower either does not exist or was removed earlier.
    MissingTower,
    /// The intended bug target does not exist or already died.
    MissingTarget,
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

/// Describes the discrete tile layout of the maze.
#[derive(Clone, Debug, PartialEq)]
pub struct TileGrid {
    columns: TileCoord,
    rows: TileCoord,
    tile_length: f32,
}

impl TileGrid {
    /// Creates a new tile grid description.
    #[must_use]
    pub const fn new(columns: TileCoord, rows: TileCoord, tile_length: f32) -> Self {
        Self {
            columns,
            rows,
            tile_length,
        }
    }

    /// Number of columns contained in the grid.
    #[must_use]
    pub const fn columns(&self) -> TileCoord {
        self.columns
    }

    /// Number of rows contained in the grid.
    #[must_use]
    pub const fn rows(&self) -> TileCoord {
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
        self.columns.get() as f32 * self.tile_length
    }

    /// Total height of the grid measured in world units.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.rows.get() as f32 * self.tile_length
    }
}

/// Perimeter wall that guards the maze interior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wall {
    target: Target,
}

impl Wall {
    /// Creates a new wall containing the provided target opening.
    #[must_use]
    pub fn new(target: Target) -> Self {
        Self { target }
    }

    /// Retrieves the target carved into the wall.
    #[must_use]
    pub const fn target(&self) -> &Target {
        &self.target
    }
}

/// Opening carved into the wall that bugs attempt to reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    cells: Vec<TargetCell>,
}

impl Target {
    /// Creates a target composed of the provided cells.
    #[must_use]
    pub fn new(cells: Vec<TargetCell>) -> Self {
        Self { cells }
    }

    /// Cells that make up the target opening.
    #[must_use]
    pub fn cells(&self) -> &[TargetCell] {
        &self.cells
    }
}

/// Discrete cell that composes part of the wall target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetCell {
    cell: CellCoord,
}

impl TargetCell {
    /// Creates a new target cell located at the provided column and row.
    #[must_use]
    pub const fn new(column: u32, row: u32) -> Self {
        Self {
            cell: CellCoord::new(column, row),
        }
    }

    /// Column that contains the cell relative to the tile grid.
    #[must_use]
    pub const fn column(&self) -> u32 {
        self.cell.column()
    }

    /// Row that contains the cell relative to the tile grid.
    #[must_use]
    pub const fn row(&self) -> u32 {
        self.cell.row()
    }

    /// Returns the complete cell coordinate for the target cell.
    #[must_use]
    pub const fn cell(&self) -> CellCoord {
        self.cell
    }
}

/// Permanent wall segment occupying a single navigation cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WallCell {
    cell: CellCoord,
}

impl WallCell {
    /// Creates a new wall segment anchored to the provided cell.
    #[must_use]
    pub const fn new(column: u32, row: u32) -> Self {
        Self {
            cell: CellCoord::new(column, row),
        }
    }

    /// Zero-based column index of the wall segment.
    #[must_use]
    pub const fn column(&self) -> u32 {
        self.cell.column()
    }

    /// Zero-based row index of the wall segment.
    #[must_use]
    pub const fn row(&self) -> u32 {
        self.cell.row()
    }

    /// Returns the full coordinate occupied by the wall segment.
    #[must_use]
    pub const fn cell(&self) -> CellCoord {
        self.cell
    }
}

/// Immutable representation of a single bug's state used for queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BugSnapshot {
    /// Unique identifier assigned to the bug.
    pub id: BugId,
    /// Grid cell currently occupied by the bug.
    pub cell: CellCoord,
    /// Appearance assigned to the bug.
    pub color: BugColor,
    /// Indicates whether the bug accrued enough time to advance.
    pub ready_for_step: bool,
    /// Duration accumulated toward the next step.
    pub accumulated: Duration,
}

/// Read-only snapshot describing all bugs within the maze.
#[derive(Clone, Debug, Default)]
pub struct BugView {
    snapshots: Vec<BugSnapshot>,
}

impl BugView {
    /// Creates a new bug view from the provided snapshots.
    #[must_use]
    pub fn from_snapshots(mut snapshots: Vec<BugSnapshot>) -> Self {
        snapshots.sort_by_key(|snapshot| snapshot.id);
        Self { snapshots }
    }

    /// Iterator over the captured bug snapshots in deterministic order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&self) -> impl Iterator<Item = &BugSnapshot> {
        self.snapshots.iter()
    }

    /// Consumes the view, yielding the underlying snapshots.
    #[must_use]
    pub fn into_vec(self) -> Vec<BugSnapshot> {
        self.snapshots
    }
}

/// Read-only view into the dense occupancy grid.
#[derive(Clone, Copy, Debug)]
pub struct OccupancyView<'a> {
    cells: &'a [Option<BugId>],
    columns: u32,
    rows: u32,
}

impl<'a> OccupancyView<'a> {
    /// Captures a new occupancy view backed by the provided cell slice.
    #[must_use]
    pub fn new(cells: &'a [Option<BugId>], columns: u32, rows: u32) -> Self {
        Self {
            cells,
            columns,
            rows,
        }
    }

    /// Returns the bug occupying the provided cell, if any.
    #[must_use]
    pub fn occupant(&self, cell: CellCoord) -> Option<BugId> {
        self.index(cell)
            .and_then(|index| self.cells.get(index).copied().flatten())
    }

    /// Reports whether the cell is currently free for traversal.
    #[must_use]
    pub fn is_free(&self, cell: CellCoord) -> bool {
        self.index(cell)
            .is_none_or(|index| self.cells.get(index).copied().unwrap_or(None).is_none())
    }

    /// Returns an iterator over all cells.
    pub fn iter(&self) -> impl Iterator<Item = Option<BugId>> + 'a {
        self.cells.iter().copied()
    }

    /// Provides the dimensions of the underlying occupancy grid.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }

    fn index(&self, cell: CellCoord) -> Option<usize> {
        if cell.column() < self.columns && cell.row() < self.rows {
            let row = usize::try_from(cell.row()).ok()?;
            let column = usize::try_from(cell.column()).ok()?;
            let width = usize::try_from(self.columns).ok()?;
            Some(row * width + column)
        } else {
            None
        }
    }
}

/// Read-only snapshot describing all towers placed within the maze.
#[derive(Clone, Debug, Default)]
pub struct TowerView {
    snapshots: Vec<TowerSnapshot>,
}

impl TowerView {
    /// Creates a new tower view from the provided snapshots.
    #[must_use]
    pub fn from_snapshots(mut snapshots: Vec<TowerSnapshot>) -> Self {
        snapshots.sort_by_key(|snapshot| snapshot.id);
        Self { snapshots }
    }

    /// Iterator over the captured tower snapshots in deterministic order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&self) -> impl Iterator<Item = &TowerSnapshot> {
        self.snapshots.iter()
    }

    /// Consumes the view, yielding the underlying snapshots.
    #[must_use]
    pub fn into_vec(self) -> Vec<TowerSnapshot> {
        self.snapshots
    }
}

/// Immutable representation of a single tower's state used for queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TowerSnapshot {
    /// Identifier allocated to the tower by the world.
    pub id: TowerId,
    /// Kind of tower that was constructed.
    pub kind: TowerKind,
    /// Region of cells occupied by the tower.
    pub region: CellRect,
}

/// Types of towers that can be constructed in the maze.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TowerKind {
    /// Basic tower with default attack parameters.
    Basic,
}

impl TowerKind {
    /// Returns the tower's targeting range measured in tiles.
    ///
    /// `TowerKind::Basic` covers a radius of four tiles.
    #[must_use]
    pub const fn range_in_tiles(self) -> f32 {
        match self {
            Self::Basic => 4.0,
        }
    }

    /// Converts the tower's targeting range into whole cell units.
    ///
    /// Guarantees: returns `floor(self.range_in_tiles() * cells_per_tile)`.
    /// This method never rounds up, ensuring deterministic, grid-aligned
    /// behaviour that mirrors the targeting system's integer half-cell checks.
    /// Targeting converts this radius to half-cells (`radius_half =
    /// radius_cells * 2`) for integer-only distance comparisons.
    ///
    /// The provided `cells_per_tile` factor originates from the authoritative
    /// world configuration. A value of zero produces a zero radius so that
    /// callers never observe negative or undefined distances. Fractional
    /// results are truncated via the floor operation to keep the returned
    /// radius aligned with the discrete cell grid used by systems. This
    /// convenience helper tolerates zero for ergonomics; use
    /// [`TowerKind::range_in_cells_nz`] when the configuration is already
    /// validated.
    ///
    /// # Examples
    ///
    /// ```
    /// use maze_defence_core::TowerKind;
    ///
    /// let cells_per_tile = 3;
    /// assert_eq!(TowerKind::Basic.range_in_cells(cells_per_tile), 4 * cells_per_tile);
    /// ```
    #[must_use]
    pub fn range_in_cells(self, cells_per_tile: u32) -> u32 {
        if cells_per_tile == 0 {
            return 0;
        }

        let scaled = self.range_in_tiles() * cells_per_tile as f32;
        scaled.floor() as u32
    }

    /// Converts the tower's targeting range into whole cell units while
    /// encoding the non-zero invariant for `cells_per_tile` at the type level.
    ///
    /// This variant mirrors [`TowerKind::range_in_cells`] but accepts a
    /// [`NonZeroU32`] to ensure callers uphold the positive spacing guarantee
    /// established by the world configuration.
    #[must_use]
    pub fn range_in_cells_nz(self, cells_per_tile: NonZeroU32) -> u32 {
        let scaled = self.range_in_tiles() * cells_per_tile.get() as f32;
        scaled.floor() as u32
    }

    /// Cooldown between successive shots measured in milliseconds.
    #[must_use]
    pub const fn fire_cooldown_ms(self) -> u32 {
        match self {
            Self::Basic => 1_000,
        }
    }

    /// Projectile speed expressed in half-cell units advanced per millisecond.
    #[must_use]
    pub const fn speed_half_cells_per_ms(self) -> u32 {
        match self {
            Self::Basic => 12,
        }
    }

    /// Damage dealt by a projectile fired by this tower kind.
    #[must_use]
    pub const fn projectile_damage(self) -> Damage {
        match self {
            Self::Basic => Damage::new(1),
        }
    }
}

/// Reasons a tower placement request may be rejected by the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    use std::num::NonZeroU32;

    use super::{
        CellCoord, CellRect, CellRectSize, Damage, Health, PlacementError, ProjectileId,
        ProjectileRejection, RemovalError, TowerId, TowerKind,
    };
    use serde::{de::DeserializeOwned, Serialize};

    #[test]
    fn manhattan_distance_matches_expectation() {
        let origin = CellCoord::new(1, 1);
        let destination = CellCoord::new(4, 3);
        assert_eq!(origin.manhattan_distance(destination), 5);
        assert_eq!(destination.manhattan_distance(origin), 5);
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
    fn projectile_id_round_trips_through_bincode() {
        let projectile = ProjectileId::new(7);
        assert_round_trip(&projectile);
    }

    #[test]
    fn damage_round_trips_through_bincode() {
        let damage = Damage::new(3);
        assert_round_trip(&damage);
    }

    #[test]
    fn health_round_trips_through_bincode() {
        let health = Health::new(9);
        assert_round_trip(&health);
    }

    #[test]
    fn projectile_rejection_round_trips_through_bincode() {
        assert_round_trip(&ProjectileRejection::CooldownActive);
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

    #[test]
    fn tower_basic_range_in_tiles_matches_specification() {
        assert_eq!(TowerKind::Basic.range_in_tiles(), 4.0);
    }

    #[test]
    fn tower_basic_fire_cooldown_matches_specification() {
        assert_eq!(TowerKind::Basic.fire_cooldown_ms(), 1_000);
    }

    #[test]
    fn tower_basic_projectile_speed_matches_specification() {
        assert_eq!(TowerKind::Basic.speed_half_cells_per_ms(), 12);
    }

    #[test]
    fn tower_basic_projectile_damage_matches_specification() {
        assert_eq!(TowerKind::Basic.projectile_damage(), Damage::new(1));
    }

    #[test]
    fn tower_range_in_cells_scales_with_configuration() {
        let cells_per_tile = 3;
        assert_eq!(TowerKind::Basic.range_in_cells(cells_per_tile), 12);
    }

    #[test]
    fn tower_range_in_cells_handles_zero_configuration() {
        assert_eq!(TowerKind::Basic.range_in_cells(0), 0);
    }

    #[test]
    fn tower_range_in_cells_handles_large_configuration() {
        let cells_per_tile = 10_000;
        assert_eq!(TowerKind::Basic.range_in_cells(cells_per_tile), 40_000);
    }

    #[test]
    fn range_in_cells_is_monotonic_and_truncates() {
        for cells_per_tile in 0..=32 {
            let got = TowerKind::Basic.range_in_cells(cells_per_tile);
            let expect = (TowerKind::Basic.range_in_tiles() * cells_per_tile as f32).floor() as u32;
            assert_eq!(got, expect, "cpt={cells_per_tile}");
        }

        let mut previous = 0;
        for cells_per_tile in 0..=32 {
            let now = TowerKind::Basic.range_in_cells(cells_per_tile);
            assert!(now >= previous, "cpt={cells_per_tile}");
            previous = now;
        }
    }

    #[test]
    fn health_saturating_sub_saturates_at_zero() {
        let start = Health::new(5);
        let reduced = start.saturating_sub(Damage::new(2));
        assert_eq!(reduced, Health::new(3));

        let zeroed = start.saturating_sub(Damage::new(9));
        assert_eq!(zeroed, Health::ZERO);
        assert!(zeroed.is_zero());
    }

    #[test]
    fn range_in_cells_nz_matches_truncating_contract() {
        let cells_per_tile = NonZeroU32::new(7).expect("non-zero");
        assert_eq!(
            TowerKind::Basic.range_in_cells_nz(cells_per_tile),
            TowerKind::Basic.range_in_cells(cells_per_tile.get())
        );
    }
}
