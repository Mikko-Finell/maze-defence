#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

use std::str::FromStr;

use anyhow::Result;
use clap::Parser;
use maze_defence_core::Command;
use maze_defence_rendering::{
    BugPresentation, Color, Presentation, RenderingBackend, Scene, TileGridPresentation,
    WallHoleCellPresentation, WallHolePresentation, WallPresentation,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_world::{self as world, World};

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;

/// Command-line arguments for launching the Maze Defence experience.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Tile grid dimensions expressed as WIDTHxHEIGHT (for example 12x18).
    #[arg(short = 's', long = "size", value_name = "WIDTHxHEIGHT", conflicts_with_all = ["width", "height"])]
    grid_size: Option<GridSizeArg>,
    /// Number of columns in the tile grid when using explicit dimensions.
    #[arg(long, value_name = "COLUMNS", requires = "height")]
    width: Option<u32>,
    /// Number of rows in the tile grid when using explicit dimensions.
    #[arg(long, value_name = "ROWS", requires = "width")]
    height: Option<u32>,
    /// Thickness of the surrounding wall measured in pixels.
    #[arg(long, value_name = "PIXELS", default_value_t = 40.0)]
    wall_thickness: f32,
    /// Number of subcells drawn along each tile edge when rendering.
    #[arg(
        long = "cells-per-tile",
        value_name = "COUNT",
        default_value_t = TileGridPresentation::DEFAULT_SUBDIVISIONS_PER_TILE,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    cells_per_tile: u32,
}

/// Grid dimensions parsed from a WIDTHxHEIGHT command-line argument.
#[derive(Clone, Copy, Debug)]
struct GridSizeArg {
    columns: u32,
    rows: u32,
}

impl GridSizeArg {
    /// Converts the parsed grid size into discrete dimensions.
    #[must_use]
    fn into_dimensions(self) -> (u32, u32) {
        (self.columns, self.rows)
    }
}

impl FromStr for GridSizeArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (columns, rows) = value
            .split_once(['x', 'X'])
            .ok_or_else(|| "expected format WIDTHxHEIGHT".to_string())?;

        let columns = columns
            .trim()
            .parse::<u32>()
            .map_err(|error| format!("invalid width: {error}"))?;
        let rows = rows
            .trim()
            .parse::<u32>()
            .map_err(|error| format!("invalid height: {error}"))?;

        if columns == 0 || rows == 0 {
            return Err("grid dimensions must be positive".to_string());
        }

        Ok(Self { columns, rows })
    }
}

/// Entry point for the Maze Defence command-line interface.
fn main() -> Result<()> {
    let args = CliArgs::parse();

    let (columns, rows) = if let Some(size) = args.grid_size {
        size.into_dimensions()
    } else if let (Some(width), Some(height)) = (args.width, args.height) {
        (width, height)
    } else {
        (DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS)
    };

    let mut world = World::new();
    let mut _events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns,
            rows,
            tile_length: DEFAULT_TILE_LENGTH,
        },
        &mut _events,
    );
    let bootstrap = Bootstrap::default();
    let banner = bootstrap.welcome_banner(&world);

    let tile_grid = bootstrap.tile_grid(&world);
    let bug_view = bootstrap.bugs(&world);
    let wall_hole = bootstrap.wall_hole(&world);

    let grid_scene = TileGridPresentation::new(
        tile_grid.columns(),
        tile_grid.rows(),
        tile_grid.tile_length(),
        args.cells_per_tile,
        Color::from_rgb_u8(31, 54, 22),
    )?;

    let wall_hole_cells: Vec<WallHoleCellPresentation> = wall_hole
        .cells()
        .iter()
        .map(|cell| WallHoleCellPresentation::new(cell.column(), cell.row()))
        .collect();

    let wall_scene = WallPresentation::new(
        args.wall_thickness,
        Color::from_rgb_u8(68, 45, 15),
        WallHolePresentation::new(wall_hole_cells),
    );

    let bug_presentations: Vec<BugPresentation> = bug_view
        .iter()
        .map(|bug| {
            let cell = bug.cell;
            let color = bug.color;
            BugPresentation::new(
                cell.column(),
                cell.row(),
                Color::from_rgb_u8(color.red(), color.green(), color.blue()),
            )
        })
        .collect();

    let scene = Scene::new(grid_scene, wall_scene, bug_presentations);

    let presentation = Presentation::new(banner.to_owned(), Color::from_rgb_u8(85, 142, 52), scene);

    MacroquadBackend::default().run(presentation)
}
