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

use std::{borrow::Cow, num::NonZeroU32, time::Duration};

use serde::{Deserialize, Serialize};

/// Domain structures that occupy individual navigation cells.
pub mod structures {
    use super::CellCoord;

    /// Permanent structure that occupies a single navigation cell.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Wall {
        cell: CellCoord,
    }

    impl Wall {
        /// Creates a wall anchored at the provided cell coordinate.
        #[must_use]
        pub const fn at(cell: CellCoord) -> Self {
            Self { cell }
        }

        /// Returns the cell guarded by the wall.
        #[must_use]
        pub const fn cell(&self) -> CellCoord {
            self.cell
        }

        /// Zero-based column index of the wall within the navigation grid.
        #[must_use]
        pub const fn column(&self) -> u32 {
            self.cell.column()
        }

        /// Zero-based row index of the wall within the navigation grid.
        #[must_use]
        pub const fn row(&self) -> u32 {
            self.cell.row()
        }
    }

    /// Read-only snapshot describing all cell-sized walls stored in the world.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct WallView {
        walls: Vec<Wall>,
    }

    impl WallView {
        /// Creates a new view from the provided wall collection.
        #[must_use]
        pub fn from_walls(mut walls: Vec<Wall>) -> Self {
            walls.sort_by_key(|wall| (wall.column(), wall.row()));
            walls.dedup();
            Self { walls }
        }

        /// Iterator over the captured wall descriptors in deterministic order.
        #[must_use = "iterators are lazy and do nothing unless consumed"]
        pub fn iter(&self) -> impl Iterator<Item = &Wall> {
            self.walls.iter()
        }

        /// Consumes the view, yielding the underlying wall descriptors.
        #[must_use]
        pub fn into_vec(self) -> Vec<Wall> {
            self.walls
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wall_view_sorts_and_deduplicates_cells() {
            let walls = vec![
                Wall::at(CellCoord::new(4, 2)),
                Wall::at(CellCoord::new(1, 1)),
                Wall::at(CellCoord::new(4, 2)),
                Wall::at(CellCoord::new(0, 3)),
            ];

            let view = WallView::from_walls(walls);
            let ordered: Vec<_> = view.iter().map(|wall| wall.cell()).collect();

            assert_eq!(
                ordered,
                vec![
                    CellCoord::new(0, 3),
                    CellCoord::new(1, 1),
                    CellCoord::new(4, 2),
                ]
            );
        }
    }
}

/// Canonical banner emitted when the experience boots.
pub const WELCOME_BANNER: &str = "Welcome to Maze Defence.";

/// Number of cells a congestion probe inspects ahead of each bug.
///
/// The pathing spec keeps this window small (five cells) so the gradient
/// dominates the decision making while still detecting jams slightly ahead of
/// the current position. Values in the `4..=6` range were validated during
/// authoring; any change must re-run the deterministic replay harness to keep
/// behaviour reproducible across runs.
pub const CONGESTION_LOOKAHEAD: u32 = 5;

/// Multiplier applied to congestion samples when scoring neighbours.
///
/// Keeping this weight at three preserves the gradient as the primary
/// signal—the congestion term only nudges decisions when distances tie. The
/// spec calls out that adjustments should remain low and deterministic, so
/// revisit replay fixtures before modifying the constant.
pub const CONGESTION_WEIGHT: u32 = 3;

/// Depth limit for the bounded detour breadth-first search.
///
/// A radius of six keeps detours local while guaranteeing the planner explores
/// small side corridors before stalling. Expanding this radius increases the
/// search cost and must be accompanied by replay updates to maintain
/// determinism.
pub const DETOUR_RADIUS: u32 = 6;

/// Describes the active gameplay mode for the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayMode {
    /// Standard attack mode where bugs advance toward the target.
    Attack,
    /// Builder mode that pauses simulation to enable planning and placement.
    Builder,
}

/// Authoritative description of a single hand-authored enemy wave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackPlan {
    pressure: u32,
    bursts: Vec<AttackBurst>,
}

