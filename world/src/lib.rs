#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

use std::{collections::VecDeque, time::Duration};

use maze_defence_core::{BugId, CellCoord, Command, Direction, Event, TileCoord, WELCOME_BANNER};

const BUG_GENERATION_SEED: u64 = 0x42f0_e1eb_d4a5_3c21;
const BUG_COUNT: usize = 4;

const DEFAULT_GRID_COLUMNS: TileCoord = TileCoord::new(10);
const DEFAULT_GRID_ROWS: TileCoord = TileCoord::new(10);
const DEFAULT_TILE_LENGTH: f32 = 100.0;

const STEP_QUANTUM: Duration = Duration::from_secs(1);

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
    hole: WallHole,
}

impl Wall {
    /// Creates a new wall aligned with the provided grid dimensions.
    #[must_use]
    pub(crate) fn new(columns: TileCoord, rows: TileCoord) -> Self {
        Self {
            hole: WallHole::aligned_with_grid(columns, rows),
        }
    }

    /// Retrieves the hole carved into the perimeter wall.
    #[must_use]
    pub fn hole(&self) -> &WallHole {
        &self.hole
    }
}

/// Opening carved into the wall to connect the maze with the outside world.
#[derive(Debug)]
pub struct WallHole {
    cells: Vec<WallCell>,
}

impl WallHole {
    fn aligned_with_grid(columns: TileCoord, rows: TileCoord) -> Self {
        Self {
            cells: hole_cells(columns, rows),
        }
    }

    /// Collection of cells that occupy the hole within the wall.
    #[must_use]
    pub fn cells(&self) -> &[WallCell] {
        &self.cells
    }
}

/// Discrete cell that composes part of the wall hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WallCell {
    column: TileCoord,
    row: TileCoord,
}

impl WallCell {
    /// Creates a new wall cell located at the provided column and row.
    #[must_use]
    pub const fn new(column: TileCoord, row: TileCoord) -> Self {
        Self { column, row }
    }

    /// Column that contains the cell relative to the tile grid.
    #[must_use]
    pub const fn column(&self) -> TileCoord {
        self.column
    }

    /// Row that contains the cell relative to the tile grid.
    #[must_use]
    pub const fn row(&self) -> TileCoord {
        self.row
    }
}

