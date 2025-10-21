#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

use std::{collections::BTreeSet, time::Duration};

#[cfg(any(test, feature = "tower_scaffolding"))]
mod towers;

#[cfg(any(test, feature = "tower_scaffolding"))]
use towers::{footprint_for, TowerRegistry, TowerState};

use maze_defence_core::{
    BugColor, BugId, CellCoord, Command, Direction, Event, PlayMode, TileCoord, WELCOME_BANNER,
};

#[cfg(any(test, feature = "tower_scaffolding"))]
use maze_defence_core::{CellRect, PlacementError, RemovalError, TowerId, TowerKind};

const DEFAULT_GRID_COLUMNS: TileCoord = TileCoord::new(10);
const DEFAULT_GRID_ROWS: TileCoord = TileCoord::new(10);
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_CELLS_PER_TILE: u32 = 1;

const DEFAULT_STEP_QUANTUM: Duration = Duration::from_millis(250);
const MIN_STEP_QUANTUM: Duration = Duration::from_micros(1);
const SIDE_BORDER_CELL_LAYERS: u32 = 1;
const TOP_BORDER_CELL_LAYERS: u32 = 1;

/// Describes the discrete tile layout of the world.
#[derive(Debug)]
pub struct TileGrid {
    columns: TileCoord,
    rows: TileCoord,
    tile_length: f32,
}

impl TileGrid {
    /// Creates a new tile grid description.
    #[must_use]
    pub(crate) const fn new(columns: TileCoord, rows: TileCoord, tile_length: f32) -> Self {
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

/// Describes the perimeter wall that surrounds the tile grid.
#[derive(Debug)]
pub struct Wall {
    target: Target,
}

impl Wall {
    /// Creates a new wall aligned with the provided grid dimensions.
    #[must_use]
    pub(crate) fn new(columns: TileCoord, rows: TileCoord, cells_per_tile: u32) -> Self {
        Self {
            target: Target::aligned_with_grid(columns, rows, cells_per_tile),
        }
    }

    /// Retrieves the target carved into the perimeter wall.
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }
}

/// Opening carved into the wall to connect the maze with the outside world.
#[derive(Debug)]
pub struct Target {
    cells: Vec<TargetCell>,
}

impl Target {
    fn aligned_with_grid(columns: TileCoord, rows: TileCoord, cells_per_tile: u32) -> Self {
        Self {
            cells: target_cells(columns, rows, cells_per_tile),
        }
    }

    /// Collection of cells that occupy the target within the wall.
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

/// Represents the authoritative Maze Defence world state.
#[derive(Debug)]
pub struct World {
    banner: &'static str,
    tile_grid: TileGrid,
    cells_per_tile: u32,
    wall: Wall,
    targets: Vec<CellCoord>,
    bugs: Vec<Bug>,
    bug_spawners: BugSpawnerRegistry,
    next_bug_id: u32,
    occupancy: OccupancyGrid,
    #[cfg(any(test, feature = "tower_scaffolding"))]
    towers: TowerRegistry,
    #[cfg(any(test, feature = "tower_scaffolding"))]
    tower_occupancy: BitGrid,
    reservations: ReservationFrame,
    tick_index: u64,
    step_quantum: Duration,
    play_mode: PlayMode,
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        let tile_grid = TileGrid::new(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS, DEFAULT_TILE_LENGTH);
        let cells_per_tile = DEFAULT_CELLS_PER_TILE;
        let wall = Wall::new(tile_grid.columns(), tile_grid.rows(), cells_per_tile);
        let targets = target_cells_from_wall(&wall);
        let total_columns = total_cell_columns(tile_grid.columns(), cells_per_tile);
        let total_rows = total_cell_rows(tile_grid.rows(), cells_per_tile);
        let occupancy = OccupancyGrid::new(total_columns, total_rows);
        #[cfg(any(test, feature = "tower_scaffolding"))]
        let tower_occupancy = BitGrid::new(total_columns, total_rows);
        let mut world = Self {
            banner: WELCOME_BANNER,
            bugs: Vec::new(),
            bug_spawners: BugSpawnerRegistry::new(),
            next_bug_id: 0,
            occupancy,
            #[cfg(any(test, feature = "tower_scaffolding"))]
            towers: TowerRegistry::new(),
            #[cfg(any(test, feature = "tower_scaffolding"))]
            tower_occupancy,
            reservations: ReservationFrame::new(),
            wall,
            targets,
            tile_grid,
            cells_per_tile,
            tick_index: 0,
            step_quantum: DEFAULT_STEP_QUANTUM,
            play_mode: PlayMode::Attack,
        };
        world.rebuild_bug_spawners();
        world.clear_bugs();
        world
    }

    fn clear_bugs(&mut self) {
        self.bugs.clear();
        self.occupancy.clear();
        self.reservations.clear();
        self.next_bug_id = 0;
    }

    fn iter_bugs_mut(&mut self) -> impl Iterator<Item = &mut Bug> {
        self.bugs.iter_mut()
    }

    fn bug_index(&self, bug_id: BugId) -> Option<usize> {
        self.bugs.iter().position(|bug| bug.id == bug_id)
    }

    fn spawn_from_spawner(
        &mut self,
        cell: CellCoord,
        color: BugColor,
        out_events: &mut Vec<Event>,
    ) {
        if !self.bug_spawners.contains(cell) {
            return;
        }

        if self.occupancy.index(cell).is_none() || !self.occupancy.can_enter(cell) {
            return;
        }

        let bug_id = self.next_bug_identifier();
        let bug = Bug::new(bug_id, cell, color);
        self.occupancy.occupy(bug_id, cell);
        self.bugs.push(bug);
        out_events.push(Event::BugSpawned {
            bug_id,
            cell,
            color,
        });
    }

    fn next_bug_identifier(&mut self) -> BugId {
        let bug_id = BugId::new(self.next_bug_id);
        self.next_bug_id = self.next_bug_id.saturating_add(1);
        bug_id
    }