impl AttackPlan {
    /// Creates a new attack plan populated with the provided bursts.
    #[must_use]
    pub fn new(pressure: u32, bursts: Vec<AttackBurst>) -> Self {
        Self { pressure, bursts }
    }

    /// Returns the pressure budget associated with this plan.
    #[must_use]
    pub const fn pressure(&self) -> u32 {
        self.pressure
    }

    /// Provides immutable access to the burst descriptors in deterministic order.
    #[must_use]
    pub fn bursts(&self) -> &[AttackBurst] {
        &self.bursts
    }

    /// Returns whether the plan contains no bursts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bursts.is_empty()
    }
}

/// Homogeneous burst scheduled within an [`AttackPlan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackBurst {
    spawner: CellCoord,
    bug: AttackBugDescriptor,
    count: NonZeroU32,
    cadence_ms: NonZeroU32,
    start_ms: u32,
}

impl AttackBurst {
    /// Creates a new burst descriptor spawning identical bugs from a single spawner.
    #[must_use]
    pub fn new(
        spawner: CellCoord,
        bug: AttackBugDescriptor,
        count: NonZeroU32,
        cadence_ms: NonZeroU32,
        start_ms: u32,
    ) -> Self {
        Self {
            spawner,
            bug,
            count,
            cadence_ms,
            start_ms,
        }
    }

    /// Returns the cell coordinate of the spawner assigned to the burst.
    #[must_use]
    pub const fn spawner(&self) -> CellCoord {
        self.spawner
    }

    /// Returns the bug descriptor emitted by this burst.
    #[must_use]
    pub const fn bug(&self) -> AttackBugDescriptor {
        self.bug
    }

    /// Returns the number of bugs emitted by the burst.
    #[must_use]
    pub const fn count(&self) -> NonZeroU32 {
        self.count
    }

    /// Returns the cadence in milliseconds between consecutive spawns.
    #[must_use]
    pub const fn cadence_ms(&self) -> NonZeroU32 {
        self.cadence_ms
    }

    /// Returns the start offset in milliseconds relative to the beginning of the wave.
    #[must_use]
    pub const fn start_ms(&self) -> u32 {
        self.start_ms
    }
}

/// Describes the appearance and behaviour of bugs spawned by an [`AttackBurst`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackBugDescriptor {
    color: BugColor,
    health: Health,
    step_ms: u32,
}

impl AttackBugDescriptor {
    /// Creates a new bug descriptor using the provided appearance and pacing.
    #[must_use]
    pub const fn new(color: BugColor, health: Health, step_ms: u32) -> Self {
        Self {
            color,
            health,
            step_ms,
        }
    }

    /// Returns the colour assigned to spawned bugs.
    #[must_use]
    pub const fn color(&self) -> BugColor {
        self.color
    }

    /// Returns the health applied to spawned bugs.
    #[must_use]
    pub const fn health(&self) -> Health {
        self.health
    }

    /// Returns the bug step cadence expressed in milliseconds.
    #[must_use]
    pub const fn step_ms(&self) -> u32 {
        self.step_ms
    }
}

