#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

use std::{num::NonZeroU32, time::Duration};

use maze_defence_core::{BugId, CellCoord, Command, Direction, Event, TileCoord, WELCOME_BANNER};

const BUG_GENERATION_SEED: u64 = 0x42f0_e1eb_d4a5_3c21;
const BUG_COUNT: usize = 20;

const DEFAULT_GRID_COLUMNS: TileCoord = TileCoord::new(10);
const DEFAULT_GRID_ROWS: TileCoord = TileCoord::new(10);
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_CELLS_PER_TILE: u32 = 1;

const DEFAULT_STEP_QUANTUM: Duration = Duration::from_millis(250);
const MIN_STEP_QUANTUM: Duration = Duration::from_micros(1);

/// Describes the discrete tile layout of the world.
#[derive(Debug)]
pub struct TileGrid {
    columns: TileCoord,
    rows: TileCoord,
    tile_length: f32,
    cells_per_tile: NonZeroU32,
}

impl TileGrid {
    const SIDE_BORDER_CELL_LAYERS: u32 = 1;
    const TOP_BORDER_CELL_LAYERS: u32 = 1;
    const BOTTOM_BORDER_CELL_LAYERS: u32 = 0;

    /// Creates a new tile grid description.
    #[must_use]
    pub(crate) fn new(
        columns: TileCoord,
        rows: TileCoord,
        tile_length: f32,
        cells_per_tile: NonZeroU32,
    ) -> Self {
        Self {
            columns,
            rows,
            tile_length,
            cells_per_tile,
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

    /// Number of navigation cells carved along each tile edge.
    #[must_use]
    pub const fn cells_per_tile(&self) -> u32 {
        self.cells_per_tile.get()
    }

    fn interior_cell_columns(&self) -> u32 {
        self.columns.get().saturating_mul(self.cells_per_tile())
    }

    fn interior_cell_rows(&self) -> u32 {
        self.rows.get().saturating_mul(self.cells_per_tile())
    }

    fn interior_origin_column(&self) -> u32 {
        Self::SIDE_BORDER_CELL_LAYERS
    }

    fn interior_origin_row(&self) -> u32 {
        Self::TOP_BORDER_CELL_LAYERS
    }

    fn total_cell_columns(&self) -> u32 {
        let interior = self.interior_cell_columns();
        if interior == 0 {
            0
        } else {
            interior.saturating_add(Self::SIDE_BORDER_CELL_LAYERS.saturating_mul(2))
        }
    }

    fn total_cell_rows(&self) -> u32 {
        let interior = self.interior_cell_rows();
        if interior == 0 {
            0
        } else {
            interior.saturating_add(Self::TOP_BORDER_CELL_LAYERS + Self::BOTTOM_BORDER_CELL_LAYERS)
        }
    }

    fn exit_row(&self) -> u32 {
        self.interior_origin_row()
            .saturating_add(self.interior_cell_rows())
    }

    fn exit_column_range(&self) -> Option<(u32, u32)> {
        let tile_columns = self.columns.get();
        if tile_columns == 0 || self.rows.get() == 0 {
            return None;
        }

        let center_tile = if tile_columns % 2 == 0 {
            (tile_columns - 1) / 2
        } else {
            tile_columns / 2
        };
        let cells_per_tile = self.cells_per_tile();
        let start = self
            .interior_origin_column()
            .saturating_add(center_tile.saturating_mul(cells_per_tile));
        let end = start.saturating_add(cells_per_tile);
        Some((start, end))
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
    pub(crate) fn new(grid: &TileGrid) -> Self {
        Self {
            target: Target::aligned_with_grid(grid),
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
    fn aligned_with_grid(grid: &TileGrid) -> Self {
        Self {
            cells: target_cells(grid),
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

    /// Returns the target cell as a concrete [`CellCoord`].
    #[must_use]
    pub const fn as_cell(&self) -> CellCoord {
        self.cell
    }
}

/// Represents the authoritative Maze Defence world state.
#[derive(Debug)]
pub struct World {
    banner: &'static str,
    tile_grid: TileGrid,
    wall: Wall,
    targets: Vec<CellCoord>,
    bugs: Vec<Bug>,
    occupancy: OccupancyGrid,
    reservations: ReservationFrame,
    tick_index: u64,
    step_quantum: Duration,
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        let cells_per_tile = NonZeroU32::new(DEFAULT_CELLS_PER_TILE)
            .expect("default cell count must remain non-zero");
        let tile_grid = TileGrid::new(
            DEFAULT_GRID_COLUMNS,
            DEFAULT_GRID_ROWS,
            DEFAULT_TILE_LENGTH,
            cells_per_tile,
        );
        let wall = Wall::new(&tile_grid);
        let targets = target_cells_from_wall(&wall);
        let mut world = Self {
            banner: WELCOME_BANNER,
            bugs: Vec::new(),
            occupancy: OccupancyGrid::new(
                tile_grid.total_cell_columns(),
                tile_grid.total_cell_rows(),
            ),
            reservations: ReservationFrame::new(),
            wall,
            targets,
            tile_grid,
            tick_index: 0,
            step_quantum: DEFAULT_STEP_QUANTUM,
        };
        world.reset_bugs();
        world
    }

    fn reset_bugs(&mut self) {
        let generated = generate_bugs(&self.tile_grid);
        self.bugs = generated
            .into_iter()
            .map(|seed| Bug::from_seed(seed.id, seed.cell, seed.color))
            .collect();
        self.occupancy.fill_with(&self.bugs);
        self.reservations.clear();
    }

    fn iter_bugs_mut(&mut self) -> impl Iterator<Item = &mut Bug> {
        self.bugs.iter_mut()
    }

    fn bug_index(&self, bug_id: BugId) -> Option<usize> {
        self.bugs.iter().position(|bug| bug.id == bug_id)
    }

    fn resolve_pending_steps(&mut self, out_events: &mut Vec<Event>) {
        let requests = self.reservations.drain_sorted();
        if requests.is_empty() {
            return;
        }

        let mut exited_bugs: Vec<BugId> = Vec::new();
        let columns = self.tile_grid.total_cell_columns();
        let rows = self.tile_grid.total_cell_rows();
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
            world.tile_grid = TileGrid::new(columns, rows, tile_length, cells_per_tile);
            world.wall = Wall::new(&world.tile_grid);
            world.targets = target_cells_from_wall(&world.wall);
            world.occupancy = OccupancyGrid::new(
                world.tile_grid.total_cell_columns(),
                world.tile_grid.total_cell_rows(),
            );
            world.reset_bugs();
        }
        Command::Tick { dt } => {
            world.tick_index = world.tick_index.saturating_add(1);
            out_events.push(Event::TimeAdvanced { dt });

            for bug in world.iter_bugs_mut() {
                bug.accumulator = bug.accumulator.saturating_add(dt);
            }
        }
        Command::ConfigureBugStep { step_duration } => {
            let clamped = step_duration.max(MIN_STEP_QUANTUM);
            world.step_quantum = clamped;
        }
        Command::StepBug { bug_id, direction } => {
            world
                .reservations
                .queue(world.tick_index, StepRequest { bug_id, direction });
            world.resolve_pending_steps(out_events);
        }
    }
}

/// Query functions that provide read-only access to the world state.
pub mod query {
    use std::time::Duration;

    use super::{OccupancyGrid, Target, TileGrid, Wall, World};
    use maze_defence_core::{select_goal, BugId, CellCoord, Goal};

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

    /// Enumerates the wall target cells bugs should attempt to reach.
    #[must_use]
    pub fn target_cells(world: &World) -> Vec<CellCoord> {
        world.targets.clone()
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
        pub color: super::BugColor,
        /// Indicates whether the bug accrued enough time to advance.
        pub ready_for_step: bool,
        /// Duration accumulated toward the next step.
        pub accumulated: Duration,
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
}

/// Visual appearance applied to a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug)]
struct Bug {
    id: BugId,
    cell: CellCoord,
    color: BugColor,
    accumulator: Duration,
}

impl Bug {
    fn from_seed(id: BugId, cell: CellCoord, color: BugColor) -> Self {
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

#[derive(Clone, Copy, Debug)]
struct BugSeed {
    id: BugId,
    cell: CellCoord,
    color: BugColor,
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

    fn fill_with(&mut self, bugs: &[Bug]) {
        self.cells.fill(None);
        for bug in bugs {
            if let Some(index) = self.index(bug.cell) {
                self.cells[index] = Some(bug.id);
            }
        }
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

fn target_cells(grid: &TileGrid) -> Vec<TargetCell> {
    if grid.interior_cell_columns() == 0 || grid.interior_cell_rows() == 0 {
        return Vec::new();
    }

    let Some((start_column, end_column)) = grid.exit_column_range() else {
        return Vec::new();
    };
    if start_column >= end_column {
        return Vec::new();
    }

    let exit_row = grid.exit_row();
    let mut cells = Vec::new();
    for column in start_column..end_column {
        cells.push(TargetCell::new(column, exit_row));
    }
    cells
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
        .map(TargetCell::as_cell)
        .collect()
}

fn generate_bugs(tile_grid: &TileGrid) -> Vec<BugSeed> {
    let column_count = tile_grid.interior_cell_columns();
    let row_count = tile_grid.interior_cell_rows();

    if column_count == 0 || row_count == 0 {
        return Vec::new();
    }

    let available_cells_u64 = u64::from(column_count) * u64::from(row_count);
    let available_cells = match usize::try_from(available_cells_u64) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    };
    let target_capacity = available_cells.saturating_sub(1);
    let target_count = BUG_COUNT.min(target_capacity);

    let mut cells: Vec<CellCoord> = if available_cells == usize::MAX {
        Vec::new()
    } else {
        Vec::with_capacity(available_cells)
    };
    let column_offset = tile_grid.interior_origin_column();
    let row_offset = tile_grid.interior_origin_row();
    for row in 0..row_count {
        for column in 0..column_count {
            let absolute_column = column_offset.saturating_add(column);
            let absolute_row = row_offset.saturating_add(row);
            cells.push(CellCoord::new(absolute_column, absolute_row));
        }
    }

    let mut rng_state = BUG_GENERATION_SEED;
    for index in (1..cells.len()).rev() {
        rng_state = next_random(rng_state);
        let swap_index = (rng_state % (index as u64 + 1)) as usize;
        cells.swap(index, swap_index);
    }

    let mut bugs: Vec<BugSeed> = if available_cells == usize::MAX {
        Vec::new()
    } else {
        Vec::with_capacity(target_count)
    };
    for (index, cell) in cells.into_iter().take(target_count).enumerate() {
        let color = BUG_COLORS[index % BUG_COLORS.len()];
        let bug_id = BugId::new(index as u32);
        bugs.push(BugSeed {
            id: bug_id,
            cell,
            color,
        });
    }

    bugs
}

const BUG_COLORS: [BugColor; 4] = [
    BugColor::from_rgb(0x2f, 0x95, 0x32),
    BugColor::from_rgb(0xc8, 0x2a, 0x36),
    BugColor::from_rgb(0xff, 0xc1, 0x07),
    BugColor::from_rgb(0x58, 0x47, 0xff),
];

fn next_random(state: u64) -> u64 {
    state.wrapping_mul(636_413_622_384_679_3005).wrapping_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::Goal;
    use std::num::NonZeroU32;

    #[test]
    fn apply_configures_tile_grid() {
        let mut world = World::new();
        let mut events = Vec::new();

        let expected_columns = TileCoord::new(12);
        let expected_rows = TileCoord::new(8);
        let expected_tile_length = 75.0;
        let expected_cells_per_tile = NonZeroU32::new(3).unwrap();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: expected_columns,
                rows: expected_rows,
                tile_length: expected_tile_length,
                cells_per_tile: expected_cells_per_tile,
            },
            &mut events,
        );

        let tile_grid = query::tile_grid(&world);

        assert_eq!(tile_grid.columns(), expected_columns);
        assert_eq!(tile_grid.rows(), expected_rows);
        assert_eq!(tile_grid.tile_length(), expected_tile_length);
        assert_eq!(tile_grid.cells_per_tile(), expected_cells_per_tile.get());
        assert!(events.is_empty());
    }

    #[test]
    fn bugs_are_generated_within_configured_grid() {
        let mut world = World::new();
        let mut events = Vec::new();
        let columns = TileCoord::new(8);
        let rows = TileCoord::new(6);
        let cells_per_tile = NonZeroU32::new(2).unwrap();

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

        let left_border = TileGrid::SIDE_BORDER_CELL_LAYERS;
        let top_border = TileGrid::TOP_BORDER_CELL_LAYERS;
        let playable_columns = columns
            .get()
            .saturating_mul(cells_per_tile.get())
            .saturating_add(left_border);
        let playable_rows = rows
            .get()
            .saturating_mul(cells_per_tile.get())
            .saturating_add(top_border);

        for bug in query::bug_view(&world).iter() {
            assert!(bug.cell.column() >= left_border);
            assert!(bug.cell.column() < playable_columns);
            assert!(bug.cell.row() >= top_border);
            assert!(bug.cell.row() < playable_rows);
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
                cells_per_tile: NonZeroU32::new(4).unwrap(),
            },
            &mut events,
        );

        let bugs = query::bug_view(&world).into_vec();
        let grid = query::tile_grid(&world);
        let interior_columns = grid.columns().get().saturating_mul(grid.cells_per_tile());
        let interior_rows = grid.rows().get().saturating_mul(grid.cells_per_tile());
        let available_cells = u64::from(interior_columns) * u64::from(interior_rows);
        let max_bugs = usize::try_from(available_cells.saturating_sub(1)).unwrap_or(usize::MAX);
        let expected = BUG_COUNT.min(max_bugs);
        assert_eq!(bugs.len(), expected);
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
                cells_per_tile: NonZeroU32::new(2).unwrap(),
            },
            &mut first_events,
        );

        apply(
            &mut second_world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(9),
                tile_length: 50.0,
                cells_per_tile: NonZeroU32::new(2).unwrap(),
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

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(9),
                rows: TileCoord::new(7),
                tile_length: 64.0,
                cells_per_tile: NonZeroU32::new(3).unwrap(),
            },
            &mut events,
        );

        let grid = query::tile_grid(&world);
        let target_cells = query::target(&world).cells();
        let expected_count = usize::try_from(grid.cells_per_tile()).unwrap();
        assert_eq!(target_cells.len(), expected_count);
        let center_tile = grid.columns().get() / 2;
        let start_column = TileGrid::SIDE_BORDER_CELL_LAYERS
            .saturating_add(center_tile.saturating_mul(grid.cells_per_tile()));
        let exit_row = TileGrid::TOP_BORDER_CELL_LAYERS
            .saturating_add(grid.rows().get().saturating_mul(grid.cells_per_tile()));

        for (offset, cell) in target_cells.iter().enumerate() {
            assert_eq!(cell.column(), start_column.saturating_add(offset as u32));
            assert_eq!(cell.row(), exit_row);
        }
        assert!(events.is_empty());
    }

    #[test]
    fn target_spans_single_tile_for_even_columns() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
                cells_per_tile: NonZeroU32::new(4).unwrap(),
            },
            &mut events,
        );

        let grid = query::tile_grid(&world);
        let target_cells = query::target(&world).cells();
        let expected_count = usize::try_from(grid.cells_per_tile()).unwrap();
        assert_eq!(target_cells.len(), expected_count);
        let center_tile = (grid.columns().get() - 1) / 2;
        let start_column = TileGrid::SIDE_BORDER_CELL_LAYERS
            .saturating_add(center_tile.saturating_mul(grid.cells_per_tile()));
        let exit_row = TileGrid::TOP_BORDER_CELL_LAYERS
            .saturating_add(grid.rows().get().saturating_mul(grid.cells_per_tile()));

        for (offset, cell) in target_cells.iter().enumerate() {
            assert_eq!(cell.column(), start_column.saturating_add(offset as u32));
            assert_eq!(cell.row(), exit_row);
        }
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
                cells_per_tile: NonZeroU32::new(3).unwrap(),
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
                cells_per_tile: NonZeroU32::new(2).unwrap(),
            },
            &mut events,
        );

        assert!(events.is_empty());

        let goal = query::goal_for(&world, CellCoord::new(0, 0));
        let expected = CellCoord::new(5, 9);
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
}