    fn resolve_pending_steps(&mut self, out_events: &mut Vec<Event>) {
        let requests = self.reservations.drain_sorted();
        if requests.is_empty() {
            return;
        }

        let mut exited_bugs: Vec<BugId> = Vec::new();
        let (columns, rows) = self.occupancy.dimensions();
        let target_columns: Vec<u32> = self.targets.iter().map(|cell| cell.column()).collect();
        for request in requests {
            let Some(index) = self.bug_index(request.bug_id) else {
                continue;
            };

            let (before, after) = self.bugs.split_at_mut(index);
            let bug = &mut after[0];
            let from = bug.cell;

            if bug.accumulator < self.step_quantum {
                continue;
            }

            let Some(next_cell) =
                advance_cell(from, request.direction, columns, rows, &target_columns)
            else {
                continue;
            };

            if !self.occupancy.can_enter(next_cell) {
                continue;
            }

            let reached_target = self.targets.iter().any(|target| *target == next_cell);

            self.occupancy.vacate(from);
            self.occupancy.occupy(bug.id, next_cell);
            bug.advance(next_cell);
            bug.accumulator = bug.accumulator.saturating_sub(self.step_quantum);
            out_events.push(Event::BugAdvanced {
                bug_id: bug.id,
                from,
                to: next_cell,
            });

            if reached_target {
                self.occupancy.vacate(next_cell);
                exited_bugs.push(bug.id);
                continue;
            }

            let _ = before;
        }

        for bug_id in exited_bugs {
            if let Some(position) = self.bug_index(bug_id) {
                let _ = self.bugs.remove(position);
            }
        }
    }
}

/// Applies the provided command to the world, mutating state deterministically.
pub fn apply(world: &mut World, command: Command, out_events: &mut Vec<Event>) {
    match command {
        Command::ConfigureTileGrid {
            columns,
            rows,
            tile_length,
            cells_per_tile,
        } => {
            world.tile_grid = TileGrid::new(columns, rows, tile_length);
            let normalized_cells = cells_per_tile.max(1);
            world.cells_per_tile = normalized_cells;
            world.wall = Wall::new(columns, rows, normalized_cells);
            world.targets = target_cells_from_wall(&world.wall);
            let total_columns = total_cell_columns(columns, normalized_cells);
            let total_rows = total_cell_rows(rows, normalized_cells);
            world.occupancy = OccupancyGrid::new(total_columns, total_rows);
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.tower_occupancy = BitGrid::new(total_columns, total_rows);
                world.towers = TowerRegistry::new();
            }
            world.rebuild_bug_spawners();
            world.clear_bugs();
        }
        Command::Tick { dt } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }
            world.tick_index = world.tick_index.saturating_add(1);
            out_events.push(Event::TimeAdvanced { dt });

            let step_quantum = world.step_quantum;
            for bug in world.iter_bugs_mut() {
                bug.accumulator = bug.accumulator.saturating_add(dt).min(step_quantum);
            }
        }
        Command::ConfigureBugStep { step_duration } => {
            let clamped = step_duration.max(MIN_STEP_QUANTUM);
            world.step_quantum = clamped;
        }
        Command::StepBug { bug_id, direction } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }
            world
                .reservations
                .queue(world.tick_index, StepRequest { bug_id, direction });
            world.resolve_pending_steps(out_events);
        }
        Command::SetPlayMode { mode } => {
            if world.play_mode == mode {
                return;
            }

            world.play_mode = mode;

            match mode {
                PlayMode::Attack => {}
                PlayMode::Builder => {
                    world.clear_bugs();
                }
            }

            out_events.push(Event::PlayModeChanged { mode });
        }
        Command::SpawnBug { spawner, color } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }

            world.spawn_from_spawner(spawner, color, out_events);
        }
        Command::PlaceTower { kind, origin } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_place_tower(kind, origin, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = (kind, origin);
        }
        Command::RemoveTower { tower } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_remove_tower(tower, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = tower;
        }
    }
}

impl World {
    fn rebuild_bug_spawners(&mut self) {
        let (columns, rows) = self.occupancy.dimensions();
        self.bug_spawners.assign_outer_rim(columns, rows);
        self.bug_spawners.remove_bottom_row(columns, rows);
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn handle_place_tower(
        &mut self,
        kind: TowerKind,
        origin: CellCoord,
        out_events: &mut Vec<Event>,
    ) {
        if self.play_mode != PlayMode::Builder {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::InvalidMode,
            });
            return;
        }

        if let Some(stride) = self.tower_alignment_stride() {
            let Some(column_alignment) = origin.column().checked_sub(SIDE_BORDER_CELL_LAYERS)
            else {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            };
            let Some(row_alignment) = origin.row().checked_sub(TOP_BORDER_CELL_LAYERS) else {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            };
            if column_alignment % stride != 0 || row_alignment % stride != 0 {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            }
        }

        let footprint = footprint_for(kind);
        let region = CellRect::from_origin_and_size(origin, footprint);