/// Outcome emitted when resolving a round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoundOutcome {
    /// Indicates that the defenders successfully cleared the round.
    Win,
    /// Indicates that the defenders lost the round.
    Loss,
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
        /// Resolved cadence in milliseconds required between steps.
        step_ms: u32,
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
    /// Resolves the previously concluded round with the provided outcome.
    ResolveRound {
        /// Outcome that should be applied to the world state.
        outcome: RoundOutcome,
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
    /// Announces that the current round ended in defeat.
    RoundLost {
        /// Identifier of the bug that caused the loss by reaching the exit.
        bug: BugId,
    },
    /// Announces that the simulation entered a new play mode.
    PlayModeChanged {
        /// Mode that became active after processing commands.
        mode: PlayMode,
    },
    /// Reports that the player's gold balance changed.
    GoldChanged {
        /// Total gold owned after the adjustment.
        amount: Gold,
    },
    /// Reports that the experience's difficulty tier changed.
    DifficultyTierChanged {
        /// Difficulty tier active after the adjustment.
        tier: u32,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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

/// Amount of gold owned by the defending player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Gold(u32);

impl Gold {
    /// Canonical zero value representing a lack of gold.
    pub const ZERO: Self = Self(0);

    /// Creates a new gold value from the provided raw integer.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Retrieves the underlying gold amount.
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0
    }

    /// Adds another gold amount while saturating at `u32::MAX`.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Subtracts another gold amount while saturating at zero.
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
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

/// Immutable representation of a projectile maintained by the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectileSnapshot {
    /// Identifier allocated to the projectile by the world.
    pub projectile: ProjectileId,
    /// Tower that fired the projectile.
    pub tower: TowerId,
    /// Intended bug target recorded at launch time.
    pub target: BugId,
    /// Starting point of the projectile expressed in half-cell units.
    pub origin_half: CellPointHalf,
    /// Destination point of the projectile expressed in half-cell units.
    pub dest_half: CellPointHalf,
    /// Total distance the projectile must travel measured in half-cell units.
    pub distance_half: u128,
    /// Distance already travelled by the projectile measured in half-cell units.
    pub travelled_half: u128,
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

/// Coordinate anchored to the centre of a cell measured in half-cell units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellPointHalf {
    column_half: i64,
    row_half: i64,
}

impl CellPointHalf {
    /// Creates a new half-cell point using the provided integer coordinates.
    #[must_use]
    pub const fn new(column_half: i64, row_half: i64) -> Self {
        Self {
            column_half,
            row_half,
        }
    }

    /// Half-cell column coordinate represented as a signed integer.
    #[must_use]
    pub const fn column_half(&self) -> i64 {
        self.column_half
    }

    /// Half-cell row coordinate represented as a signed integer.
    #[must_use]
    pub const fn row_half(&self) -> i64 {
        self.row_half
    }

    /// Computes the rounded Euclidean distance to another point in half-cell units.
    #[must_use]
    pub fn distance_to(self, other: Self) -> u128 {
        let dx = i128::from(other.column_half).saturating_sub(i128::from(self.column_half));
        let dy = i128::from(other.row_half).saturating_sub(i128::from(self.row_half));
        let squared = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) as u128;
        let mut distance = integer_sqrt(squared);
        if distance.saturating_mul(distance) < squared {
            distance = distance.saturating_add(1);
        }
        distance
    }
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }

    let mut op = value;
    let mut result = 0_u128;
    let mut bit = 1_u128 << 126;

    while bit > op {
        bit >>= 2;
    }

    while bit != 0 {
        if op >= result + bit {
            op -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }

    result
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

/// Immutable representation of a single bug's state used for queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BugSnapshot {
    /// Unique identifier assigned to the bug.
    pub id: BugId,
    /// Grid cell currently occupied by the bug.
    pub cell: CellCoord,
    /// Appearance assigned to the bug.
    pub color: BugColor,
    /// Maximum health the bug was spawned with.
    pub max_health: Health,
    /// Remaining health stored for the bug.
    pub health: Health,
    /// Resolved cadence in milliseconds that governs when the bug may step.
    ///
    /// The cadence is always an integer duration measured in milliseconds.
    /// Systems never inspect provenance—every bug exposes the exact cadence it
    /// must satisfy before advancing.
    pub step_ms: u32,
    /// Accumulated cadence progress measured in milliseconds.
    ///
    /// The accumulator is clamped by the world so it never exceeds
    /// [`BugSnapshot::step_ms`]. World tick handlers advance this value with
    /// pure integer math and carry any remainder after a movement step.
    pub accum_ms: u32,
    /// Indicates whether the bug accrued enough time to advance.
    ///
    /// The flag is derived from the cadence fields inside the world using the
    /// relation `accum_ms >= step_ms` and is the only gate movement systems
    /// consult when planning steps.
    ///
    /// ```
    /// use maze_defence_core::{BugColor, BugId, BugSnapshot, CellCoord, Health};
    ///
    /// let step_ms = 400;
    /// let mut snapshot = BugSnapshot {
    ///     id: BugId::new(7),
    ///     cell: CellCoord::new(0, 0),
    ///     color: BugColor::from_rgb(0xff, 0, 0),
    ///     max_health: Health::new(3),
    ///     health: Health::new(3),
    ///     step_ms,
    ///     accum_ms: 0,
    ///     ready_for_step: false,
    /// };
    ///
    /// snapshot.accum_ms = snapshot.accum_ms.saturating_add(200);
    /// snapshot.ready_for_step = snapshot.accum_ms >= snapshot.step_ms;
    /// assert!(!snapshot.ready_for_step);
    ///
    /// snapshot.accum_ms = (snapshot.accum_ms + 200).min(snapshot.step_ms);
    /// snapshot.ready_for_step = snapshot.accum_ms >= snapshot.step_ms;
    /// assert!(snapshot.ready_for_step);
    /// assert_eq!(snapshot.accum_ms, snapshot.step_ms);
    /// ```
    pub ready_for_step: bool,
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

/// Read-only snapshot of the pre-computed navigation distances.
///
/// The field stores a deterministic Manhattan distance for every cell in the
/// maze plus the virtual exit row. Consumers may borrow the backing buffer or
/// take ownership of it, but the API only ever exposes immutable slices so
/// callers cannot mutate authoritative data.
///
/// ```
/// use maze_defence_core::{CellCoord, NavigationFieldView};
///
/// // 3×2 field laid out in row-major order pointing toward the exit in the
/// // bottom-right corner.
/// let view = NavigationFieldView::from_owned(vec![3, 2, 1, 2, 1, 0], 3, 2);
///
/// assert_eq!(view.width(), 3);
/// assert_eq!(view.height(), 2);
/// assert_eq!(view.distance(CellCoord::new(2, 1)), Some(0));
/// assert_eq!(view.cells()[0], 3);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationFieldView<'a> {
    width: u32,
    height: u32,
    #[serde(borrow)]
    distances: Cow<'a, [u16]>,
}

