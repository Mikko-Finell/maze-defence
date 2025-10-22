#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

use std::{
    collections::HashMap,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use glam::Vec2;
use maze_defence_core::{
    BugId, CellCoord, CellPointHalf, CellRect, CellRectSize, Command, Event, PlacementError,
    PlayMode, ProjectileSnapshot, RemovalError, TileCoord, TowerCooldownView, TowerId, TowerKind,
    TowerTarget,
};
use maze_defence_rendering::{
    BugHealthPresentation, BugPresentation, Color, FrameInput, FrameSimulationBreakdown,
    Presentation, RenderingBackend, Scene, SceneProjectile, SceneTower, SceneWall,
    TileGridPresentation, TileSpacePosition, TowerInteractionFeedback, TowerPreview,
    TowerTargetLine,
};
use maze_defence_rendering_macroquad::MacroquadBackend;
use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_system_builder::{
    Builder as TowerBuilder, BuilderInput as TowerBuilderInput,
    PlacementPreview as BuilderPlacementPreview,
};
use maze_defence_system_movement::Movement;
use maze_defence_system_spawning::{Config as SpawningConfig, Spawning};
use maze_defence_system_tower_combat::TowerCombat;
use maze_defence_system_tower_targeting::TowerTargeting;
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
            target.tower_center_cells.column(),
            target.tower_center_cells.row(),
        );
        let to = Vec2::new(
            target.bug_center_cells.column(),
            target.bug_center_cells.row(),
        );
        scene
            .tower_targets
            .push(TowerTargetLine::new(target.tower, target.bug, from, to));
    }
}

/// Populates the scene with projectiles derived from world snapshots.
///
/// `target_position` resolves the current cell-space destination for each projectile
/// using the target bug identifier and the cached half-cell fallback destination.
pub fn push_projectiles(
    scene: &mut Scene,
    projectiles: &[ProjectileSnapshot],
    mut target_position: impl FnMut(BugId, Vec2) -> Vec2,
) {
    scene.projectiles.clear();
    scene.projectiles.reserve(projectiles.len());

    for snapshot in projectiles {
        let from = half_point_to_cells(snapshot.origin_half);
        let fallback_to = half_point_to_cells(snapshot.dest_half);
        let to = target_position(snapshot.target, fallback_to);

        let progress = if snapshot.distance_half == 0 {
            1.0
        } else {
            let ratio = (snapshot.travelled_half as f64) / (snapshot.distance_half as f64);
            ratio.clamp(0.0, 1.0) as f32
        };

        let direction = to - from;
        let position = if progress <= 0.0 {
            from
        } else if progress >= 1.0 {
            to
        } else if direction.length_squared() <= f32::EPSILON {
            to
        } else {
            from + direction * progress
        };

        scene.projectiles.push(SceneProjectile::new(
            snapshot.projectile,
            from,
            to,
            position,
            progress,
        ));
    }

    fn half_point_to_cells(point: CellPointHalf) -> Vec2 {
        Vec2::new(
            point.column_half() as f32 / 2.0,
            point.row_half() as f32 / 2.0,
        )
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
    /// Requests that the renderer either synchronise presentation with the display refresh rate or run uncapped.
    #[arg(long, value_enum, value_name = "on|off")]
    vsync: Option<VsyncMode>,
}

/// CLI argument controlling whether vertical sync is requested from the rendering backend.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum VsyncMode {
    /// Request presentation synchronisation with the display refresh rate.
    On,
    /// Request uncapped presentation without waiting for the display refresh rate.
    Off,
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
    let (banner, grid_scene, wall_color) = {
        let world = simulation.world();
        let banner = bootstrap.welcome_banner(world).to_owned();
        let tile_grid = bootstrap.tile_grid(world);
        let grid_scene = TileGridPresentation::new(
            tile_grid.columns().get(),
            tile_grid.rows().get(),
            tile_grid.tile_length(),
            args.cells_per_tile,
            Color::from_rgb_u8(31, 54, 22),
        )?;
        let wall_color = Color::from_rgb_u8(68, 45, 15);
        (banner, grid_scene, wall_color)
    };

    let mut scene = Scene::new(
        grid_scene,
        wall_color,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        query::play_mode(simulation.world()),
        None,
        None,
        None,
    );
    simulation.populate_scene(&mut scene);

    let presentation = Presentation::new(banner, Color::from_rgb_u8(85, 142, 52), scene);

    let backend = match args.vsync {
        Some(VsyncMode::On) => MacroquadBackend::default().with_vsync(true),
        Some(VsyncMode::Off) => MacroquadBackend::default().with_vsync(false),
        None => MacroquadBackend::default(),
    };

    backend.run(presentation, move |dt, input, scene| {
        simulation.handle_input(input);
        simulation.advance(dt);
        let populate_start = Instant::now();
        simulation.populate_scene(scene);
        let scene_population = populate_start.elapsed();
        let advance_profile = simulation.last_advance_profile();
        FrameSimulationBreakdown::new(
            advance_profile.total,
            advance_profile.pathfinding,
            scene_population,
        )
    })
}