        if !self.tower_region_within_bounds(region) {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::OutOfBounds,
            });
            return;
        }

        if self.tower_region_occupied(region) {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::Occupied,
            });
            return;
        }

        let id = self.towers.allocate();
        self.mark_tower_region(region, true);
        self.towers.insert(TowerState { id, kind, region });
        debug_assert!(self.towers.get(id).is_some());
        out_events.push(Event::TowerPlaced {
            tower: id,
            kind,
            region,
        });
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn handle_remove_tower(&mut self, tower: TowerId, out_events: &mut Vec<Event>) {
        if self.play_mode != PlayMode::Builder {
            out_events.push(Event::TowerRemovalRejected {
                tower,
                reason: RemovalError::InvalidMode,
            });
            return;
        }

        let Some(state) = self.towers.remove(tower) else {
            out_events.push(Event::TowerRemovalRejected {
                tower,
                reason: RemovalError::MissingTower,
            });
            return;
        };

        self.mark_tower_region(state.region, false);
        out_events.push(Event::TowerRemoved {
            tower: state.id,
            region: state.region,
        });
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_alignment_stride(&self) -> Option<u32> {
        let stride = self.cells_per_tile / 2;
        if stride <= 1 {
            None
        } else {
            Some(stride)
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_within_bounds(&self, region: CellRect) -> bool {
        let (columns, rows) = self.tower_occupancy.dimensions();
        let size = region.size();
        if size.width() == 0 || size.height() == 0 {
            return false;
        }

        let origin = region.origin();
        if origin.column() >= columns || origin.row() >= rows {
            return false;
        }

        let Some(end_column) = origin.column().checked_add(size.width()) else {
            return false;
        };
        let Some(end_row) = origin.row().checked_add(size.height()) else {
            return false;
        };

        end_column <= columns && end_row <= rows
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_occupied(&self, region: CellRect) -> bool {
        let origin = region.origin();
        let size = region.size();

        for column_offset in 0..size.width() {
            for row_offset in 0..size.height() {
                let column = origin
                    .column()
                    .checked_add(column_offset)
                    .expect("column bounded by region");
                let row = origin
                    .row()
                    .checked_add(row_offset)
                    .expect("row bounded by region");
                let cell = CellCoord::new(column, row);
                if self.tower_occupancy.contains(cell) {
                    return true;
                }
                if !self.occupancy.can_enter(cell) {
                    return true;
                }
            }
        }

        false
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn mark_tower_region(&mut self, region: CellRect, occupied: bool) {
        let origin = region.origin();
        let size = region.size();

        for column_offset in 0..size.width() {
            for row_offset in 0..size.height() {
                let column = origin
                    .column()
                    .checked_add(column_offset)
                    .expect("column bounded by region");
                let row = origin
                    .row()
                    .checked_add(row_offset)
                    .expect("row bounded by region");
                let cell = CellCoord::new(column, row);
                if occupied {
                    self.tower_occupancy.set(cell);
                } else {
                    self.tower_occupancy.clear(cell);
                }
            }
        }
    }
}

/// Query functions that provide read-only access to the world state.
pub mod query {
    use std::time::Duration;

    use super::{OccupancyGrid, Target, TileGrid, Wall, World};
    use maze_defence_core::{select_goal, BugColor, BugId, CellCoord, Goal, PlayMode};

    #[cfg(any(test, feature = "tower_scaffolding"))]
    use maze_defence_core::{CellRect, TowerId, TowerKind};

    /// Reports the active play mode for the world.
    #[must_use]
    pub fn play_mode(world: &World) -> PlayMode {
        world.play_mode
    }

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

    /// Provides read-only access to the wall guarding the maze perimeter.
    #[must_use]
    pub fn wall(world: &World) -> &Wall {
        &world.wall
    }

    /// Provides read-only access to the target carved into the perimeter wall.
    #[must_use]
    pub fn target(world: &World) -> &Target {
        world.wall.target()
    }

    /// Computes the canonical goal for an entity starting from the provided cell.
    #[must_use]
    pub fn goal_for(world: &World, origin: CellCoord) -> Option<Goal> {
        select_goal(origin, &world.targets)
    }

    /// Captures a read-only view of the bugs inhabiting the maze.
    #[must_use]
    pub fn bug_view(world: &World) -> BugView {
        let mut snapshots: Vec<BugSnapshot> = world
            .bugs
            .iter()
            .map(|bug| BugSnapshot {
                id: bug.id,
                cell: bug.cell,
                color: bug.color,
                ready_for_step: bug.ready_for_step(world.step_quantum),
                accumulated: bug.accumulator,
            })
            .collect();
        snapshots.sort_by_key(|snapshot| snapshot.id);
        BugView { snapshots }
    }

    /// Exposes a read-only view of the dense occupancy grid.
    #[must_use]
    pub fn occupancy_view(world: &World) -> OccupancyView<'_> {
        OccupancyView {
            grid: &world.occupancy,
        }
    }

    /// Captures a read-only snapshot of all towers stored in the world.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[must_use]
    pub fn towers(world: &World) -> TowerView {
        if world.towers.is_empty() {
            return TowerView {
                snapshots: Vec::new(),
            };
        }

        let snapshots = world
            .towers
            .iter()
            .map(|tower| TowerSnapshot {
                id: tower.id,
                kind: tower.kind,
                region: tower.region,
            })
            .collect();
        TowerView { snapshots }
    }

    /// Reports whether the provided cell is blocked by the world state.
    #[must_use]
    pub fn is_cell_blocked(world: &World, cell: CellCoord) -> bool {
        if world.occupancy.index(cell).is_none() || !world.occupancy.can_enter(cell) {
            return true;
        }

        #[cfg(any(test, feature = "tower_scaffolding"))]
        if world.tower_occupancy.contains(cell) {
            return true;
        }

        false
    }

    /// Identifies the tower occupying the provided cell, if any.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[must_use]
    pub fn tower_at(world: &World, cell: CellCoord) -> Option<TowerId> {
        if !world.tower_occupancy.contains(cell) {
            return None;
        }

        world
            .towers
            .iter()
            .find(|tower| tower_region_contains_cell(tower.region, cell))
            .map(|tower| tower.id)
    }

    /// Enumerates the wall target cells bugs should attempt to reach.
    #[must_use]
    pub fn target_cells(world: &World) -> Vec<CellCoord> {
        world.targets.clone()
    }

    /// Enumerates the bug spawners ringing the maze.
    #[must_use]
    pub fn bug_spawners(world: &World) -> Vec<CellCoord> {
        world.bug_spawners.iter().collect()
    }

    /// Read-only snapshot describing all bugs within the maze.
    #[derive(Clone, Debug)]
    pub struct BugView {
        snapshots: Vec<BugSnapshot>,
    }

    impl BugView {
        /// Iterator over the captured bug snapshots in deterministic order.
        pub fn iter(&self) -> impl Iterator<Item = &BugSnapshot> {
            self.snapshots.iter()
        }

        /// Consumes the view, yielding the underlying snapshots.
        pub fn into_vec(self) -> Vec<BugSnapshot> {
            self.snapshots
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

    /// Read-only snapshot describing all towers placed within the maze.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[derive(Clone, Debug)]
    pub struct TowerView {
        snapshots: Vec<TowerSnapshot>,
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    impl TowerView {
        /// Iterator over the captured tower snapshots in deterministic order.
        pub fn iter(&self) -> impl Iterator<Item = &TowerSnapshot> {
            self.snapshots.iter()
        }

        /// Consumes the view, yielding the underlying snapshots.
        pub fn into_vec(self) -> Vec<TowerSnapshot> {
            self.snapshots
        }
    }

    /// Immutable representation of a single tower's state used for queries.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct TowerSnapshot {
        /// Identifier allocated to the tower by the world.
        pub id: TowerId,
        /// Kind of tower that was constructed.
        pub kind: TowerKind,
        /// Region of cells occupied by the tower.
        pub region: CellRect,
    }

    /// Read-only view into the dense occupancy grid.
    #[derive(Clone, Copy, Debug)]
    pub struct OccupancyView<'a> {
        grid: &'a OccupancyGrid,
    }

    impl<'a> OccupancyView<'a> {
        /// Returns the bug occupying the provided cell, if any.
        #[must_use]
        pub fn occupant(&self, cell: CellCoord) -> Option<BugId> {
            self.grid
                .index(cell)
                .and_then(|index| self.grid.cells().get(index).copied().flatten())
        }

        /// Reports whether the cell is currently free for traversal.
        #[must_use]
        pub fn is_free(&self, cell: CellCoord) -> bool {
            self.grid.can_enter(cell)
        }

        /// Returns an iterator over all cells.
        pub fn iter(&self) -> impl Iterator<Item = Option<BugId>> + 'a {
            self.grid.cells().iter().copied()
        }

        /// Provides the dimensions of the underlying occupancy grid.
        #[must_use]
        pub fn dimensions(&self) -> (u32, u32) {
            self.grid.dimensions()
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_contains_cell(region: CellRect, cell: CellCoord) -> bool {
        let origin = region.origin();
        let size = region.size();
        let column = u64::from(cell.column());
        let row = u64::from(cell.row());
        let origin_column = u64::from(origin.column());
        let origin_row = u64::from(origin.row());
        let width = u64::from(size.width());
        let height = u64::from(size.height());

        column >= origin_column
            && column < origin_column.saturating_add(width)
            && row >= origin_row
            && row < origin_row.saturating_add(height)
    }
}

#[derive(Clone, Debug)]
struct Bug {
    id: BugId,
    cell: CellCoord,
    color: BugColor,
    accumulator: Duration,
}

impl Bug {
    fn new(id: BugId, cell: CellCoord, color: BugColor) -> Self {
        Self {
            id,
            cell,
            color,
            accumulator: Duration::ZERO,
        }
    }

    fn advance(&mut self, destination: CellCoord) {
        self.cell = destination;
    }

    fn ready_for_step(&self, step_quantum: Duration) -> bool {
        self.accumulator >= step_quantum
    }
}

#[derive(Clone, Debug)]
struct BugSpawnerRegistry {
    cells: BTreeSet<CellCoord>,
}

impl BugSpawnerRegistry {
    fn new() -> Self {
        Self {
            cells: BTreeSet::new(),
        }
    }

    fn assign_outer_rim(&mut self, columns: u32, rows: u32) {
        self.cells.clear();

        if columns == 0 || rows == 0 {
            return;
        }

        let last_column = columns.saturating_sub(1);
        let last_row = rows.saturating_sub(1);

        for column in 0..columns {
            let _ = self.cells.insert(CellCoord::new(column, 0));
            let _ = self.cells.insert(CellCoord::new(column, last_row));
        }

        for row in 0..rows {
            let _ = self.cells.insert(CellCoord::new(0, row));
            let _ = self.cells.insert(CellCoord::new(last_column, row));
        }
    }

    fn remove_bottom_row(&mut self, columns: u32, rows: u32) {
        if columns == 0 || rows == 0 {
            return;
        }

        let bottom_row = rows.saturating_sub(1);

        for column in 0..columns {
            let _ = self.cells.remove(&CellCoord::new(column, bottom_row));
        }
    }

    fn contains(&self, cell: CellCoord) -> bool {
        self.cells.contains(&cell)
    }

    fn iter(&self) -> impl Iterator<Item = CellCoord> + '_ {
        self.cells.iter().copied()
    }

    #[cfg(test)]
    fn cells(&self) -> &BTreeSet<CellCoord> {
        &self.cells
    }
}

#[derive(Clone, Copy, Debug)]
struct StepRequest {
    bug_id: BugId,
    direction: Direction,
}

#[derive(Debug)]
struct ReservationFrame {
    tick_index: u64,
    requests: Vec<StepRequest>,
}

impl ReservationFrame {
    fn new() -> Self {
        Self {
            tick_index: 0,
            requests: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.tick_index = 0;
        self.requests.clear();
    }

    fn queue(&mut self, tick_index: u64, request: StepRequest) {
        if self.tick_index != tick_index {
            self.tick_index = tick_index;
            self.requests.clear();
        }
        self.requests.push(request);
    }

    fn drain_sorted(&mut self) -> Vec<StepRequest> {
        self.requests.sort_by_key(|request| request.bug_id);
        self.requests.drain(..).collect()
    }
}

#[derive(Clone, Debug)]
struct OccupancyGrid {
    columns: u32,
    rows: u32,
    cells: Vec<Option<BugId>>,
}

impl OccupancyGrid {
    fn new(columns: u32, rows: u32) -> Self {
        let capacity_u64 = u64::from(columns) * u64::from(rows);
        let capacity = usize::try_from(capacity_u64).unwrap_or(0);
        Self {
            columns,
            rows,
            cells: vec![None; capacity],
        }
    }

    fn clear(&mut self) {
        self.cells.fill(None);
    }

    pub(crate) fn can_enter(&self, cell: CellCoord) -> bool {
        self.index(cell).map_or(true, |index| {
            self.cells.get(index).copied().unwrap_or(None).is_none()
        })
    }

    fn occupy(&mut self, bug_id: BugId, cell: CellCoord) {
        if let Some(index) = self.index(cell) {
            if let Some(slot) = self.cells.get_mut(index) {
                *slot = Some(bug_id);
            }
        }
    }

    fn vacate(&mut self, cell: CellCoord) {
        if let Some(index) = self.index(cell) {
            if let Some(slot) = self.cells.get_mut(index) {
                *slot = None;
            }
        }
    }

    pub(crate) fn index(&self, cell: CellCoord) -> Option<usize> {
        if cell.column() < self.columns && cell.row() < self.rows {
            let row = usize::try_from(cell.row()).ok()?;
            let column = usize::try_from(cell.column()).ok()?;
            let width = usize::try_from(self.columns).ok()?;
            Some(row * width + column)
        } else {
            None
        }
    }

    pub(crate) fn cells(&self) -> &[Option<BugId>] {
        &self.cells
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }
}

#[cfg(any(test, feature = "tower_scaffolding"))]
#[derive(Clone, Debug)]
struct BitGrid {
    columns: u32,
    rows: u32,
    words: Vec<u64>,
}

#[cfg(any(test, feature = "tower_scaffolding"))]
impl BitGrid {
    fn new(columns: u32, rows: u32) -> Self {
        let cell_count = u64::from(columns) * u64::from(rows);
        let word_count = if cell_count == 0 {
            0
        } else {
            ((cell_count - 1) / 64) + 1
        };
        let capacity = usize::try_from(word_count).unwrap_or(0);
        Self {
            columns,
            rows,
            words: vec![0; capacity],
        }
    }

    fn contains(&self, cell: CellCoord) -> bool {
        let Some((index, bit_offset)) = self.bit_position(cell) else {
            return false;
        };
        self.words
            .get(index)
            .map_or(false, |word| (*word & (1_u64 << bit_offset)) != 0)
    }

    fn set(&mut self, cell: CellCoord) {
        if let Some((index, bit_offset)) = self.bit_position(cell) {
            if let Some(word) = self.words.get_mut(index) {
                *word |= 1_u64 << bit_offset;
            }
        }
    }

    fn clear(&mut self, cell: CellCoord) {
        if let Some((index, bit_offset)) = self.bit_position(cell) {
            if let Some(word) = self.words.get_mut(index) {
                *word &= !(1_u64 << bit_offset);
            }
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }

    fn bit_position(&self, cell: CellCoord) -> Option<(usize, u32)> {
        if cell.column() >= self.columns || cell.row() >= self.rows {
            return None;
        }

        let width = usize::try_from(self.columns).ok()?;
        let row = usize::try_from(cell.row()).ok()?;
        let column = usize::try_from(cell.column()).ok()?;
        let offset = row.checked_mul(width)?.checked_add(column)?;
        let word_index = offset / 64;
        let bit_offset = u32::try_from(offset % 64).ok()?;
        Some((word_index, bit_offset))
    }
}

fn interior_cell_columns(columns: TileCoord, cells_per_tile: u32) -> u32 {
    columns.get().saturating_mul(cells_per_tile)
}

fn interior_cell_rows(rows: TileCoord, cells_per_tile: u32) -> u32 {
    rows.get().saturating_mul(cells_per_tile)
}

fn total_cell_columns(columns: TileCoord, cells_per_tile: u32) -> u32 {
    let interior = interior_cell_columns(columns, cells_per_tile);
    if interior == 0 {
        0
    } else {
        interior.saturating_add(SIDE_BORDER_CELL_LAYERS.saturating_mul(2))
    }
}

fn total_cell_rows(rows: TileCoord, cells_per_tile: u32) -> u32 {
    let interior = interior_cell_rows(rows, cells_per_tile);
    if interior == 0 {
        0
    } else {
        interior.saturating_add(TOP_BORDER_CELL_LAYERS)
    }
}

fn exit_row_for_tile_grid(rows: TileCoord, cells_per_tile: u32) -> u32 {
    total_cell_rows(rows, cells_per_tile)
}

fn exit_columns_for_tile_grid(columns: TileCoord, cells_per_tile: u32) -> Vec<u32> {
    let tile_columns = columns.get();
    if tile_columns == 0 || cells_per_tile == 0 {
        return Vec::new();
    }

    let center_tile = if tile_columns % 2 == 0 {
        tile_columns.saturating_sub(1) / 2
    } else {
        tile_columns / 2
    };
    let left_margin = SIDE_BORDER_CELL_LAYERS;
    let start_column = left_margin.saturating_add(center_tile.saturating_mul(cells_per_tile));

    (0..cells_per_tile)
        .map(|offset| start_column.saturating_add(offset))
        .collect()
}

fn target_cells(columns: TileCoord, rows: TileCoord, cells_per_tile: u32) -> Vec<TargetCell> {
    if columns.get() == 0 || rows.get() == 0 || cells_per_tile == 0 {
        return Vec::new();
    }

    let exit_row = exit_row_for_tile_grid(rows, cells_per_tile);
    exit_columns_for_tile_grid(columns, cells_per_tile)
        .into_iter()
        .map(|column| TargetCell::new(column, exit_row))
        .collect()
}

fn advance_cell(
    from: CellCoord,
    direction: Direction,
    columns: u32,
    rows: u32,
    target_columns: &[u32],
) -> Option<CellCoord> {
    match direction {
        Direction::North => {
            let next_row = from.row().checked_sub(1)?;
            Some(CellCoord::new(from.column(), next_row))
        }
        Direction::East => {
            let next_column = from.column().checked_add(1)?;
            if next_column < columns {
                Some(CellCoord::new(next_column, from.row()))
            } else {
                None
            }
        }
        Direction::South => {
            let next_row = from.row().checked_add(1)?;
            if next_row < rows {
                Some(CellCoord::new(from.column(), next_row))
            } else if next_row == rows
                && target_columns.iter().any(|column| *column == from.column())
            {
                Some(CellCoord::new(from.column(), rows))
            } else {
                None
            }
        }
        Direction::West => {
            let next_column = from.column().checked_sub(1)?;
            Some(CellCoord::new(next_column, from.row()))
        }
    }
}

fn target_cells_from_wall(wall: &Wall) -> Vec<CellCoord> {
    wall.target()
        .cells()
        .iter()
        .map(|cell| cell.cell())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{BugColor, Goal, PlayMode};
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        time::Duration,
    };

    fn expected_outer_rim(columns: u32, rows: u32) -> BTreeSet<CellCoord> {
        let mut cells = BTreeSet::new();

        if columns == 0 || rows == 0 {
            return cells;
        }

        let last_column = columns.saturating_sub(1);
        let last_row = rows.saturating_sub(1);

        for column in 0..columns {
            let _ = cells.insert(CellCoord::new(column, 0));
            let _ = cells.insert(CellCoord::new(column, last_row));
        }

        for row in 0..rows {
            let _ = cells.insert(CellCoord::new(0, row));
            let _ = cells.insert(CellCoord::new(last_column, row));
        }

        if columns > 0 && rows > 0 {
            let bottom_row = rows.saturating_sub(1);
            for column in 0..columns {
                let _ = cells.remove(&CellCoord::new(column, bottom_row));
            }
        }

        cells
    }

    #[test]
    fn world_defaults_to_attack_mode() {
        let world = World::new();

        assert_eq!(query::play_mode(&world), PlayMode::Attack);
    }

    #[test]
    fn world_starts_without_bugs() {
        let world = World::new();

        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn tower_occupancy_does_not_block_when_empty() {
        let world = World::new();
        let occupancy = query::occupancy_view(&world);
        let (columns, rows) = occupancy.dimensions();

        for column in 0..columns {
            for row in 0..rows {
                let cell = CellCoord::new(column, row);
                let expected = !occupancy.is_free(cell);
                assert_eq!(query::is_cell_blocked(&world, cell), expected);
            }
        }
    }

    #[test]
    fn placing_tower_requires_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();
        let origin = CellCoord::new(1, 1);

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::InvalidMode,
            }]
        );
        assert!(world.towers.is_empty());
        assert!(!world.tower_occupancy.contains(origin));
    }

    #[test]
    fn placing_tower_rejects_misaligned_origin() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: DEFAULT_GRID_COLUMNS,
                rows: DEFAULT_GRID_ROWS,
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 4,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(
            SIDE_BORDER_CELL_LAYERS.saturating_add(1),
            TOP_BORDER_CELL_LAYERS,
        );
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::Misaligned,
            }]
        );
        assert!(world.towers.is_empty());
    }

    #[test]
    fn placing_tower_rejects_out_of_bounds_origin() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let (columns, _) = world.tower_occupancy.dimensions();
        assert!(columns > 0);
        let origin = CellCoord::new(columns.saturating_sub(1), 0);

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::OutOfBounds,
            }]
        );
        assert!(world.towers.is_empty());
    }

    #[test]
    fn placing_tower_rejects_when_region_is_occupied() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlaced {
                tower: TowerId::new(0),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    first_origin,
                    super::footprint_for(TowerKind::Basic),
                ),
            }]
        );
        events.clear();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin: first_origin,
                reason: PlacementError::Occupied,
            }]
        );
        events.clear();

        let second_origin = CellCoord::new(6, 6);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlaced {
                tower: TowerId::new(1),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    second_origin,
                    super::footprint_for(TowerKind::Basic),
                ),
            }]
        );
    }

    #[test]
    fn placing_tower_sets_occupancy_bits() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(3, 4);
        let region = CellRect::from_origin_and_size(origin, super::footprint_for(TowerKind::Basic));
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlaced {
                tower: TowerId::new(0),
                kind: TowerKind::Basic,
                region,
            }]
        );

        for column_offset in 0..region.size().width() {
            for row_offset in 0..region.size().height() {
                let cell =
                    CellCoord::new(origin.column() + column_offset, origin.row() + row_offset);
                assert!(world.tower_occupancy.contains(cell));
            }
        }
    }

    #[test]
    fn removing_tower_requires_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(2, 3);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemovalRejected {
                tower: TowerId::new(0),
                reason: RemovalError::InvalidMode,
            }]
        );
        assert!(world.towers.get(TowerId::new(0)).is_some());
    }

    #[test]
    fn removing_missing_tower_reports_error() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(42),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemovalRejected {
                tower: TowerId::new(42),
                reason: RemovalError::MissingTower,
            }]
        );
    }

    #[test]
    fn removing_tower_clears_state_and_occupancy() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(4, 2);
        let region = CellRect::from_origin_and_size(origin, super::footprint_for(TowerKind::Basic));
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemoved {
                tower: TowerId::new(0),
                region,
            }]
        );
        assert!(world.towers.is_empty());

        for column_offset in 0..region.size().width() {
            for row_offset in 0..region.size().height() {
                let cell =
                    CellCoord::new(origin.column() + column_offset, origin.row() + row_offset);
                assert!(!world.tower_occupancy.contains(cell));
            }
        }
    }

    #[test]
    fn tower_query_reports_snapshots_in_identifier_order() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(6, 4);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );
        events.clear();

        let second_origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        let footprint = super::footprint_for(TowerKind::Basic);
        let first_region = CellRect::from_origin_and_size(first_origin, footprint);
        let second_region = CellRect::from_origin_and_size(second_origin, footprint);

        let snapshots = query::towers(&world).into_vec();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].id, TowerId::new(0));
        assert_eq!(snapshots[0].kind, TowerKind::Basic);
        assert_eq!(snapshots[0].region, first_region);
        assert_eq!(snapshots[1].id, TowerId::new(1));
        assert_eq!(snapshots[1].kind, TowerKind::Basic);
        assert_eq!(snapshots[1].region, second_region);
    }

    #[test]
    fn tower_at_reports_identifier_for_cells_inside_footprints() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(4, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );
        events.clear();

        let second_origin = CellCoord::new(8, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        let footprint = super::footprint_for(TowerKind::Basic);

        for column_offset in 0..footprint.width() {
            for row_offset in 0..footprint.height() {
                let first_cell = CellCoord::new(
                    first_origin.column() + column_offset,
                    first_origin.row() + row_offset,
                );
                assert_eq!(query::tower_at(&world, first_cell), Some(TowerId::new(0)));

                let second_cell = CellCoord::new(
                    second_origin.column() + column_offset,
                    second_origin.row() + row_offset,
                );
                assert_eq!(query::tower_at(&world, second_cell), Some(TowerId::new(1)));
            }
        }

        let outside_above =
            CellCoord::new(first_origin.column(), first_origin.row().saturating_sub(1));
        assert_eq!(query::tower_at(&world, outside_above), None);

        let outside_between = CellCoord::new(
            second_origin.column().saturating_sub(1),
            first_origin.row().saturating_add(footprint.height()),
        );
        assert_eq!(query::tower_at(&world, outside_between), None);

        assert_eq!(query::tower_at(&world, CellCoord::new(0, 0)), None);
    }

    #[test]
    fn entering_builder_mode_clears_bugs_and_occupancy() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            },
            &mut events,
        );
        assert!(!query::bug_view(&world).into_vec().is_empty());
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }]
        );
        assert_eq!(query::play_mode(&world), PlayMode::Builder);
        assert!(query::bug_view(&world).into_vec().is_empty());
        assert!(query::occupancy_view(&world)
            .iter()
            .all(|slot| slot.is_none()));
    }

    #[test]
    fn returning_to_attack_mode_preserves_empty_maze() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Attack,
            }]
        );
        assert_eq!(query::play_mode(&world), PlayMode::Attack);
        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn setting_same_play_mode_is_idempotent() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        assert!(events.is_empty());

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }]
        );

        events.clear();
        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn configure_tile_grid_respects_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert_eq!(query::play_mode(&world), PlayMode::Builder);
        assert!(query::bug_view(&world).into_vec().is_empty());
        assert!(query::occupancy_view(&world)
            .iter()
            .all(|slot| slot.is_none()));
    }

    #[test]
    fn tick_is_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let tick_before = world.tick_index;
        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(500),
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert_eq!(world.tick_index, tick_before);
        assert!(world.reservations.requests.is_empty());
    }

    #[test]
    fn step_bug_is_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::StepBug {
                bug_id: BugId::new(0),
                direction: Direction::North,
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert!(world.reservations.requests.is_empty());
    }

    #[test]
    fn bug_spawner_creates_bug_when_cell_free() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(1),
                rows: TileCoord::new(1),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        events.clear();
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x12, 0x34, 0x56),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::BugSpawned {
                bug_id: BugId::new(0),
                cell: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x12, 0x34, 0x56),
            }]
        );

        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.id, BugId::new(0));
        assert_eq!(snapshot.cell, CellCoord::new(0, 0));
        assert_eq!(snapshot.color, BugColor::from_rgb(0x12, 0x34, 0x56));
        assert_eq!(
            query::occupancy_view(&world).occupant(CellCoord::new(0, 0)),
            Some(BugId::new(0))
        );
    }

    #[test]
    fn bug_spawner_requires_free_cell() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(1),
                rows: TileCoord::new(1),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        events.clear();
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0xaa, 0xbb, 0xcc),
            },
            &mut events,
        );

        assert_eq!(events.len(), 1);

        events.clear();
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x10, 0x20, 0x30),
            },
            &mut events,
        );

        assert!(events.is_empty());
        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].color, BugColor::from_rgb(0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn bug_spawner_ignored_without_registration() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(3),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        world.clear_bugs();

        events.clear();
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(1, 1),
                color: BugColor::from_rgb(0xaa, 0x00, 0xff),
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn bug_spawner_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );

        events.clear();
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0, 0, 0),
            },
            &mut events,
        );

        assert!(events.is_empty());
    }

    #[test]
    fn bug_spawners_cover_outer_rim_in_default_world() {
        let world = World::new();
        let (columns, rows) = world.occupancy.dimensions();
        let expected = expected_outer_rim(columns, rows);

        assert_eq!(world.bug_spawners.cells(), &expected);
    }

    #[test]
    fn bug_spawners_rebuilt_after_configuring_grid() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(2),
                rows: TileCoord::new(3),
                tile_length: 50.0,
                cells_per_tile: 2,
            },
            &mut events,
        );

        let (columns, rows) = world.occupancy.dimensions();
        let expected_columns = total_cell_columns(TileCoord::new(2), 2);
        let expected_rows = total_cell_rows(TileCoord::new(3), 2);
        assert_eq!((columns, rows), (expected_columns, expected_rows));
        let expected = expected_outer_rim(columns, rows);

        assert_eq!(world.bug_spawners.cells(), &expected);
    }

    #[test]
    fn apply_configures_tile_grid() {
        let mut world = World::new();
        let mut events = Vec::new();

        let expected_columns = TileCoord::new(12);
        let expected_rows = TileCoord::new(8);
        let expected_tile_length = 75.0;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: expected_columns,
                rows: expected_rows,
                tile_length: expected_tile_length,
                cells_per_tile: 1,
            },
            &mut events,
        );

        let tile_grid = query::tile_grid(&world);

        assert_eq!(tile_grid.columns(), expected_columns);
        assert_eq!(tile_grid.rows(), expected_rows);
        assert_eq!(tile_grid.tile_length(), expected_tile_length);
        assert!(events.is_empty());
    }

    #[test]
    fn bugs_are_generated_within_configured_grid() {
        let mut world = World::new();
        let mut events = Vec::new();
        let columns = TileCoord::new(8);
        let rows = TileCoord::new(6);
        let cells_per_tile = 2;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns,
                rows,
                tile_length: 32.0,
                cells_per_tile,
            },
            &mut events,
        );

        let interior_start_column = 0;
        let interior_end_column =
            interior_start_column + columns.get().saturating_mul(cells_per_tile);
        let interior_start_row = 0;
        let interior_end_row = interior_start_row + rows.get().saturating_mul(cells_per_tile);

        for bug in query::bug_view(&world).iter() {
            assert!(bug.cell.column() >= interior_start_column);
            assert!(bug.cell.column() < interior_end_column);
            assert!(bug.cell.row() >= interior_start_row);
            assert!(bug.cell.row() < interior_end_row);
        }
        assert!(events.is_empty());
    }

    #[test]
    fn bug_generation_limits_to_available_cells() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(1),
                rows: TileCoord::new(1),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        let bugs = query::bug_view(&world).into_vec();
        assert!(bugs.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn bug_generation_is_deterministic_for_same_grid() {
        let mut first_world = World::new();
        let mut second_world = World::new();
        let mut first_events = Vec::new();
        let mut second_events = Vec::new();

        apply(
            &mut first_world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(9),
                tile_length: 50.0,
                cells_per_tile: 3,
            },
            &mut first_events,
        );

        apply(
            &mut second_world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(9),
                tile_length: 50.0,
                cells_per_tile: 3,
            },
            &mut second_events,
        );

        assert_eq!(
            query::bug_view(&first_world).into_vec(),
            query::bug_view(&second_world).into_vec()
        );
        assert_eq!(first_events, second_events);
    }

    #[test]
    fn target_aligns_with_center_for_odd_columns() {
        let mut world = World::new();
        let mut events = Vec::new();
        let cells_per_tile = 3;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(9),
                rows: TileCoord::new(7),
                tile_length: 64.0,
                cells_per_tile,
            },
            &mut events,
        );

        let target_cells = query::target(&world).cells();

        assert_eq!(
            target_cells.len(),
            usize::try_from(cells_per_tile).expect("cells_per_tile fits in usize")
        );
        let expected_row = exit_row_for_tile_grid(TileCoord::new(7), cells_per_tile);
        let expected_start =
            SIDE_BORDER_CELL_LAYERS.saturating_add(4_u32.saturating_mul(cells_per_tile));
        let expected_columns: Vec<u32> = (0..cells_per_tile)
            .map(|offset| expected_start + offset)
            .collect();
        let actual_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column()).collect();
        assert_eq!(actual_columns, expected_columns);
        assert!(target_cells.iter().all(|cell| cell.row() == expected_row));
        assert!(events.is_empty());
    }

    #[test]
    fn target_spans_single_tile_for_even_columns() {
        let mut world = World::new();
        let mut events = Vec::new();
        let cells_per_tile = 2;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
                cells_per_tile,
            },
            &mut events,
        );

        let target_cells = query::target(&world).cells();

        assert_eq!(
            target_cells.len(),
            usize::try_from(cells_per_tile).expect("cells_per_tile fits in usize")
        );
        let expected_row = exit_row_for_tile_grid(TileCoord::new(6), cells_per_tile);
        let expected_start =
            SIDE_BORDER_CELL_LAYERS.saturating_add(5_u32.saturating_mul(cells_per_tile));
        let expected_columns: Vec<u32> = (0..cells_per_tile)
            .map(|offset| expected_start + offset)
            .collect();
        let actual_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column()).collect();
        assert_eq!(actual_columns, expected_columns);
        assert!(target_cells.iter().all(|cell| cell.row() == expected_row));
        assert!(events.is_empty());
    }

    #[test]
    fn target_absent_when_grid_missing() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(0),
                rows: TileCoord::new(0),
                tile_length: 32.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        assert!(query::target(&world).cells().is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn goal_for_returns_nearest_target_cell() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(5),
                rows: TileCoord::new(4),
                tile_length: 1.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        assert!(events.is_empty());

        let goal = query::goal_for(&world, CellCoord::new(0, 0));
        let expected_columns = exit_columns_for_tile_grid(TileCoord::new(5), 1);
        let expected_column = *expected_columns
            .first()
            .expect("expected at least one target column");
        let expected = CellCoord::new(
            expected_column,
            exit_row_for_tile_grid(TileCoord::new(4), 1),
        );
        assert_eq!(goal, Some(Goal::at(expected)));
    }

    #[test]
    fn configure_bug_step_adjusts_quantum() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureBugStep {
                step_duration: Duration::from_millis(125),
            },
            &mut events,
        );

        assert!(events.is_empty());

        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(125),
            },
            &mut events,
        );

        assert!(events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. })));

        let bug_view = query::bug_view(&world);
        assert!(bug_view.iter().any(|bug| bug.ready_for_step));
    }

    #[test]
    fn tower_replay_is_deterministic() {
        let first = replay_tower_script(scripted_tower_commands());
        let second = replay_tower_script(scripted_tower_commands());

        assert_eq!(first, second, "tower replay diverged between runs");

        let fingerprint = first.fingerprint();
        let expected = 0x195a_71ab_29bc_3554;
        assert_eq!(
            fingerprint, expected,
            "tower replay fingerprint mismatch: {fingerprint:#x}"
        );
    }

    fn replay_tower_script(commands: Vec<Command>) -> ReplayOutcome {
        let mut world = World::new();
        let mut log = Vec::new();

        for command in commands {
            let mut events = Vec::new();
            apply(&mut world, command, &mut events);
            log.extend(events.into_iter().map(EventRecord::from));
        }

        let towers = query::towers(&world)
            .into_vec()
            .into_iter()
            .map(TowerRecord::from)
            .collect();

        ReplayOutcome {
            towers,
            events: log,
        }
    }

    fn scripted_tower_commands() -> Vec<Command> {
        vec![
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(1, 1),
            },
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            Command::ConfigureTileGrid {
                columns: TileCoord::new(6),
                rows: TileCoord::new(5),
                tile_length: 1.0,
                cells_per_tile: 4,
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(3, 2),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 20),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 4),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 4),
            },
            Command::RemoveTower {
                tower: TowerId::new(1),
            },
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(8, 6),
            },
        ]
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ReplayOutcome {
        towers: Vec<TowerRecord>,
        events: Vec<EventRecord>,
    }

    impl ReplayOutcome {
        fn fingerprint(&self) -> u64 {
            let mut hasher = DefaultHasher::new();
            self.hash(&mut hasher);
            hasher.finish()
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct TowerRecord {
        id: TowerId,
        kind: TowerKind,
        region: CellRect,
    }

    impl From<query::TowerSnapshot> for TowerRecord {
        fn from(snapshot: query::TowerSnapshot) -> Self {
            Self {
                id: snapshot.id,
                kind: snapshot.kind,
                region: snapshot.region,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    enum EventRecord {
        PlayModeChanged {
            mode: PlayMode,
        },
        TowerPlaced {
            tower: TowerId,
            kind: TowerKind,
            region: CellRect,
        },
        TowerRemoved {
            tower: TowerId,
            region: CellRect,
        },
        TowerPlacementRejected {
            kind: TowerKind,
            origin: CellCoord,
            reason: PlacementError,
        },
        TowerRemovalRejected {
            tower: TowerId,
            reason: RemovalError,
        },
    }

    impl From<Event> for EventRecord {
        fn from(event: Event) -> Self {
            match event {
                Event::PlayModeChanged { mode } => Self::PlayModeChanged { mode },
                Event::TowerPlaced {
                    tower,
                    kind,
                    region,
                } => Self::TowerPlaced {
                    tower,
                    kind,
                    region,
                },
                Event::TowerRemoved { tower, region } => Self::TowerRemoved { tower, region },
                Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason,
                } => Self::TowerPlacementRejected {
                    kind,
                    origin,
                    reason,
                },
                Event::TowerRemovalRejected { tower, reason } => {
                    Self::TowerRemovalRejected { tower, reason }
                }
                other => panic!("unexpected event emitted during tower replay: {other:?}"),
            }
        }
    }
}