impl<'a> NavigationFieldView<'a> {
    /// Captures a new view borrowing the provided navigation distances.
    #[must_use]
    pub fn from_slice(distances: &'a [u16], width: u32, height: u32) -> Self {
        Self::with_distances(Cow::Borrowed(distances), width, height)
    }

    /// Captures a new view that owns its navigation distances outright.
    #[must_use]
    pub fn from_owned(
        distances: Vec<u16>,
        width: u32,
        height: u32,
    ) -> NavigationFieldView<'static> {
        NavigationFieldView::with_distances(Cow::Owned(distances), width, height)
    }

    fn with_distances(distances: Cow<'a, [u16]>, width: u32, height: u32) -> Self {
        let expected_len = NavigationFieldView::expected_len(width, height);
        assert_eq!(
            distances.len(),
            expected_len,
            "navigation field dimensions must match the backing buffer length",
        );
        Self {
            width,
            height,
            distances,
        }
    }

    fn expected_len(width: u32, height: u32) -> usize {
        let width = usize::try_from(width).expect("width fits usize");
        let height = usize::try_from(height).expect("height fits usize");
        width
            .checked_mul(height)
            .expect("navigation field dimensions stay within addressable range")
    }

    fn index_of(&self, cell: CellCoord) -> Option<usize> {
        if cell.column() >= self.width || cell.row() >= self.height {
            return None;
        }

        let column = usize::try_from(cell.column()).ok()?;
        let row = usize::try_from(cell.row()).ok()?;
        let width = usize::try_from(self.width).ok()?;
        Some(row * width + column)
    }

    /// Width of the navigation field measured in cells.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height of the navigation field measured in cells.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Immutable view of the dense distances stored in row-major order.
    #[must_use]
    pub fn cells(&self) -> &[u16] {
        &self.distances
    }

    /// Reports the stored distance for the provided cell, if within bounds.
    #[must_use]
    pub fn distance(&self, cell: CellCoord) -> Option<u16> {
        self.index_of(cell)
            .and_then(|index| self.distances.get(index).copied())
    }

    /// Iterator over all distances in row-major order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&'a self) -> impl Iterator<Item = u16> + 'a {
        self.distances.iter().copied()
    }

    /// Converts the view into an owned variant, cloning the backing buffer when required.
    #[must_use]
    pub fn into_owned(self) -> NavigationFieldView<'static> {
        NavigationFieldView {
            width: self.width,
            height: self.height,
            distances: Cow::Owned(self.distances.into_owned()),
        }
    }
}