#[derive(Debug)]
struct Simulation {
    world: World,
    builder: TowerBuilder,
    movement: Movement,
    spawning: Spawning,
    tower_targeting: TowerTargeting,
    tower_combat: TowerCombat,
    tower_cooldowns: TowerCooldownView,
    projectiles: Vec<ProjectileSnapshot>,
    current_targets: Vec<TowerTarget>,
    pending_events: Vec<Event>,
    scratch_commands: Vec<Command>,
    queued_commands: Vec<Command>,
    pending_input: FrameInput,
    builder_preview: Option<BuilderPlacementPreview>,
    tower_feedback: Option<TowerInteractionFeedback>,
    last_placement_rejection: Option<PlacementRejection>,
    last_removal_rejection: Option<RemovalRejection>,
    bug_step_duration: Duration,
    bug_motions: HashMap<BugId, BugMotion>,
    cells_per_tile: u32,
    last_advance_profile: AdvanceProfile,
    #[cfg(test)]
    last_frame_events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, Default)]
struct AdvanceProfile {
    total: Duration,
    pathfinding: Duration,
}

impl AdvanceProfile {
    fn new(total: Duration, pathfinding: Duration) -> Self {
        Self { total, pathfinding }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcessEventsProfile {
    pathfinding: Duration,
}

impl ProcessEventsProfile {
    fn add_pathfinding(&mut self, duration: Duration) {
        self.pathfinding = self.pathfinding.saturating_add(duration);
    }
}

#[derive(Clone, Copy, Debug)]
struct BugMotion {
    from: CellCoord,
    to: CellCoord,
    elapsed: Duration,
}

impl BugMotion {
    fn new(from: CellCoord, to: CellCoord) -> Self {
        Self {
            from,
            to,
            elapsed: Duration::ZERO,
        }
    }

    fn advance(&mut self, dt: Duration, step_duration: Duration) {
        if dt.is_zero() {
            return;
        }

        self.elapsed = self.elapsed.saturating_add(dt);
        if !step_duration.is_zero() {
            self.elapsed = self.elapsed.min(step_duration);
        }
    }

    fn progress(&self, step_duration: Duration) -> f32 {
        if step_duration.is_zero() {
            return 1.0;
        }

        let numerator = self.elapsed.as_secs_f32();
        let denominator = step_duration.as_secs_f32();
        if denominator <= f32::EPSILON {
            1.0
        } else {
            (numerator / denominator).clamp(0.0, 1.0)
        }
    }
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
            tower_combat: TowerCombat::new(),
            tower_cooldowns: TowerCooldownView::default(),
            projectiles: Vec::new(),
            current_targets: Vec::new(),
            pending_events,
            scratch_commands: Vec::new(),
            queued_commands: Vec::new(),
            pending_input: FrameInput::default(),
            builder_preview: None,
            tower_feedback: None,
            last_placement_rejection: None,
            last_removal_rejection: None,
            bug_step_duration: bug_step,
            bug_motions: HashMap::new(),
            cells_per_tile,
            last_advance_profile: AdvanceProfile::default(),
            #[cfg(test)]
            last_frame_events: Vec::new(),
        };
        let _ = simulation.process_pending_events(None, TowerBuilderInput::default());
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
        let frame_start = Instant::now();
        let builder_preview = self.compute_builder_preview();
        let builder_input = self.prepare_builder_input();

        self.pending_events.clear();
        self.flush_queued_commands();

        self.advance_bug_motions(dt);
        if !dt.is_zero() {
            world::apply(
                &mut self.world,
                Command::Tick { dt },
                &mut self.pending_events,
            );
        }

        let events_profile = self.process_pending_events(builder_preview, builder_input);
        self.builder_preview = self.compute_builder_preview();
        self.last_advance_profile =
            AdvanceProfile::new(frame_start.elapsed(), events_profile.pathfinding);
    }