/// Represents the authoritative Maze Defence world state.
#[derive(Debug)]
pub struct World {
    banner: &'static str,
    tile_grid: TileGrid,
    wall: Wall,
    wall_targets: Vec<CellCoord>,
    bugs: Vec<Bug>,
    occupancy: OccupancyGrid,
    reservations: ReservationFrame,
    tick_index: u64,
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        let tile_grid = TileGrid::new(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS, DEFAULT_TILE_LENGTH);
        let wall = Wall::new(tile_grid.columns(), tile_grid.rows());
        let wall_targets = wall_target_cells(&wall);
        let mut world = Self {
            banner: WELCOME_BANNER,
            bugs: Vec::new(),
            occupancy: OccupancyGrid::new(tile_grid.columns().get(), tile_grid.rows().get()),
            reservations: ReservationFrame::new(),
            wall,
            wall_targets,
            tile_grid,
            tick_index: 0,
        };
        world.reset_bugs();
        world
    }

    fn reset_bugs(&mut self) {
        let generated = generate_bugs(self.tile_grid.columns(), self.tile_grid.rows());
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

    fn bug_mut(&mut self, bug_id: BugId) -> Option<&mut Bug> {
        self.bugs.iter_mut().find(|bug| bug.id == bug_id)
    }

    fn bug_index(&self, bug_id: BugId) -> Option<usize> {
        self.bugs.iter().position(|bug| bug.id == bug_id)
    }

    fn resolve_pending_steps(&mut self, out_events: &mut Vec<Event>) {
        let requests = self.reservations.drain_sorted();
        if requests.is_empty() {
            return;
        }

        for request in requests {
            let Some(index) = self.bug_index(request.bug_id) else {
                continue;
            };

            let (before, after) = self.bugs.split_at_mut(index);
            let bug = &mut after[0];
            let from = bug.cell;

            if bug.accumulator < STEP_QUANTUM {
                continue;
            }

            let Some(next_cell) = bug.next_step() else {
                if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                }
                continue;
            };

            let Some(expected_direction) = direction_between(from, next_cell) else {
                bug.clear_path();
                if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                }
                continue;
            };

            if expected_direction != request.direction {
                bug.clear_path();
                if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                }
                continue;
            }

            if !self.occupancy.can_enter(next_cell) {
                bug.clear_path();
                if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                }
                continue;
            }

            self.occupancy.vacate(from);
            self.occupancy.occupy(bug.id, next_cell);
            bug.advance(next_cell);
            bug.accumulator = bug.accumulator.saturating_sub(STEP_QUANTUM);
            out_events.push(Event::BugAdvanced {
                bug_id: bug.id,
                from,
                to: next_cell,
            });

            if bug.next_step().is_none() && bug.accumulator >= STEP_QUANTUM {
                if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                }
            }

            let _ = before;
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
        } => {
            world.tile_grid = TileGrid::new(columns, rows, tile_length);
            world.wall = Wall::new(columns, rows);
            world.wall_targets = wall_target_cells(&world.wall);
            world.occupancy = OccupancyGrid::new(columns.get(), rows.get());
            world.reset_bugs();

            for bug in world.bugs.iter_mut() {
                out_events.push(Event::BugPathNeeded { bug_id: bug.id });
            }
        }
        Command::Tick { dt } => {
            world.tick_index = world.tick_index.saturating_add(1);
            out_events.push(Event::TimeAdvanced { dt });

            for bug in world.iter_bugs_mut() {
                bug.accumulator = bug.accumulator.saturating_add(dt);
                if bug.accumulator >= STEP_QUANTUM && bug.next_step().is_none() {
                    if bug.mark_path_needed() {
                        out_events.push(Event::BugPathNeeded { bug_id: bug.id });
                    }
                }
            }
        }
        Command::SetBugPath { bug_id, path } => {
            let columns = world.tile_grid.columns();
            let rows = world.tile_grid.rows();
            if let Some(bug) = world.bug_mut(bug_id) {
                if bug.assign_path(path, columns, rows) {
                    if bug.next_step().is_some() {
                        bug.clear_path_needed();
                    } else if bug.mark_path_needed() {
                        out_events.push(Event::BugPathNeeded { bug_id });
                    }
                } else if bug.mark_path_needed() {
                    out_events.push(Event::BugPathNeeded { bug_id });
                }
            }
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

    use super::{OccupancyGrid, TileGrid, Wall, WallHole, World};
    use maze_defence_core::{BugId, CellCoord};

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

    /// Provides read-only access to the hole carved into the perimeter wall.
    #[must_use]
    pub fn wall_hole(world: &World) -> &WallHole {
        world.wall.hole()
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
                next_hop: bug.next_step(),
                ready_for_step: bug.ready_for_step(),
                needs_path: bug.path_needed,
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

    /// Enumerates the wall-hole cells that are currently unoccupied.
    #[must_use]
    pub fn available_wall_cells(world: &World) -> Vec<CellCoord> {
        world
            .wall_targets
            .iter()
            .copied()
            .filter(|cell| world.occupancy.can_enter(*cell))
            .collect()
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
        /// Head of the queued path, if any.
        pub next_hop: Option<CellCoord>,
        /// Indicates whether the bug accrued enough time to advance.
        pub ready_for_step: bool,
        /// Indicates whether the world awaits a new path for the bug.
        pub needs_path: bool,
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
    path: VecDeque<CellCoord>,
    accumulator: Duration,
    path_needed: bool,
}

impl Bug {
    fn from_seed(id: BugId, cell: CellCoord, color: BugColor) -> Self {
        Self {
            id,
            cell,
            color,
            path: VecDeque::new(),
            accumulator: Duration::ZERO,
            path_needed: true,
        }
    }

    fn assign_path(&mut self, path: Vec<CellCoord>, columns: TileCoord, rows: TileCoord) -> bool {
        let deque: VecDeque<CellCoord> = path.into();
        if let Some(first) = deque.front().copied() {
            let column_bound = columns.get();
            let row_bound = rows.get().saturating_add(1);
            if !is_valid_cell(first, column_bound, row_bound) {
                return false;
            }

            if direction_between(self.cell, first).is_none() {
                return false;
            }
        }

        self.path = deque;
        true
    }

    fn next_step(&self) -> Option<CellCoord> {
        self.path.front().copied()
    }

    fn advance(&mut self, destination: CellCoord) {
        let _ = self.path.pop_front();
        self.cell = destination;
    }

    fn mark_path_needed(&mut self) -> bool {
        let was_needed = self.path_needed;
        self.path_needed = true;
        !was_needed
    }

    fn clear_path_needed(&mut self) {
        self.path_needed = false;
    }

    fn ready_for_step(&self) -> bool {
        self.accumulator >= STEP_QUANTUM
    }

    fn clear_path(&mut self) {
        self.path.clear();
        self.path_needed = true;
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

fn is_valid_cell(cell: CellCoord, columns: u32, rows: u32) -> bool {
    cell.column() < columns && cell.row() < rows
}

fn hole_cells(columns: TileCoord, rows: TileCoord) -> Vec<WallCell> {
    let column_count = columns.get();
    let row_count = rows.get();

    if column_count == 0 || row_count == 0 {
        return Vec::new();
    }

    let center_column = if column_count % 2 == 0 {
        (column_count - 1) / 2
    } else {
        column_count / 2
    };

    vec![WallCell::new(
        TileCoord::new(center_column),
        TileCoord::new(row_count),
    )]
}

fn direction_between(from: CellCoord, to: CellCoord) -> Option<Direction> {
    let column_diff = from.column().abs_diff(to.column());
    let row_diff = from.row().abs_diff(to.row());

    if column_diff + row_diff != 1 {
        return None;
    }

    if column_diff == 1 {
        if to.column() > from.column() {
            Some(Direction::East)
        } else {
            Some(Direction::West)
        }
    } else if to.row() > from.row() {
        Some(Direction::South)
    } else {
        Some(Direction::North)
    }
}

fn wall_target_cells(wall: &Wall) -> Vec<CellCoord> {
    wall.hole()
        .cells()
        .iter()
        .map(|cell| CellCoord::new(cell.column().get(), cell.row().get()))
        .collect()
}

fn generate_bugs(columns: TileCoord, rows: TileCoord) -> Vec<BugSeed> {
    let column_count = columns.get();
    let row_count = rows.get();

    if column_count == 0 || row_count == 0 {
        return Vec::new();
    }

    let available_cells_u64 = u64::from(column_count) * u64::from(row_count);
    let available_cells = match usize::try_from(available_cells_u64) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    };
    let target_count = BUG_COUNT.min(available_cells);

    let mut bugs: Vec<BugSeed> = Vec::with_capacity(target_count);
    let mut rng_state = BUG_GENERATION_SEED;

    for index in 0..target_count {
        let color = BUG_COLORS[index % BUG_COLORS.len()];
        let bug_id = BugId::new(index as u32);

        loop {
            rng_state = next_random(rng_state);
            let column = (rng_state as u32) % column_count;
            rng_state = next_random(rng_state);
            let row = (rng_state as u32) % row_count;
            let cell = CellCoord::new(column, row);

            if bugs.iter().any(|bug| bug.cell == cell) {
                continue;
            }

            bugs.push(BugSeed {
                id: bug_id,
                cell,
                color,
            });
            break;
        }
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
            },
            &mut events,
        );

        let tile_grid = query::tile_grid(&world);

        assert_eq!(tile_grid.columns(), expected_columns);
        assert_eq!(tile_grid.rows(), expected_rows);
        assert_eq!(tile_grid.tile_length(), expected_tile_length);
        assert_eq!(events.len(), BUG_COUNT);
    }

    #[test]
    fn bugs_are_generated_within_configured_grid() {
        let mut world = World::new();
        let mut events = Vec::new();
        let columns = TileCoord::new(8);
        let rows = TileCoord::new(6);

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns,
                rows,
                tile_length: 32.0,
            },
            &mut events,
        );

        for bug in query::bug_view(&world).iter() {
            assert!(bug.cell.column() < columns.get());
            assert!(bug.cell.row() < rows.get());
        }
        let cell_capacity = columns.get() * rows.get();
        assert_eq!(events.len(), BUG_COUNT.min(cell_capacity as usize));
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
            },
            &mut events,
        );

        let bugs = query::bug_view(&world).into_vec();
        assert_eq!(bugs.len(), 1);
        let bug = bugs.first().expect("exactly one bug should be generated");
        assert_eq!(bug.cell.column(), 0);
        assert_eq!(bug.cell.row(), 0);
        assert_eq!(events.len(), 1);
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
            },
            &mut first_events,
        );

        apply(
            &mut second_world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(9),
                tile_length: 50.0,
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
    fn wall_hole_aligns_with_center_for_odd_columns() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(9),
                rows: TileCoord::new(7),
                tile_length: 64.0,
            },
            &mut events,
        );

        let hole_cells = query::wall_hole(&world).cells();

        assert_eq!(hole_cells.len(), 1);
        let cell = hole_cells[0];
        assert_eq!(cell.column().get(), 4);
        assert_eq!(cell.row().get(), 7);
        assert_eq!(events.len(), BUG_COUNT);
    }

    #[test]
    fn wall_hole_spans_single_tile_for_even_columns() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
            },
            &mut events,
        );

        let hole_cells = query::wall_hole(&world).cells();

        assert_eq!(hole_cells.len(), 1);
        let cell = hole_cells[0];
        assert_eq!(cell.column().get(), 5);
        assert_eq!(cell.row().get(), 6);
        assert_eq!(events.len(), BUG_COUNT);
    }

    #[test]
    fn wall_hole_absent_when_grid_missing() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(0),
                rows: TileCoord::new(0),
                tile_length: 32.0,
            },
            &mut events,
        );

        assert!(query::wall_hole(&world).cells().is_empty());
        assert!(events.is_empty());
    }
}