/// Immutable claim describing a bug's requested destination for the current tick.
///
/// The reservation ledger collects these claims before the world applies any
/// movement. Systems may inspect the data to understand which bugs are already
/// queued to vacate specific cells and bias their own planning accordingly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationClaim {
    bug_id: BugId,
    direction: Direction,
}

impl ReservationClaim {
    /// Creates a new reservation claim for the provided bug and direction.
    #[must_use]
    pub const fn new(bug_id: BugId, direction: Direction) -> Self {
        Self { bug_id, direction }
    }

    /// Identifier of the bug that owns the reservation.
    #[must_use]
    pub const fn bug_id(&self) -> BugId {
        self.bug_id
    }

    /// Direction the bug intends to travel during this tick.
    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }
}

/// Read-only snapshot enumerating all pending movement reservations.
///
/// The ledger mirrors the world's reservation queue for the active tick. It is
/// deterministic—claims are sorted by [`BugId`]—so systems may iterate the
/// entries without additional ordering work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationLedgerView<'a> {
    #[serde(borrow)]
    claims: Cow<'a, [ReservationClaim]>,
}

impl<'a> ReservationLedgerView<'a> {
    /// Captures a ledger view borrowing the underlying reservation claims.
    #[must_use]
    pub fn from_slice(claims: &'a [ReservationClaim]) -> Self {
        Self {
            claims: Cow::Borrowed(claims),
        }
    }

    /// Captures a ledger view that owns the provided reservation claims outright.
    #[must_use]
    pub fn from_owned(claims: Vec<ReservationClaim>) -> ReservationLedgerView<'static> {
        ReservationLedgerView {
            claims: Cow::Owned(claims),
        }
    }

    /// Number of claims stored in the ledger for the current tick.
    #[must_use]
    pub fn len(&self) -> usize {
        self.claims.len()
    }

    /// Reports whether the ledger currently tracks any reservations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    /// Iterator over the captured reservation claims in deterministic order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&'a self) -> impl Iterator<Item = ReservationClaim> + 'a {
        self.claims.iter().copied()
    }

    /// Retrieves the reservation registered for the provided bug, if any.
    #[must_use]
    pub fn claim_for(&self, bug_id: BugId) -> Option<ReservationClaim> {
        self.claims
            .iter()
            .copied()
            .find(|claim| claim.bug_id == bug_id)
    }

    /// Consumes the view, yielding an owned ledger snapshot.
    #[must_use]
    pub fn into_owned(self) -> ReservationLedgerView<'static> {
        ReservationLedgerView {
            claims: Cow::Owned(self.claims.into_owned()),
        }
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

/// Immutable representation of a tower's firing cooldown state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TowerCooldownSnapshot {
    /// Identifier allocated to the tower by the world.
    pub tower: TowerId,
    /// Kind of tower that owns the cooldown.
    pub kind: TowerKind,
    /// Duration remaining before the tower may fire again.
    pub ready_in: Duration,
}

/// Read-only snapshot describing tower cooldown progress.
#[derive(Clone, Debug, Default)]
pub struct TowerCooldownView {
    snapshots: Vec<TowerCooldownSnapshot>,
}