    fn advance_bug_motions(&mut self, dt: Duration) {
        if dt.is_zero() || self.bug_motions.is_empty() {
            return;
        }

        let step_duration = self.bug_step_duration;
        for motion in self.bug_motions.values_mut() {
            motion.advance(dt, step_duration);
        }
    }

    fn last_advance_profile(&self) -> AdvanceProfile {
        self.last_advance_profile
    }

    fn interpolated_bug_position_with_cell(&self, id: BugId, cell: Option<CellCoord>) -> Vec2 {
        if let Some(motion) = self.bug_motions.get(&id) {
            let from = Self::cell_center(motion.from);
            let to = Self::cell_center(motion.to);
            let progress = motion.progress(self.bug_step_duration);
            return from + (to - from) * progress;
        }

        if let Some(cell) = cell {
            return Self::cell_center(cell);
        }

        let occupancy = query::occupancy_view(&self.world);
        let (columns, rows) = occupancy.dimensions();
        for row in 0..rows {
            for column in 0..columns {
                let cell = CellCoord::new(column, row);
                if occupancy.occupant(cell) == Some(id) {
                    return Self::cell_center(cell);
                }
            }
        }

        Vec2::new(0.5, 0.5)
    }

    fn cell_center(cell: CellCoord) -> Vec2 {
        Vec2::new(cell.column() as f32 + 0.5, cell.row() as f32 + 0.5)
    }

    fn populate_scene(&self, scene: &mut Scene) {
        let wall_view = query::walls(&self.world);
        scene.walls.clear();
        scene.walls.extend(
            wall_view
                .iter()
                .map(|wall| SceneWall::new(wall.column(), wall.row())),
        );

        let bug_view = query::bug_view(&self.world);
        scene.bugs.clear();
        let mut bug_positions = HashMap::new();
        for bug in bug_view.iter() {
            let color = bug.color;
            let position = self.interpolated_bug_position_with_cell(bug.id, Some(bug.cell));
            let _ = bug_positions.insert(bug.id, position);
            scene.bugs.push(BugPresentation::new(
                position,
                Color::from_rgb_u8(color.red(), color.green(), color.blue()),
                BugHealthPresentation::new(bug.health.get(), bug.max_health.get()),
            ));
        }

        let tower_view = query::towers(&self.world);
        scene.towers.clear();
        scene.towers.extend(
            tower_view
                .iter()
                .map(|tower| SceneTower::new(tower.id, tower.kind, tower.region)),
        );

        push_tower_targets(scene, &self.current_targets);
        push_projectiles(scene, &self.projectiles, |bug, fallback| {
            bug_positions.get(&bug).copied().unwrap_or(fallback)
        });

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
        scene.hovered_tower = if scene.play_mode == PlayMode::Attack {
            match (
                self.pending_input.cursor_tile_space,
                self.pending_input.cursor_world_space,
            ) {
                (Some(_), Some(world)) => {
                    let cell = self.world_position_to_cell(world);
                    query::tower_at(&self.world, cell)
                }
                _ => None,
            }
        } else {
            None
        };
        scene.tower_feedback = self.tower_feedback;
    }

    fn process_pending_events(
        &mut self,
        mut builder_preview: Option<BuilderPlacementPreview>,
        mut builder_input: TowerBuilderInput,
    ) -> ProcessEventsProfile {
        let mut events = std::mem::take(&mut self.pending_events);
        let mut next_events = Vec::new();
        let mut profile = ProcessEventsProfile::default();

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

            self.handle_bug_motion_events(&events);
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
                let pathfinding_start = Instant::now();
                self.movement.handle(
                    &events,
                    &bug_view,
                    occupancy_view,
                    &target_cells,
                    |cell| query::is_cell_blocked(&self.world, cell),
                    &mut self.scratch_commands,
                );
                profile.add_pathfinding(pathfinding_start.elapsed());
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

        profile
    }

