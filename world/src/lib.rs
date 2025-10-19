#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

use maze_defence_core::{Command, WELCOME_BANNER};

const BUG_GENERATION_SEED: u64 = 0x42f0_e1eb_d4a5_3c21;
const BUG_COUNT: usize = 4;

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
    pub(crate) const fn new(columns: u32, rows: u32, tile_length: f32) -> Self {
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
    bugs: Vec<Bug>,
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        let tile_grid = TileGrid::new(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS, DEFAULT_TILE_LENGTH);

        Self {
            banner: WELCOME_BANNER,
            bugs: generate_bugs(tile_grid.columns, tile_grid.rows),
            tile_grid,
        }
    }
}

/// Applies the provided command to the world, mutating state deterministically.
pub fn apply(world: &mut World, command: Command) {
    match command {
        Command::ConfigureTileGrid {
            columns,
            rows,
            tile_length,
        } => {
            world.tile_grid = TileGrid::new(columns, rows, tile_length);
            world.bugs = generate_bugs(columns, rows);
        }
    }
}

/// Query functions that provide read-only access to the world state.
pub mod query {
    use super::{Bug, TileGrid, World};

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

    /// Provides read-only access to the bugs currently inhabiting the maze.
    #[must_use]
    pub fn bugs(world: &World) -> &[Bug] {
        &world.bugs
    }
}

/// Unique identifier assigned to a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridCell {
    column: u32,
    row: u32,
}

impl GridCell {
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

/// Immutable description of a single maze bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bug {
    id: BugId,
    cell: GridCell,
    color: BugColor,
}

impl Bug {
    /// Creates a new bug with the provided attributes.
    #[must_use]
    pub const fn new(id: BugId, cell: GridCell, color: BugColor) -> Self {
        Self { id, cell, color }
    }

    /// Unique identifier assigned to the bug.
    #[must_use]
    pub const fn id(&self) -> BugId {
        self.id
    }

    /// Grid cell currently occupied by the bug.
    #[must_use]
    pub const fn cell(&self) -> GridCell {
        self.cell
    }

    /// Color describing the bug's appearance.
    #[must_use]
    pub const fn color(&self) -> BugColor {
        self.color
    }
}

fn generate_bugs(columns: u32, rows: u32) -> Vec<Bug> {
    if columns == 0 || rows == 0 {
        return Vec::new();
    }

    let available_cells_u64 = u64::from(columns) * u64::from(rows);
    let available_cells = match usize::try_from(available_cells_u64) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    };
    let target_count = BUG_COUNT.min(available_cells);

    let mut bugs: Vec<Bug> = Vec::with_capacity(target_count);
    let mut rng_state = BUG_GENERATION_SEED;

    for index in 0..target_count {
        let color = BUG_COLORS[index % BUG_COLORS.len()];
        let bug_id = BugId::new(index as u32);

        loop {
            rng_state = next_random(rng_state);
            let column = (rng_state as u32) % columns;
            rng_state = next_random(rng_state);
            let row = (rng_state as u32) % rows;
            let cell = GridCell::new(column, row);

            if bugs.iter().any(|bug| bug.cell == cell) {
                continue;
            }

            bugs.push(Bug::new(bug_id, cell, color));
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

        let expected_columns = 12;
        let expected_rows = 8;
        let expected_tile_length = 75.0;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: expected_columns,
                rows: expected_rows,
                tile_length: expected_tile_length,
            },
        );

        let tile_grid = query::tile_grid(&world);

        assert_eq!(tile_grid.columns(), expected_columns);
        assert_eq!(tile_grid.rows(), expected_rows);
        assert_eq!(tile_grid.tile_length(), expected_tile_length);
    }

    #[test]
    fn bugs_are_generated_within_configured_grid() {
        let mut world = World::new();
        let columns = 8;
        let rows = 6;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns,
                rows,
                tile_length: 32.0,
            },
        );

        for bug in query::bugs(&world) {
            assert!(bug.cell().column() < columns);
            assert!(bug.cell().row() < rows);
        }
    }

    #[test]
    fn bug_generation_limits_to_available_cells() {
        let mut world = World::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: 1,
                rows: 1,
                tile_length: 25.0,
            },
        );

        let bugs = query::bugs(&world);
        assert_eq!(bugs.len(), 1);
        let bug = bugs.first().expect("exactly one bug should be generated");
        assert_eq!(bug.cell().column(), 0);
        assert_eq!(bug.cell().row(), 0);
    }

    #[test]
    fn bug_generation_is_deterministic_for_same_grid() {
        let mut first_world = World::new();
        let mut second_world = World::new();

        apply(
            &mut first_world,
            Command::ConfigureTileGrid {
                columns: 12,
                rows: 9,
                tile_length: 50.0,
            },
        );

        apply(
            &mut second_world,
            Command::ConfigureTileGrid {
                columns: 12,
                rows: 9,
                tile_length: 50.0,
            },
        );

        assert_eq!(query::bugs(&first_world), query::bugs(&second_world));
    }
}
