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
use maze_defence_core::{BugColor, Command, Event, PlayMode, TileCoord};
use maze_defence_rendering::{
    BugPresentation, Color, FrameInput, PlacementPreview, Presentation, RenderingBackend, Scene,
    TargetCellPresentation, TargetPresentation, TileGridPresentation, TileSpacePosition,
    WallPresentation,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_BUG_STEP_MS: u64 = 250;
const DEFAULT_BUG_SPAWN_INTERVAL_MS: u64 = 1_000;
const SPAWN_RNG_SEED: u64 = 0x4d59_5df4_d0f3_3173;
const SPAWN_RNG_MULTIPLIER: u64 = 636_413_622_384_679_3005;
const SPAWN_RNG_INCREMENT: u64 = 1;
const SPAWN_COLORS: [BugColor; 4] = [
    BugColor::from_rgb(0x2f, 0x95, 0x32),
    BugColor::from_rgb(0xc8, 0x2a, 0x36),
    BugColor::from_rgb(0xff, 0xc1, 0x07),
    BugColor::from_rgb(0x58, 0x47, 0xff),
];

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
    /// Milliseconds between automatic bug spawns while in attack mode.
    #[arg(
        long = "bug-spawn-interval-ms",
        value_name = "MILLISECONDS",
        default_value_t = DEFAULT_BUG_SPAWN_INTERVAL_MS,
        value_parser = clap::value_parser!(u64).range(1..=60_000)
    )]
    bug_spawn_interval_ms: u64,
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
    let bug_spawn_interval = Duration::from_millis(args.bug_spawn_interval_ms);
    let mut simulation = Simulation::new(
        columns,
        rows,
        DEFAULT_TILE_LENGTH,
        args.cells_per_tile,
        bug_step_duration,
        bug_spawn_interval,
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

    let mut scene = Scene::new(
        grid_scene,
        wall_scene,
        Vec::new(),
        query::play_mode(simulation.world()),
        None,
    );
    simulation.populate_scene(&mut scene);

    let presentation = Presentation::new(banner, Color::from_rgb_u8(85, 142, 52), scene);

    MacroquadBackend::default().run(presentation, move |dt, input, scene| {
        simulation.handle_input(input);
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
    queued_commands: Vec<Command>,
    pending_input: FrameInput,
    bug_spawn_interval: Duration,
    bug_spawn_accumulator: Duration,
    spawn_rng_state: u64,
    spawn_color_index: usize,
}

impl Simulation {
    fn new(
        columns: u32,
        rows: u32,
        tile_length: f32,
        cells_per_tile: u32,
        bug_step: Duration,
        bug_spawn_interval: Duration,
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
            queued_commands: Vec::new(),
            pending_input: FrameInput::default(),
            bug_spawn_interval,
            bug_spawn_accumulator: Duration::ZERO,
            spawn_rng_state: SPAWN_RNG_SEED,
            spawn_color_index: 0,
        };
        simulation.process_pending_events();
        simulation
    }

    fn world(&self) -> &World {
        &self.world
    }

    fn handle_input(&mut self, input: FrameInput) {
        if input.mode_toggle {
            let current_mode = query::play_mode(&self.world);
            let next_mode = match current_mode {
                PlayMode::Attack => PlayMode::Builder,
                PlayMode::Builder => PlayMode::Attack,
            };
            self.queued_commands
                .push(Command::SetPlayMode { mode: next_mode });
        }

        self.pending_input = FrameInput {
            mode_toggle: false,
            cursor_world_space: input.cursor_world_space,
            cursor_tile_space: input.cursor_tile_space,
        };
    }

    fn advance(&mut self, dt: Duration) {
        self.pending_events.clear();
        self.flush_queued_commands();

        let play_mode = query::play_mode(&self.world);

        if !dt.is_zero() {
            world::apply(
                &mut self.world,
                Command::Tick { dt },
                &mut self.pending_events,
            );
        }

        let spawn_attempts = self.update_spawn_timer(play_mode, dt);
        for _ in 0..spawn_attempts {
            if let Some(command) = self.random_spawn_command() {
                world::apply(&mut self.world, command, &mut self.pending_events);
            }
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

        scene.play_mode = query::play_mode(&self.world);
        scene.placement_preview = if scene.play_mode == PlayMode::Builder {
            self.pending_input
                .cursor_tile_space
                .map(|tile| PlacementPreview::new(tile, 1))
        } else {
            None
        };
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

    fn flush_queued_commands(&mut self) {
        if self.queued_commands.is_empty() {
            return;
        }

        for command in self.queued_commands.drain(..) {
            world::apply(&mut self.world, command, &mut self.pending_events);
        }
    }

    fn update_spawn_timer(&mut self, play_mode: PlayMode, dt: Duration) -> usize {
        if play_mode != PlayMode::Attack {
            self.bug_spawn_accumulator = Duration::ZERO;
            return 0;
        }

        if self.bug_spawn_interval.is_zero() {
            return 0;
        }

        self.bug_spawn_accumulator = self.bug_spawn_accumulator.saturating_add(dt);
        let mut spawn_count = 0;
        while self.bug_spawn_accumulator >= self.bug_spawn_interval {
            self.bug_spawn_accumulator -= self.bug_spawn_interval;
            spawn_count += 1;
        }
        spawn_count
    }

    fn random_spawn_command(&mut self) -> Option<Command> {
        let spawners = query::bug_spawners(&self.world);
        if spawners.is_empty() {
            return None;
        }

        let rng_value = self.advance_rng();
        let index = (rng_value % spawners.len() as u64) as usize;
        let spawner = spawners[index];
        let color = self.next_spawn_color();
        Some(Command::SpawnBug { spawner, color })
    }

    fn advance_rng(&mut self) -> u64 {
        self.spawn_rng_state = self
            .spawn_rng_state
            .wrapping_mul(SPAWN_RNG_MULTIPLIER)
            .wrapping_add(SPAWN_RNG_INCREMENT);
        self.spawn_rng_state
    }

    fn next_spawn_color(&mut self) -> BugColor {
        let color = SPAWN_COLORS[self.spawn_color_index % SPAWN_COLORS.len()];
        self.spawn_color_index = (self.spawn_color_index + 1) % SPAWN_COLORS.len();
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    fn new_simulation() -> Simulation {
        Simulation::new(
            4,
            3,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Duration::from_millis(200),
            Duration::from_secs(1),
        )
    }

    fn make_scene() -> Scene {
        let tile_grid = TileGridPresentation::new(
            4,
            3,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(30, 30, 30),
        )
        .expect("valid grid dimensions");
        let wall = WallPresentation::new(
            12.0,
            Color::from_rgb_u8(60, 45, 30),
            TargetPresentation::new(Vec::new()),
        );

        Scene::new(tile_grid, wall, Vec::new(), PlayMode::Attack, None)
    }

    #[test]
    fn handle_input_toggles_mode_and_caches_cursor() {
        let mut simulation = new_simulation();
        let first_tile = TileSpacePosition::from_indices(1, 2);
        let first_world = Vec2::new(12.5, 24.0);

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            cursor_world_space: Some(first_world),
            cursor_tile_space: Some(first_tile),
        });

        assert_eq!(
            simulation.queued_commands,
            vec![Command::SetPlayMode {
                mode: PlayMode::Builder,
            }]
        );
        assert!(!simulation.pending_input.mode_toggle);
        assert_eq!(
            simulation.pending_input.cursor_world_space,
            Some(first_world)
        );
        assert_eq!(simulation.pending_input.cursor_tile_space, Some(first_tile));

        let second_tile = TileSpacePosition::from_indices(2, 1);
        let second_world = Vec2::new(48.0, 16.0);
        simulation.handle_input(FrameInput {
            mode_toggle: false,
            cursor_world_space: Some(second_world),
            cursor_tile_space: Some(second_tile),
        });

        assert_eq!(
            simulation.queued_commands,
            vec![Command::SetPlayMode {
                mode: PlayMode::Builder,
            }]
        );
        assert_eq!(
            simulation.pending_input.cursor_world_space,
            Some(second_world)
        );
        assert_eq!(
            simulation.pending_input.cursor_tile_space,
            Some(second_tile)
        );
    }

    #[test]
    fn populate_scene_projects_cached_preview_in_builder_mode() {
        let mut simulation = new_simulation();
        let initial_tile = TileSpacePosition::from_indices(0, 1);
        simulation.handle_input(FrameInput {
            mode_toggle: true,
            cursor_world_space: Some(Vec2::new(16.0, 48.0)),
            cursor_tile_space: Some(initial_tile),
        });

        simulation.advance(Duration::ZERO);
        assert!(simulation.queued_commands.is_empty());

        let preview_tile = TileSpacePosition::from_indices(3, 2);
        simulation.handle_input(FrameInput {
            mode_toggle: false,
            cursor_world_space: Some(Vec2::new(96.0, 64.0)),
            cursor_tile_space: Some(preview_tile),
        });

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.play_mode, PlayMode::Builder);
        assert_eq!(
            scene.placement_preview,
            Some(PlacementPreview::new(preview_tile, 1))
        );
    }

    #[test]
    fn advance_spawns_bug_after_interval() {
        let mut simulation = new_simulation();

        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.advance(Duration::from_millis(500));
        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.advance(Duration::from_millis(500));
        assert_eq!(query::bug_view(simulation.world()).into_vec().len(), 1);
    }

    #[test]
    fn builder_mode_pauses_spawning() {
        let mut simulation = new_simulation();

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        simulation.advance(Duration::from_secs(2));
        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        simulation.advance(Duration::from_secs(1));

        assert_eq!(query::bug_view(simulation.world()).into_vec().len(), 1);
    }
}
