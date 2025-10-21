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
use glam::Vec2;
use maze_defence_core::{
    CellCoord, CellRect, CellRectSize, Command, Event, PlacementError, PlayMode, RemovalError,
    TileCoord, TowerId, TowerKind,
};
use maze_defence_rendering::{
    BugPresentation, Color, FrameInput, Presentation, RenderingBackend, Scene, SceneTower,
    TargetCellPresentation, TargetPresentation, TileGridPresentation, TileSpacePosition,
    TowerInteractionFeedback, TowerPreview, TowerTargetLine, WallPresentation,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_system_builder::{
    Builder as TowerBuilder, BuilderInput as TowerBuilderInput,
    PlacementPreview as BuilderPlacementPreview,
};
use maze_defence_system_movement::Movement;
use maze_defence_system_spawning::{Config as SpawningConfig, Spawning};
use maze_defence_system_tower_targeting::{TowerTarget, TowerTargeting};
use maze_defence_world::{self as world, query, World};

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_BUG_STEP_MS: u64 = 250;
const DEFAULT_BUG_SPAWN_INTERVAL_MS: u64 = 1_000;
const SPAWN_RNG_SEED: u64 = 0x4d59_5df4_d0f3_3173;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlacementRejection {
    kind: TowerKind,
    origin: CellCoord,
    reason: PlacementError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemovalRejection {
    tower: TowerId,
    reason: RemovalError,
}

/// Populates the scene with targeting beams derived from system DTOs.
pub fn push_tower_targets(scene: &mut Scene, targets: &[TowerTarget]) {
    scene.tower_targets.clear();
    scene.tower_targets.reserve(targets.len());
    for target in targets {
        let from = Vec2::new(
            target.tower_center_cells.column,
            target.tower_center_cells.row,
        );
        let to = Vec2::new(target.bug_center_cells.column, target.bug_center_cells.row);
        scene
            .tower_targets
            .push(TowerTargetLine::new(target.tower, target.bug, from, to));
    }
}

/// Command-line arguments for launching the Maze Defence experience.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
// Developers: changing CLI arguments requires updating the README's command-line documentation.
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
    let bootstrap = Bootstrap;
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
        Vec::new(),
        Vec::new(),
        query::play_mode(simulation.world()),
        None,
        None,
        None,
    );
    simulation.populate_scene(&mut scene);

    let presentation = Presentation::new(banner, Color::from_rgb_u8(85, 142, 52), scene);

    MacroquadBackend.run(presentation, move |dt, input, scene| {
        simulation.handle_input(input);
        simulation.advance(dt);
        simulation.populate_scene(scene);
    })
}

