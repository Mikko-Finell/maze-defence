#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

use std::{str::FromStr, time::Duration};

use anyhow::Result;
use clap::Parser;
use maze_defence_core::{Command, Event, TileCoord};
use maze_defence_rendering::{
    BugPresentation, Color, Presentation, RenderingBackend, Scene, TargetCellPresentation,
    TargetPresentation, TileGridPresentation, WallPresentation,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_BUG_STEP_MS: u64 = 250;

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
    /// Number of cells drawn along each tile edge when rendering.
    #[arg(
        long = "cells-per-tile",
        value_name = "COUNT",
        default_value_t = TileGridPresentation::DEFAULT_CELLS_PER_TILE,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    cells_per_tile: u32,
    /// Milliseconds each bug waits between steps. Smaller values make bugs move faster.
    #[arg(
        long = "bug-step-ms",
        value_name = "MILLISECONDS",
        default_value_t = DEFAULT_BUG_STEP_MS,
        value_parser = clap::value_parser!(u64).range(1..=60_000)
    )]
    bug_step_ms: u64,
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

    let bug_step_duration = Duration::from_millis(args.bug_step_ms);
    let mut simulation = Simulation::new(
        columns,
        rows,
        DEFAULT_TILE_LENGTH,
        args.cells_per_tile,
        bug_step_duration,
    );
    let bootstrap = Bootstrap::default();
    let (banner, grid_scene, wall_scene) = {
        let world = simulation.world();
        let banner = bootstrap.welcome_banner(world).to_owned();
        let tile_grid = bootstrap.tile_grid(world);
        let target = bootstrap.target(world);
        let grid_scene = TileGridPresentation::new(
            tile_grid.columns().get(),
            tile_grid.rows().get(),
            tile_grid.tile_length(),
            args.cells_per_tile,
            Color::from_rgb_u8(31, 54, 22),
        )?;
        let target_cells: Vec<TargetCellPresentation> = target
            .cells()
            .iter()
            .map(|cell| TargetCellPresentation::new(cell.column(), cell.row()))
            .collect();
        let wall_scene = WallPresentation::new(
            args.wall_thickness,
            Color::from_rgb_u8(68, 45, 15),
            TargetPresentation::new(target_cells),
        );
        (banner, grid_scene, wall_scene)
    };

    let mut scene = Scene::new(grid_scene, wall_scene, Vec::new());
    simulation.populate_scene(&mut scene);

    let presentation = Presentation::new(banner, Color::from_rgb_u8(85, 142, 52), scene);

    MacroquadBackend::default().run(presentation, move |dt, scene| {
        simulation.advance(dt);
        simulation.populate_scene(scene);
    })
}

#[derive(Debug)]
struct Simulation {
    world: World,
    movement: Movement,
    pending_events: Vec<Event>,
    scratch_commands: Vec<Command>,
}

impl Simulation {
    fn new(
        columns: u32,
        rows: u32,
        tile_length: f32,
        cells_per_tile: u32,
        bug_step: Duration,
    ) -> Self {
        let mut world = World::new();
        let mut pending_events = Vec::new();
        world::apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(columns),
                rows: TileCoord::new(rows),
                tile_length,
                cells_per_tile,
            },
            &mut pending_events,
        );
        world::apply(
            &mut world,
            Command::ConfigureBugStep {
                step_duration: bug_step,
            },
            &mut pending_events,
        );

        let mut simulation = Self {
            world,
            movement: Movement::default(),
            pending_events,
            scratch_commands: Vec::new(),
        };
        simulation.process_pending_events();
        simulation
    }

    fn world(&self) -> &World {
        &self.world
    }

    fn advance(&mut self, dt: Duration) {
        if !dt.is_zero() {
            self.pending_events.clear();
            world::apply(
                &mut self.world,
                Command::Tick { dt },
                &mut self.pending_events,
            );
        }
        self.process_pending_events();
    }

    fn populate_scene(&self, scene: &mut Scene) {
        let bug_view = query::bug_view(&self.world);
        scene.bugs.clear();
        scene.bugs.extend(bug_view.iter().map(|bug| {
            let cell = bug.cell;
            let color = bug.color;
            BugPresentation::new(
                cell.column(),
                cell.row(),
                Color::from_rgb_u8(color.red(), color.green(), color.blue()),
            )
        }));
    }

    fn process_pending_events(&mut self) {
        let mut events = std::mem::take(&mut self.pending_events);
        loop {
            if events.is_empty() {
                break;
            }

            let bug_view = query::bug_view(&self.world);
            let occupancy_view = query::occupancy_view(&self.world);
            let target_cells = query::target_cells(&self.world);
            self.scratch_commands.clear();
            self.movement.handle(
                &events,
                &bug_view,
                occupancy_view,
                &target_cells,
                &mut self.scratch_commands,
            );

            if self.scratch_commands.is_empty() {
                break;
            }

            events.clear();
            for command in self.scratch_commands.drain(..) {
                world::apply(&mut self.world, command, &mut events);
            }
        }

        self.pending_events = events;
        self.pending_events.clear();
    }
}