impl TowerCooldownView {
    /// Creates a new view from the provided cooldown snapshots.
    #[must_use]
    pub fn from_snapshots(mut snapshots: Vec<TowerCooldownSnapshot>) -> Self {
        snapshots.sort_by_key(|snapshot| snapshot.tower);
        Self { snapshots }
    }

    /// Iterator over cooldown snapshots in deterministic order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&self) -> impl Iterator<Item = &TowerCooldownSnapshot> {
        self.snapshots.iter()
    }

    /// Consumes the view, yielding the underlying cooldown snapshots.
    #[must_use]
    pub fn into_vec(self) -> Vec<TowerCooldownSnapshot> {
        self.snapshots
    }
}

/// Point in cell space expressed using floating point coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellPoint {
    column: f32,
    row: f32,
}

impl CellPoint {
    /// Creates a point located at the provided column and row coordinates.
    #[must_use]
    pub const fn new(column: f32, row: f32) -> Self {
        Self { column, row }
    }

    /// Column coordinate measured in cell units.
    #[must_use]
    pub const fn column(&self) -> f32 {
        self.column
    }

    /// Row coordinate measured in cell units.
    #[must_use]
    pub const fn row(&self) -> f32 {
        self.row
    }
}

/// Target assignment describing a tower aiming at a specific bug.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TowerTarget {
    /// Identifier of the tower emitting the targeting beam.
    pub tower: TowerId,
    /// Identifier of the bug selected as the target.
    pub bug: BugId,
    /// Centre of the tower footprint expressed in cell coordinates.
    pub tower_center_cells: CellPoint,
    /// Centre of the targeted bug expressed in cell coordinates.
    pub bug_center_cells: CellPoint,
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

    /// Damage dealt by a projectile fired by this tower kind.
    #[must_use]
    pub const fn projectile_damage(self) -> Damage {
        match self {
            Self::Basic => Damage::new(1),
        }
    }

    /// Time in milliseconds for a projectile to traverse the tower's maximum range.
    #[must_use]
    pub const fn projectile_travel_time_ms(self) -> u32 {
        match self {
            Self::Basic => 1_000,
        }
    }

    /// Gold required to construct a tower of this kind.
    #[must_use]
    pub const fn build_cost(self) -> Gold {
        match self {
            Self::Basic => Gold::new(10),
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
    /// The placement would block all paths between the exit and bug spawners.
    PathBlocked,
    /// The world cannot afford the tower's construction cost.
    InsufficientFunds,
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
        CellCoord, CellRect, CellRectSize, Damage, Gold, Health, NavigationFieldView,
        PlacementError, ProjectileId, ProjectileRejection, RemovalError, TowerId, TowerKind,
        CONGESTION_LOOKAHEAD, CONGESTION_WEIGHT, DETOUR_RADIUS,
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

    fn deserialize_navigation_field<'de>(bytes: &'de [u8]) -> NavigationFieldView<'de> {
        bincode::deserialize(bytes).expect("deserialize navigation field")
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
    fn gold_round_trips_through_bincode() {
        let gold = Gold::new(27);
        assert_round_trip(&gold);
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
        assert_round_trip(&PlacementError::PathBlocked);
        assert_round_trip(&PlacementError::InsufficientFunds);
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
        assert_eq!(TowerKind::Basic.projectile_travel_time_ms(), 1_000);
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
    fn navigation_constants_match_specification() {
        assert_eq!(CONGESTION_LOOKAHEAD, 5);
        assert_eq!(CONGESTION_WEIGHT, 3);
        assert_eq!(DETOUR_RADIUS, 6);
    }

    #[test]
    fn navigation_field_view_round_trips_through_bincode() {
        let view = NavigationFieldView::from_owned(vec![3, 2, 1, 2, 1, 0], 3, 2);
        let bytes = bincode::serialize(&view).expect("serialize navigation field");
        let restored = deserialize_navigation_field(&bytes).into_owned();
        assert_eq!(restored, view);
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