    fn handle_bug_motion_events(&mut self, events: &[Event]) {
        for event in events {
            match event {
                Event::BugAdvanced { bug_id, from, to } => {
                    let _ = self.bug_motions.insert(*bug_id, BugMotion::new(*from, *to));
                }
                Event::BugSpawned { bug_id, .. } | Event::BugExited { bug_id, .. } => {
                    let _ = self.bug_motions.remove(bug_id);
                }
                Event::BugDied { bug } => {
                    let _ = self.bug_motions.remove(bug);
                }
                Event::PlayModeChanged { mode } if *mode == PlayMode::Builder => {
                    self.bug_motions.clear();
                }
                _ => {}
            }
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
            self.tower_cooldowns = TowerCooldownView::default();
            if !self.projectiles.is_empty() {
                self.projectiles.clear();
            }
            return;
        }

        self.tower_cooldowns = query::tower_cooldowns(&self.world);
        self.projectiles.clear();
        self.projectiles.extend(query::projectiles(&self.world));

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

        if self.current_targets.is_empty() {
            return;
        }

        self.tower_combat.handle(
            play_mode,
            self.tower_cooldowns.clone(),
            &self.current_targets,
            &mut self.queued_commands,
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

    fn world_position_to_cell(&self, position: Vec2) -> CellCoord {
        let tile_grid = query::tile_grid(&self.world);
        let cells_per_tile = self.cells_per_tile.max(1);
        let tile_length = tile_grid.tile_length();
        if tile_length <= f32::EPSILON {
            return CellCoord::new(
                TileGridPresentation::SIDE_BORDER_CELL_LAYERS,
                TileGridPresentation::TOP_BORDER_CELL_LAYERS,
            );
        }

        let cell_length = tile_length / cells_per_tile as f32;
        if cell_length <= f32::EPSILON {
            return CellCoord::new(
                TileGridPresentation::SIDE_BORDER_CELL_LAYERS,
                TileGridPresentation::TOP_BORDER_CELL_LAYERS,
            );
        }

        let column_cells = Self::world_axis_to_cell_index(
            position.x,
            tile_grid.columns().get(),
            cells_per_tile,
            cell_length,
        );
        let row_cells = Self::world_axis_to_cell_index(
            position.y,
            tile_grid.rows().get(),
            cells_per_tile,
            cell_length,
        );

        let column = TileGridPresentation::SIDE_BORDER_CELL_LAYERS.saturating_add(column_cells);
        let row = TileGridPresentation::TOP_BORDER_CELL_LAYERS.saturating_add(row_cells);
        CellCoord::new(column, row)
    }

    fn world_axis_to_cell_index(
        value: f32,
        tiles: u32,
        cells_per_tile: u32,
        cell_length: f32,
    ) -> u32 {
        if tiles == 0 || cells_per_tile == 0 || cell_length <= f32::EPSILON {
            return 0;
        }

        let total_cells = tiles.saturating_mul(cells_per_tile);
        if total_cells == 0 {
            return 0;
        }

        let index = (value / cell_length).floor();
        if !index.is_finite() {
            return 0;
        }

        let max_index = (total_cells - 1) as f32;
        if index < 0.0 {
            return 0;
        }

        if index > max_index {
            return total_cells - 1;
        }

        index as u32
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
    fn bug_step_duration(&self) -> Duration {
        self.bug_step_duration
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

    #[cfg(test)]
    fn tower_cooldowns(&self) -> &TowerCooldownView {
        &self.tower_cooldowns
    }

    #[cfg(test)]
    fn projectiles(&self) -> &[ProjectileSnapshot] {
        &self.projectiles
    }

    #[cfg(test)]
    fn queued_commands(&self) -> &[Command] {
        &self.queued_commands
    }

    fn flush_queued_commands(&mut self) {
        if self.queued_commands.is_empty() {
            return;
        }

        for command in self.queued_commands.drain(..) {
            if let Command::ConfigureBugStep { step_duration } = &command {
                self.bug_step_duration = *step_duration;
                self.bug_motions.clear();
            }
            world::apply(&mut self.world, command, &mut self.pending_events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::{BugColor, BugId, Health, ProjectileId};

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
        let wall_color = Color::from_rgb_u8(60, 45, 30);

        Scene::new(
            tile_grid,
            wall_color,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            PlayMode::Attack,
            None,
            None,
            None,
        )
    }

    #[test]
    fn push_projectiles_converts_half_coordinates() {
        let mut scene = make_scene();
        let snapshot = ProjectileSnapshot {
            projectile: ProjectileId::new(7),
            tower: TowerId::new(11),
            target: BugId::new(3),
            origin_half: CellPointHalf::new(6, 4),
            dest_half: CellPointHalf::new(14, 12),
            distance_half: 10,
            travelled_half: 5,
        };

        push_projectiles(&mut scene, &[snapshot], |_bug, fallback| fallback);

        assert_eq!(scene.projectiles.len(), 1);
        let projectile = scene.projectiles[0];
        assert_eq!(projectile.id, snapshot.projectile);
        assert_eq!(projectile.from, Vec2::new(3.0, 2.0));
        assert_eq!(projectile.to, Vec2::new(7.0, 6.0));
        assert!((projectile.progress - 0.5).abs() <= f32::EPSILON);
        assert_eq!(projectile.position, Vec2::new(5.0, 4.0));
    }

    #[test]
    fn push_projectiles_homes_toward_dynamic_target() {
        let mut scene = make_scene();
        let snapshot = ProjectileSnapshot {
            projectile: ProjectileId::new(7),
            tower: TowerId::new(11),
            target: BugId::new(3),
            origin_half: CellPointHalf::new(6, 4),
            dest_half: CellPointHalf::new(14, 12),
            distance_half: 10,
            travelled_half: 5,
        };

        push_projectiles(&mut scene, &[snapshot], |_bug, fallback| {
            fallback + Vec2::new(2.0, 2.0)
        });

        let projectile = scene.projectiles[0];
        let expected_to = Vec2::new(9.0, 8.0);
        assert_eq!(projectile.to, expected_to);

        let from = Vec2::new(3.0, 2.0);
        let expected_position = from + (expected_to - from) * 0.5;
        assert_eq!(projectile.position, expected_position);
    }

    #[test]
    fn populate_scene_interpolates_bug_positions_between_cells() {
        let mut simulation = new_simulation();
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(255, 0, 0),
            health: Health::new(5),
        });
        simulation.advance(Duration::ZERO);

        let step_duration = simulation.bug_step_duration();
        simulation.advance(step_duration);

        let (from, to) = simulation
            .last_frame_events()
            .iter()
            .find_map(|event| match event {
                Event::BugAdvanced { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .expect("bug should advance after enough time elapsed");

        assert_ne!(from, to, "bug should move to a new cell");

        let partial_dt = if step_duration.is_zero() {
            Duration::ZERO
        } else {
            step_duration / 2
        };
        simulation.advance(partial_dt);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.bugs.len(), 1);
        let bug = scene.bugs[0];
        let from_vec = Simulation::cell_center(from);
        let to_vec = Simulation::cell_center(to);
        let expected_progress = if step_duration.is_zero() {
            1.0
        } else {
            (partial_dt.as_secs_f32() / step_duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        let expected_position = from_vec + (to_vec - from_vec) * expected_progress;

        assert!((bug.position - expected_position).length() <= f32::EPSILON);
    }

    #[test]
    fn interpolated_bug_position_returns_cell_center_without_motion() {
        let mut simulation = new_simulation();
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(64, 96, 128),
            health: Health::new(3),
        });
        simulation.advance(Duration::ZERO);

        let bug_view = query::bug_view(simulation.world());
        let bug = bug_view
            .iter()
            .next()
            .cloned()
            .expect("spawned bug available");

        let position = simulation.interpolated_bug_position_with_cell(bug.id, Some(bug.cell));
        assert_eq!(position, Simulation::cell_center(bug.cell));
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
    fn populate_scene_marks_hovered_tower_in_attack_mode() {
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

        let tower_view = query::towers(simulation.world());
        let tower_snapshot = tower_view.iter().next().expect("tower should be placed");

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        assert_eq!(query::play_mode(simulation.world()), PlayMode::Attack);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(preview_world),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
        });

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.hovered_tower, Some(tower_snapshot.id));
        assert_eq!(scene.play_mode, PlayMode::Attack);
    }

    #[test]
    fn hovered_tower_tracks_cursor_cell_without_offset() {
        let mut simulation = new_simulation();
        enter_builder_mode(&mut simulation);

        let placement_tile = TileSpacePosition::from_indices(0, 0);
        let placement_world = Vec2::new(16.0, 16.0);

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(placement_world),
            cursor_tile_space: Some(placement_tile),
            confirm_action: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::from_millis(16));

        let tower_view = query::towers(simulation.world());
        let tower_snapshot = tower_view.iter().next().expect("tower should be placed");

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        assert_eq!(query::play_mode(simulation.world()), PlayMode::Attack);

        let tile_grid = query::tile_grid(simulation.world());
        let cells_per_tile = simulation.cells_per_tile.max(1);
        let cell_length = tile_grid.tile_length() / cells_per_tile as f32;
        let tile_extent = tile_grid.tile_length();
        let grid_presentation = TileGridPresentation::new(
            tile_grid.columns().get(),
            tile_grid.rows().get(),
            tile_grid.tile_length(),
            cells_per_tile,
            Color::from_rgb_u8(0, 0, 0),
        )
        .expect("valid grid dimensions");
        let footprint = Vec2::splat(1.0);

        let mut scene = make_scene();

        let inside_positions = [
            Vec2::new(0.25 * cell_length, 0.25 * cell_length),
            Vec2::new(tile_extent - 0.25 * cell_length, 0.25 * cell_length),
            Vec2::new(0.25 * cell_length, tile_extent - 0.25 * cell_length),
            Vec2::new(
                tile_extent - 0.25 * cell_length,
                tile_extent - 0.25 * cell_length,
            ),
        ];
        for position in inside_positions {
            let tile_position = grid_presentation
                .snap_world_to_tile(position, footprint)
                .expect("position inside grid");
            simulation.handle_input(FrameInput {
                cursor_world_space: Some(position),
                cursor_tile_space: Some(tile_position),
                ..FrameInput::default()
            });
            simulation.populate_scene(&mut scene);
            assert_eq!(scene.hovered_tower, Some(tower_snapshot.id));
        }

        let outside_positions = [
            Vec2::new(tile_extent + 0.5 * cell_length, tile_extent * 0.5),
            Vec2::new(tile_extent * 0.5, tile_extent + 0.5 * cell_length),
            Vec2::new(tile_extent + cell_length, tile_extent + cell_length),
        ];
        for position in outside_positions {
            let tile_position = grid_presentation
                .snap_world_to_tile(position, footprint)
                .expect("position inside grid bounds");
            simulation.handle_input(FrameInput {
                cursor_world_space: Some(position),
                cursor_tile_space: Some(tile_position),
                ..FrameInput::default()
            });
            simulation.populate_scene(&mut scene);
            assert!(scene.hovered_tower.is_none());
        }

        simulation.handle_input(FrameInput {
            cursor_world_space: Some(Vec2::ZERO),
            cursor_tile_space: None,
            ..FrameInput::default()
        });
        simulation.populate_scene(&mut scene);
        assert!(scene.hovered_tower.is_none());
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
            health: Health::new(3),
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
                initial_target.tower_center_cells.column(),
                initial_target.tower_center_cells.row(),
            )
        );
        assert_eq!(
            beam.to,
            Vec2::new(
                initial_target.bug_center_cells.column(),
                initial_target.bug_center_cells.row(),
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
            health: Health::new(3),
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
            health: Health::new(3),
        });
        simulation.queued_commands.push(Command::SpawnBug {
            spawner: second_spawner,
            color: BugColor::from_rgb(0, 255, 0),
            health: Health::new(3),
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
    fn tower_combat_queues_fire_command_when_ready() {
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
            color: BugColor::from_rgb(200, 120, 80),
            health: Health::new(3),
        });
        simulation.advance(Duration::ZERO);

        assert!(
            simulation
                .queued_commands()
                .iter()
                .any(|command| matches!(command, Command::FireProjectile { .. })),
            "tower combat should queue a firing command when a target is ready",
        );
    }

    #[test]
    fn tower_combat_respects_cooldown_and_caches_projectiles() {
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
            color: BugColor::from_rgb(210, 160, 90),
            health: Health::new(3),
        });
        simulation.advance(Duration::ZERO);

        assert!(
            simulation
                .queued_commands()
                .iter()
                .any(|command| matches!(command, Command::FireProjectile { .. })),
            "initial firing command should be queued",
        );

        simulation.advance(Duration::ZERO);

        assert!(
            !simulation
                .queued_commands()
                .iter()
                .any(|command| matches!(command, Command::FireProjectile { .. })),
            "cooldown should prevent immediate refire",
        );

        let snapshot = simulation
            .tower_cooldowns()
            .iter()
            .next()
            .copied()
            .expect("cooldown snapshot should exist after firing");
        assert!(!snapshot.ready_in.is_zero());
        assert_eq!(simulation.tower_cooldowns().iter().count(), 1);

        assert!(
            !simulation.projectiles().is_empty(),
            "projectile snapshots should be cached after firing",
        );

        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Builder,
        });
        simulation.advance(Duration::ZERO);

        assert!(simulation.projectiles().is_empty());
        assert_eq!(simulation.tower_cooldowns().iter().count(), 0);
        let no_pending_fire = simulation
            .queued_commands()
            .iter()
            .all(|command| !matches!(command, Command::FireProjectile { .. }));
        assert!(no_pending_fire);
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