#[derive(Debug)]
struct Simulation {
    world: World,
    builder: TowerBuilder,
    movement: Movement,
    spawning: Spawning,
    tower_targeting: TowerTargeting,
    current_targets: Vec<TowerTarget>,
    pending_events: Vec<Event>,
    scratch_commands: Vec<Command>,
    queued_commands: Vec<Command>,
    pending_input: FrameInput,
    builder_preview: Option<BuilderPlacementPreview>,
    tower_feedback: Option<TowerInteractionFeedback>,
    last_placement_rejection: Option<PlacementRejection>,
    last_removal_rejection: Option<RemovalRejection>,
    cells_per_tile: u32,
    #[cfg(test)]
    last_frame_events: Vec<Event>,
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
            builder: TowerBuilder::default(),
            movement: Movement::default(),
            spawning: Spawning::new(SpawningConfig::new(bug_spawn_interval, SPAWN_RNG_SEED)),
            tower_targeting: TowerTargeting::new(),
            current_targets: Vec::new(),
            pending_events,
            scratch_commands: Vec::new(),
            queued_commands: Vec::new(),
            pending_input: FrameInput::default(),
            builder_preview: None,
            tower_feedback: None,
            last_placement_rejection: None,
            last_removal_rejection: None,
            cells_per_tile,
            #[cfg(test)]
            last_frame_events: Vec::new(),
        };
        simulation.process_pending_events(None, TowerBuilderInput::default());
        simulation.builder_preview = simulation.compute_builder_preview();
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
            confirm_action: input.confirm_action,
            remove_action: input.remove_action,
        };
    }

    fn advance(&mut self, dt: Duration) {
        let builder_preview = self.compute_builder_preview();
        let builder_input = self.prepare_builder_input();

        self.pending_events.clear();
        self.flush_queued_commands();

        if !dt.is_zero() {
            world::apply(
                &mut self.world,
                Command::Tick { dt },
                &mut self.pending_events,
            );
        }

        self.process_pending_events(builder_preview, builder_input);
        self.builder_preview = self.compute_builder_preview();
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

        let tower_view = query::towers(&self.world);
        scene.towers.clear();
        scene.towers.extend(
            tower_view
                .iter()
                .map(|tower| SceneTower::new(tower.id, tower.kind, tower.region)),
        );

        push_tower_targets(scene, &self.current_targets);

        scene.play_mode = query::play_mode(&self.world);
        scene.tower_preview = if scene.play_mode == PlayMode::Builder {
            self.builder_preview().map(|preview| {
                TowerPreview::new(
                    preview.kind,
                    preview.region,
                    preview.placeable,
                    preview.rejection,
                )
            })
        } else {
            None
        };
        scene.active_tower_footprint_tiles = if scene.play_mode == PlayMode::Builder {
            Some(self.selected_tower_footprint_tiles())
        } else {
            None
        };
        scene.tower_feedback = self.tower_feedback;
    }

    fn process_pending_events(
        &mut self,
        mut builder_preview: Option<BuilderPlacementPreview>,
        mut builder_input: TowerBuilderInput,
    ) {
        let mut events = std::mem::take(&mut self.pending_events);
        let mut next_events = Vec::new();

        #[cfg(test)]
        {
            self.last_frame_events.clear();
        }

        let mut ran_iteration = false;

        loop {
            if events.is_empty() {
                if next_events.is_empty() && ran_iteration {
                    break;
                }
                events = std::mem::take(&mut next_events);
            }

            ran_iteration = true;

            #[cfg(test)]
            {
                self.last_frame_events.extend(events.iter().cloned());
            }

            self.record_tower_feedback(&events);

            let play_mode = query::play_mode(&self.world);
            let spawners = query::bug_spawners(&self.world);
            self.scratch_commands.clear();
            self.spawning
                .handle(&events, play_mode, &spawners, &mut self.scratch_commands);
            for command in self.scratch_commands.drain(..) {
                world::apply(&mut self.world, command, &mut next_events);
            }

            {
                let bug_view = query::bug_view(&self.world);
                let occupancy_view = query::occupancy_view(&self.world);
                let target_cells = query::target_cells(&self.world);
                self.scratch_commands.clear();
                self.movement.handle(
                    &events,
                    &bug_view,
                    occupancy_view,
                    &target_cells,
                    |cell| query::is_cell_blocked(&self.world, cell),
                    &mut self.scratch_commands,
                );
            }
            for command in self.scratch_commands.drain(..) {
                world::apply(&mut self.world, command, &mut next_events);
            }

            self.refresh_tower_targets(play_mode);

            self.scratch_commands.clear();
            let mut tower_at = |cell| query::tower_at(&self.world, cell);
            let preview_for_frame = builder_preview;
            let input_for_frame = builder_input;
            self.builder.handle(
                &events,
                preview_for_frame,
                input_for_frame,
                &mut tower_at,
                &mut self.scratch_commands,
            );
            builder_preview = None;
            builder_input = TowerBuilderInput::default();
            for command in self.scratch_commands.drain(..) {
                world::apply(&mut self.world, command, &mut next_events);
            }

            events.clear();
        }

        self.pending_events = events;
        self.pending_events.clear();

        #[cfg(test)]
        {
            self.last_frame_events.extend(next_events.iter().cloned());
        }
    }

    fn record_tower_feedback(&mut self, events: &[Event]) {
        for event in events {
            match event {
                Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason,
                } => {
                    let rejection = PlacementRejection {
                        kind: *kind,
                        origin: *origin,
                        reason: *reason,
                    };
                    self.last_placement_rejection = Some(rejection);
                    self.tower_feedback = Some(TowerInteractionFeedback::PlacementRejected {
                        kind: *kind,
                        origin: *origin,
                        reason: *reason,
                    });
                }
                Event::TowerRemovalRejected { tower, reason } => {
                    let rejection = RemovalRejection {
                        tower: *tower,
                        reason: *reason,
                    };
                    self.last_removal_rejection = Some(rejection);
                    self.tower_feedback = Some(TowerInteractionFeedback::RemovalRejected {
                        tower: *tower,
                        reason: *reason,
                    });
                }
                Event::TowerPlaced { .. } => {
                    self.last_placement_rejection = None;
                    if matches!(
                        self.tower_feedback,
                        Some(TowerInteractionFeedback::PlacementRejected { .. })
                    ) {
                        self.tower_feedback = None;
                    }
                }
                Event::TowerRemoved { .. } => {
                    self.last_removal_rejection = None;
                    if matches!(
                        self.tower_feedback,
                        Some(TowerInteractionFeedback::RemovalRejected { .. })
                    ) {
                        self.tower_feedback = None;
                    }
                }
                _ => {}
            }
        }
    }

    fn refresh_tower_targets(&mut self, play_mode: PlayMode) {
        if play_mode != PlayMode::Attack {
            if !self.current_targets.is_empty() {
                self.current_targets.clear();
            }
            return;
        }

        let towers = query::towers(&self.world);
        let bugs = query::bug_view(&self.world);
        let cells_per_tile = query::cells_per_tile(&self.world);
        self.tower_targeting.handle(
            play_mode,
            &towers,
            &bugs,
            cells_per_tile,
            &mut self.current_targets,
        );
    }

    fn prepare_builder_input(&mut self) -> TowerBuilderInput {
        let cursor_cell = self
            .pending_input
            .cursor_tile_space
            .map(|tile| self.tile_position_to_cell(tile));
        let input = TowerBuilderInput::new(
            self.pending_input.confirm_action,
            self.pending_input.remove_action,
            cursor_cell,
        );
        self.pending_input.confirm_action = false;
        self.pending_input.remove_action = false;
        input
    }

    fn compute_builder_preview(&self) -> Option<BuilderPlacementPreview> {
        let tile_position = self.pending_input.cursor_tile_space?;
        let origin = self.tile_position_to_cell(tile_position);
        let kind = self.selected_tower_kind();
        let footprint = Self::tower_footprint(kind);
        let region = CellRect::from_origin_and_size(origin, footprint);
        let mut placeable = self.region_is_placeable(region);
        let rejection = self.last_placement_rejection.and_then(|rejection| {
            if rejection.kind == kind && rejection.origin == origin {
                Some(rejection.reason)
            } else {
                None
            }
        });
        if rejection.is_some() {
            placeable = false;
        }
        Some(BuilderPlacementPreview::new(
            kind, origin, region, placeable, rejection,
        ))
    }

    fn selected_tower_kind(&self) -> TowerKind {
        TowerKind::Basic
    }

    fn tower_footprint(kind: TowerKind) -> CellRectSize {
        match kind {
            TowerKind::Basic => CellRectSize::new(4, 4),
        }
    }

    fn selected_tower_footprint_tiles(&self) -> Vec2 {
        if self.cells_per_tile == 0 {
            return Vec2::ZERO;
        }

        let footprint = Self::tower_footprint(self.selected_tower_kind());
        Vec2::new(
            footprint.width() as f32 / self.cells_per_tile as f32,
            footprint.height() as f32 / self.cells_per_tile as f32,
        )
    }

    fn tile_position_to_cell(&self, position: TileSpacePosition) -> CellCoord {
        let cells_per_tile = self.cells_per_tile.max(1);
        let column_cells = (position.column_in_tiles() * cells_per_tile as f32).round() as u32;
        let row_cells = (position.row_in_tiles() * cells_per_tile as f32).round() as u32;
        let column = TileGridPresentation::SIDE_BORDER_CELL_LAYERS.saturating_add(column_cells);
        let row = TileGridPresentation::TOP_BORDER_CELL_LAYERS.saturating_add(row_cells);
        CellCoord::new(column, row)
    }

    fn region_is_placeable(&self, region: CellRect) -> bool {
        let size = region.size();
        if size.width() == 0 || size.height() == 0 {
            return false;
        }

        let origin = region.origin();
        for column_offset in 0..size.width() {
            let Some(column) = origin.column().checked_add(column_offset) else {
                return false;
            };
            for row_offset in 0..size.height() {
                let Some(row) = origin.row().checked_add(row_offset) else {
                    return false;
                };
                let cell = CellCoord::new(column, row);
                if query::is_cell_blocked(&self.world, cell) {
                    return false;
                }
            }
        }

        true
    }

    fn builder_preview(&self) -> Option<BuilderPlacementPreview> {
        self.builder_preview
    }

    #[cfg(test)]
    fn last_frame_events(&self) -> &[Event] {
        &self.last_frame_events
    }

    #[cfg(test)]
    fn tower_feedback(&self) -> Option<TowerInteractionFeedback> {
        self.tower_feedback
    }

    #[cfg(test)]
    fn current_targets(&self) -> &[TowerTarget] {
        &self.current_targets
    }

    fn flush_queued_commands(&mut self) {
        if self.queued_commands.is_empty() {
            return;
        }

        for command in self.queued_commands.drain(..) {
            world::apply(&mut self.world, command, &mut self.pending_events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::BugColor;

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

        Scene::new(
            tile_grid,
            wall,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            PlayMode::Attack,
            None,
            None,
            None,
        )
    }

    fn enter_builder_mode(simulation: &mut Simulation) {
        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
    }

    fn squared_distance_to_center(cell: CellCoord, center: (u32, u32)) -> u64 {
        let dx = cell.column().abs_diff(center.0);
        let dy = cell.row().abs_diff(center.1);
        u64::from(dx) * u64::from(dx) + u64::from(dy) * u64::from(dy)
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
            confirm_action: false,
            remove_action: false,
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
            confirm_action: false,
            remove_action: false,
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
            confirm_action: false,
            remove_action: false,
        });

        simulation.advance(Duration::ZERO);
        assert!(simulation.queued_commands.is_empty());

        let preview_tile = TileSpacePosition::from_indices(3, 2);
        simulation.handle_input(FrameInput {
            mode_toggle: false,
            cursor_world_space: Some(Vec2::new(96.0, 64.0)),
            cursor_tile_space: Some(preview_tile),
            confirm_action: false,
            remove_action: false,
        });

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.play_mode, PlayMode::Builder);
        let expected_preview = simulation
            .builder_preview()
            .expect("builder preview available in builder mode");
        assert_eq!(
            scene.tower_preview,
            Some(TowerPreview::new(
                expected_preview.kind,
                expected_preview.region,
                expected_preview.placeable,
                expected_preview.rejection,
            ))
        );
        let footprint = Simulation::tower_footprint(simulation.selected_tower_kind());
        let expected_footprint = Vec2::new(
            footprint.width() as f32 / simulation.cells_per_tile as f32,
            footprint.height() as f32 / simulation.cells_per_tile as f32,
        );
        assert_eq!(scene.active_tower_footprint_tiles, Some(expected_footprint));
        assert!(scene.towers.is_empty());
        assert_eq!(scene.tower_feedback, simulation.tower_feedback());
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

    #[test]
    fn builder_preview_marks_region_unplaceable_when_occupied() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        let preview_tile = TileSpacePosition::from_indices(1, 1);
        let preview_world = Vec2::new(64.0, 64.0);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);

        let preview = simulation
            .builder_preview()
            .expect("builder preview available in builder mode");
        assert!(preview.placeable, "initial preview should be placeable");
        assert_eq!(preview.kind, TowerKind::Basic);
        let cells_per_tile = TileGridPresentation::DEFAULT_CELLS_PER_TILE;
        let expected_origin = CellCoord::new(
            TileGridPresentation::SIDE_BORDER_CELL_LAYERS.saturating_add(
                (preview_tile.column_in_tiles() * cells_per_tile as f32).round() as u32,
            ),
            TileGridPresentation::TOP_BORDER_CELL_LAYERS.saturating_add(
                (preview_tile.row_in_tiles() * cells_per_tile as f32).round() as u32,
            ),
        );
        assert_eq!(preview.origin, expected_origin);
        assert_eq!(
            preview.region,
            CellRect::from_origin_and_size(preview.origin, CellRectSize::new(4, 4))
        );

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            confirm_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));
        assert_eq!(
            query::towers(simulation.world()).into_vec().len(),
            1,
            "tower placement should succeed"
        );

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        let updated_preview = simulation
            .builder_preview()
            .expect("preview should remain available");
        assert!(
            !updated_preview.placeable,
            "occupied region should be marked unplaceable"
        );
    }

    #[test]
    fn placement_rejection_updates_preview_and_feedback() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        let preview_tile = TileSpacePosition::from_indices(1, 1);
        let preview_world = Vec2::new(64.0, 64.0);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);

        let origin = simulation
            .builder_preview()
            .expect("preview available")
            .origin;

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            confirm_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
        });

        simulation.queued_commands.push(Command::PlaceTower {
            kind: TowerKind::Basic,
            origin,
        });
        simulation.advance(Duration::ZERO);

        let preview = simulation
            .builder_preview()
            .expect("preview should be available in builder mode");
        assert_eq!(preview.rejection, Some(PlacementError::Occupied));
        assert!(!preview.placeable);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        assert_eq!(
            scene.tower_feedback,
            Some(TowerInteractionFeedback::PlacementRejected {
                kind: TowerKind::Basic,
                origin: preview.origin,
                reason: PlacementError::Occupied,
            })
        );
    }

    #[test]
    fn scene_reflects_tower_removal() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        let preview_tile = TileSpacePosition::from_indices(0, 0);
        let preview_world = Vec2::new(16.0, 16.0);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            confirm_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));

        assert_eq!(query::towers(simulation.world()).into_vec().len(), 1);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            remove_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));

        assert!(query::towers(simulation.world()).into_vec().is_empty());

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        assert!(scene.towers.is_empty());
        assert!(scene.tower_feedback.is_none());
    }

    #[test]
    fn removal_rejection_surfaces_feedback() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        simulation.queued_commands.push(Command::RemoveTower {
            tower: TowerId::new(42),
        });
        simulation.advance(Duration::ZERO);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        let expected_feedback = TowerInteractionFeedback::RemovalRejected {
            tower: TowerId::new(42),
            reason: RemovalError::MissingTower,
        };
        assert_eq!(simulation.tower_feedback(), Some(expected_feedback));
        assert_eq!(scene.tower_feedback, Some(expected_feedback));
    }

    #[test]
    fn tower_targets_follow_play_mode_transitions() {
        let mut simulation = new_simulation();
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("at least one bug spawner is configured");

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Builder,
        });
        simulation.advance(Duration::ZERO);

        let placement_tile = TileSpacePosition::from_indices(1, 1);
        let origin = simulation.tile_position_to_cell(placement_tile);
        simulation.queued_commands.push(Command::PlaceTower {
            kind: TowerKind::Basic,
            origin,
        });
        simulation.advance(Duration::ZERO);

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.advance(Duration::ZERO);

        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(255, 0, 0),
        });
        simulation.advance(Duration::ZERO);

        assert!(
            !simulation.current_targets().is_empty(),
            "attack mode should populate tower targets"
        );
        let initial_target = simulation.current_targets()[0];

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        assert_eq!(scene.tower_targets.len(), 1);
        let new_beam = scene.tower_targets[0];
        assert_eq!(new_beam.tower, initial_target.tower);
        let beam = scene.tower_targets[0];
        assert_eq!(beam.tower, initial_target.tower);
        assert_eq!(beam.bug, initial_target.bug);
        assert_eq!(
            beam.from,
            Vec2::new(
                initial_target.tower_center_cells.column,
                initial_target.tower_center_cells.row,
            )
        );
        assert_eq!(
            beam.to,
            Vec2::new(
                initial_target.bug_center_cells.column,
                initial_target.bug_center_cells.row,
            )
        );

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Builder,
        });
        simulation.advance(Duration::ZERO);

        assert!(
            simulation.current_targets().is_empty(),
            "builder mode should clear cached tower targets"
        );

        simulation.populate_scene(&mut scene);
        assert!(scene.tower_targets.is_empty());

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(255, 0, 0),
        });
        simulation.advance(Duration::ZERO);

        assert!(
            !simulation.current_targets().is_empty(),
            "targets should repopulate after returning to attack mode"
        );
        assert_eq!(simulation.current_targets()[0].tower, initial_target.tower);

        simulation.populate_scene(&mut scene);
        assert_eq!(scene.tower_targets.len(), 1);
    }

    #[test]
    fn equidistant_bugs_select_smallest_id_each_tick() {
        let mut simulation = new_simulation();

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Builder,
        });
        simulation.advance(Duration::ZERO);

        let placement_tile = TileSpacePosition::from_indices(1, 1);
        let origin = simulation.tile_position_to_cell(placement_tile);
        simulation.queued_commands.push(Command::PlaceTower {
            kind: TowerKind::Basic,
            origin,
        });
        simulation.advance(Duration::ZERO);

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.advance(Duration::ZERO);

        let tower_snapshot = query::towers(simulation.world())
            .into_vec()
            .into_iter()
            .next()
            .expect("tower placement succeeded");
        let tower_region = tower_snapshot.region;
        let tower_center = (
            tower_region.origin().column() + tower_region.size().width() / 2,
            tower_region.origin().row() + tower_region.size().height() / 2,
        );

        let spawners = query::bug_spawners(simulation.world());
        let mut pair = None;
        for (index, first) in spawners.iter().enumerate() {
            let first_distance = squared_distance_to_center(*first, tower_center);
            for second in spawners.iter().skip(index + 1) {
                if squared_distance_to_center(*second, tower_center) == first_distance {
                    pair = Some((*first, *second));
                    break;
                }
            }
            if pair.is_some() {
                break;
            }
        }

        let (first_spawner, second_spawner) =
            pair.expect("expected at least one pair of equidistant spawners");

        simulation.queued_commands.push(Command::SpawnBug {
            spawner: first_spawner,
            color: BugColor::from_rgb(255, 0, 0),
        });
        simulation.queued_commands.push(Command::SpawnBug {
            spawner: second_spawner,
            color: BugColor::from_rgb(0, 255, 0),
        });
        simulation.advance(Duration::ZERO);

        let expected_bug = query::bug_view(simulation.world())
            .iter()
            .map(|bug| bug.id)
            .min()
            .expect("two bugs should exist");

        assert!(
            !simulation.current_targets().is_empty(),
            "tower targeting should select a bug"
        );
        assert_eq!(simulation.current_targets()[0].bug, expected_bug);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        assert_eq!(scene.tower_targets.len(), 1);
        assert_eq!(scene.tower_targets[0].bug, expected_bug);

        for _ in 0..3 {
            simulation.advance(Duration::from_millis(32));
            assert!(
                !simulation.current_targets().is_empty(),
                "tower targeting should remain stable"
            );
            assert_eq!(simulation.current_targets()[0].bug, expected_bug);

            simulation.populate_scene(&mut scene);
            assert_eq!(scene.tower_targets.len(), 1);
            assert_eq!(scene.tower_targets[0].bug, expected_bug);
        }
    }

    #[test]
    fn confirm_emits_tower_placed_event() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        let preview_tile = TileSpacePosition::from_indices(0, 0);
        let preview_world = Vec2::new(16.0, 16.0);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            confirm_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));

        assert!(
            simulation
                .last_frame_events()
                .iter()
                .any(|event| matches!(event, Event::TowerPlaced { .. })),
            "confirming placement should emit TowerPlaced"
        );
    }
}
