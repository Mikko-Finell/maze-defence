#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

mod layout_transfer;

use std::{
    collections::HashMap,
    f32::consts::{FRAC_PI_2, PI},
    fmt,
    num::NonZeroU32,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use glam::Vec2;
use layout_transfer::{TowerLayoutSnapshot, TowerLayoutTower};
use maze_defence_core::{
    AttackBugDescriptor, AttackBurst, AttackPlan, BugColor, BugId, BugView, CellCoord,
    CellPointHalf, CellRect, CellRectSize, Command, Event, Gold, Health, PlacementError, PlayMode,
    ProjectileSnapshot, RemovalError, RoundOutcome, TileCoord, TowerCooldownView, TowerId,
    TowerKind, TowerTarget,
};
use maze_defence_rendering::{
    visuals, BugHealthPresentation, BugPresentation, BugVisual, Color, ControlPanelView,
    FrameInput, FrameSimulationBreakdown, GoldPresentation, GroundKind, GroundSpriteTiles,
    Presentation, RenderingBackend, Scene, SceneProjectile, SceneTower, SceneWall, SpriteKey,
    TierPresentation, TileGridPresentation, TileSpacePosition, TowerInteractionFeedback,
    TowerPreview, TowerTargetLine,
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
const TILE_LENGTH_TOLERANCE: f32 = 1e-3;
const DEFAULT_BUG_HEADING: f32 = 0.0;
const GROUND_TILE_MULTIPLIER: f32 = 4.0;
const BASIC_WAVE_BUGS: u32 = 4;
const BASIC_WAVE_CADENCE_MS: u32 = 600;
const BASIC_WAVE_START_DELAY_MS: u32 = 0;
const BASIC_WAVE_PRESSURE: u32 = BASIC_WAVE_BUGS;
const BASIC_WAVE_COLOR: BugColor = BugColor::from_rgb(0x2f, 0x95, 0x32);
const BASIC_WAVE_HEALTH: Health = Health::new(3);

fn duration_to_u32_millis(duration: Duration) -> u32 {
    let millis = duration.as_millis();
    if millis > u128::from(u32::MAX) {
        u32::MAX
    } else {
        millis as u32
    }
}

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
        } else if progress >= 1.0 || direction.length_squared() <= f32::EPSILON {
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
    /// Restores the provided layout snapshot before the first frame renders.
    #[arg(long, value_name = "LAYOUT")]
    layout: Option<String>,
    /// Controls whether per-second frame timing metrics are printed to stdout.
    #[arg(long = "show-fps", value_enum, value_name = "on|off", default_value_t = Toggle::Off)]
    show_fps: Toggle,
    /// Selects whether sprites or primitive shapes render towers and bugs.
    #[arg(
        long = "visual-style",
        value_enum,
        value_name = "sprites|primitives",
        default_value_t = VisualStyle::Sprites
    )]
    visual_style: VisualStyle,
}

/// CLI argument controlling whether vertical sync is requested from the rendering backend.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum VsyncMode {
    /// Request presentation synchronisation with the display refresh rate.
    On,
    /// Request uncapped presentation without waiting for the display refresh rate.
    Off,
}

/// Generic on/off toggle used by CLI flags.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Toggle {
    /// Enable the associated behaviour.
    On,
    /// Disable the associated behaviour.
    Off,
}

/// Rendering styles offered by the CLI adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum VisualStyle {
    /// Render towers and bugs using sprite assets.
    Sprites,
    /// Render towers and bugs using primitive shapes.
    Primitives,
}

impl Toggle {
    /// Returns whether the toggle requests the behaviour to be enabled.
    #[must_use]
    fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
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
    let show_fps = args.show_fps.enabled();

    let layout_snapshot = args
        .layout
        .as_deref()
        .map(|layout| {
            TowerLayoutSnapshot::decode(layout)
                .map_err(|error| anyhow!("Failed to decode layout snapshot: {error}"))
        })
        .transpose()
        .with_context(|| "failed to restore layout from --layout")?;

    let (columns, rows) = if let Some(snapshot) = &layout_snapshot {
        (snapshot.columns, snapshot.rows)
    } else if let Some(size) = args.grid_size {
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
        args.visual_style,
    );
    if let Some(snapshot) = layout_snapshot.as_ref() {
        simulation
            .apply_layout_snapshot(snapshot)
            .map_err(anyhow::Error::from)
            .with_context(|| "failed to restore layout from --layout")?;
    }
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
        None,
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
        Some(ControlPanelView::new(200.0, Color::from_rgb_u8(0, 0, 0))),
        Some(GoldPresentation::new(query::gold(simulation.world()))),
        Some(TierPresentation::new(query::difficulty_tier(
            simulation.world(),
        ))),
    );
    simulation.populate_scene(&mut scene);

    let presentation = Presentation::new(banner, Color::from_rgb_u8(85, 142, 52), scene);

    let backend = match args.vsync {
        Some(VsyncMode::On) => MacroquadBackend::default().with_vsync(true),
        Some(VsyncMode::Off) => MacroquadBackend::default().with_vsync(false),
        None => MacroquadBackend::default(),
    };
    let backend = backend
        .with_show_fps(show_fps)
        .with_sprite_loading(args.visual_style == VisualStyle::Sprites);

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
    gold: Gold,
    difficulty_tier: u32,
    last_placement_rejection: Option<PlacementRejection>,
    last_removal_rejection: Option<RemovalRejection>,
    bug_step_duration: Duration,
    bug_motions: HashMap<BugId, BugMotion>,
    bug_headings: HashMap<BugId, f32>,
    cells_per_tile: u32,
    visual_style: VisualStyle,
    last_advance_profile: AdvanceProfile,
    last_announced_play_mode: PlayMode,
    active_wave: Option<WaveState>,
    auto_spawn_enabled: bool,
    pending_outcome_command: bool,
    awaiting_round_resolution: bool,
    #[cfg(test)]
    last_frame_events: Vec<Event>,
}

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
enum LayoutImportError {
    ColumnMismatch { expected: u32, observed: u32 },
    RowMismatch { expected: u32, observed: u32 },
    CellsPerTileMismatch { expected: u32, observed: u32 },
    TileLengthMismatch { expected: f32, observed: f32 },
    RestorationMismatch { expected: usize, observed: usize },
}

impl fmt::Display for LayoutImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ColumnMismatch { expected, observed } => write!(
                f,
                "Layout uses {observed} columns but the current map expects {expected}."
            ),
            Self::RowMismatch { expected, observed } => write!(
                f,
                "Layout uses {observed} rows but the current map expects {expected}."
            ),
            Self::CellsPerTileMismatch { expected, observed } => write!(
                f,
                "Layout was authored with {observed} cells per tile but the map is configured for {expected}."
            ),
            Self::TileLengthMismatch { expected, observed } => write!(
                f,
                "Layout was authored with tile length {observed} but the map is configured for {expected}."
            ),
            Self::RestorationMismatch { expected, observed } => write!(
                f,
                "Restored {observed} towers but the layout specified {expected}."
            ),
        }
    }
}

impl std::error::Error for LayoutImportError {}

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
    step_duration: Duration,
    elapsed: Duration,
}

impl BugMotion {
    fn new(from: CellCoord, to: CellCoord, step_duration: Duration) -> Self {
        Self {
            from,
            to,
            step_duration,
            elapsed: Duration::ZERO,
        }
    }

    fn advance(&mut self, dt: Duration) {
        if dt.is_zero() {
            return;
        }

        self.elapsed = self.elapsed.saturating_add(dt);
        if !self.step_duration.is_zero() {
            self.elapsed = self.elapsed.min(self.step_duration);
        }
    }

    fn progress(&self) -> f32 {
        if self.step_duration.is_zero() {
            return 1.0;
        }

        let numerator = self.elapsed.as_secs_f32();
        let denominator = self.step_duration.as_secs_f32();
        if denominator <= f32::EPSILON {
            1.0
        } else {
            (numerator / denominator).clamp(0.0, 1.0)
        }
    }
}

#[derive(Clone, Debug)]
struct ScheduledSpawn {
    at: Duration,
    spawner: CellCoord,
    bug: AttackBugDescriptor,
}

impl ScheduledSpawn {
    fn new(at: Duration, spawner: CellCoord, bug: AttackBugDescriptor) -> Self {
        Self { at, spawner, bug }
    }
}

#[derive(Clone, Debug)]
struct WaveState {
    plan: AttackPlan,
    scheduled: Vec<ScheduledSpawn>,
    next_spawn: usize,
    elapsed: Duration,
}

impl WaveState {
    fn new(plan: AttackPlan) -> Self {
        let mut scheduled = Vec::new();
        for burst in plan.bursts() {
            let start_ms = u64::from(burst.start_ms());
            let cadence_ms = u64::from(burst.cadence_ms().get());
            for index in 0..burst.count().get() {
                let offset_ms = start_ms.saturating_add(cadence_ms.saturating_mul(index as u64));
                scheduled.push(ScheduledSpawn::new(
                    Duration::from_millis(offset_ms),
                    burst.spawner(),
                    burst.bug(),
                ));
            }
        }
        scheduled.sort_by_key(|spawn| spawn.at);
        Self {
            plan,
            scheduled,
            next_spawn: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn advance(&mut self, dt: Duration, out: &mut Vec<Command>) {
        self.elapsed = self.elapsed.saturating_add(dt);
        while let Some(spawn) = self.scheduled.get(self.next_spawn) {
            if self.elapsed < spawn.at {
                break;
            }
            out.push(Command::SpawnBug {
                spawner: spawn.spawner,
                color: spawn.bug.color(),
                health: spawn.bug.health(),
                step_ms: spawn.bug.step_ms(),
            });
            self.next_spawn += 1;
        }
    }

    fn finished(&self) -> bool {
        self.next_spawn >= self.scheduled.len()
    }

    #[cfg(test)]
    fn plan(&self) -> &AttackPlan {
        &self.plan
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
        visual_style: VisualStyle,
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

        let initial_play_mode = query::play_mode(&world);
        pending_events.push(Event::PlayModeChanged {
            mode: initial_play_mode,
        });
        let gold = query::gold(&world);
        let difficulty_tier = query::difficulty_tier(&world);
        let mut simulation = Self {
            world,
            builder: TowerBuilder::default(),
            movement: Movement::default(),
            spawning: Spawning::new(SpawningConfig::new(
                bug_spawn_interval,
                bug_step,
                SPAWN_RNG_SEED,
            )),
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
            gold,
            difficulty_tier,
            last_placement_rejection: None,
            last_removal_rejection: None,
            bug_step_duration: bug_step,
            bug_motions: HashMap::new(),
            bug_headings: HashMap::new(),
            cells_per_tile,
            visual_style,
            last_advance_profile: AdvanceProfile::default(),
            last_announced_play_mode: initial_play_mode,
            active_wave: None,
            auto_spawn_enabled: false,
            pending_outcome_command: false,
            awaiting_round_resolution: false,
            #[cfg(test)]
            last_frame_events: Vec::new(),
        };
        let _ = simulation.process_pending_events(None, TowerBuilderInput::default());
        simulation.builder_preview = simulation.compute_builder_preview();
        simulation.last_announced_play_mode = query::play_mode(&simulation.world);
        simulation
    }

    fn world(&self) -> &World {
        &self.world
    }

    #[cfg(test)]
    fn active_wave_plan(&self) -> Option<&AttackPlan> {
        self.active_wave.as_ref().map(|wave| wave.plan())
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

        if input.spawn_wave {
            self.start_basic_wave();
        }

        self.pending_input = FrameInput {
            mode_toggle: false,
            spawn_wave: false,
            ..input
        };
    }

    fn start_basic_wave(&mut self) {
        if self.pending_outcome_command
            || self
                .queued_commands
                .iter()
                .any(|command| matches!(command, Command::ResolveRound { .. }))
            || self.awaiting_round_resolution
            || self.active_wave.is_some()
        {
            return;
        }

        let Some(spawner) = query::bug_spawners(&self.world).first().copied() else {
            return;
        };

        let plan = self.basic_attack_plan(spawner);
        if plan.is_empty() {
            return;
        }

        if query::play_mode(&self.world) != PlayMode::Attack {
            self.queued_commands.push(Command::SetPlayMode {
                mode: PlayMode::Attack,
            });
        }

        self.active_wave = Some(WaveState::new(plan));
        self.awaiting_round_resolution = true;
    }

    fn queue_round_outcome(&mut self, outcome: RoundOutcome) -> bool {
        if self.pending_outcome_command {
            return false;
        }
        if self
            .queued_commands
            .iter()
            .any(|command| matches!(command, Command::ResolveRound { .. }))
        {
            self.pending_outcome_command = true;
            return false;
        }
        self.queued_commands.push(Command::ResolveRound { outcome });
        self.pending_outcome_command = true;
        true
    }

    fn basic_attack_plan(&self, spawner: CellCoord) -> AttackPlan {
        let Some(count) = NonZeroU32::new(BASIC_WAVE_BUGS) else {
            return AttackPlan::new(0, Vec::new());
        };
        let Some(cadence) = NonZeroU32::new(BASIC_WAVE_CADENCE_MS) else {
            return AttackPlan::new(0, Vec::new());
        };
        let bug = AttackBugDescriptor::new(
            BASIC_WAVE_COLOR,
            BASIC_WAVE_HEALTH,
            duration_to_u32_millis(self.bug_step_duration),
        );
        let burst = AttackBurst::new(spawner, bug, count, cadence, BASIC_WAVE_START_DELAY_MS);
        AttackPlan::new(BASIC_WAVE_PRESSURE, vec![burst])
    }

    fn capture_layout_snapshot(&self) -> TowerLayoutSnapshot {
        let tile_grid = query::tile_grid(&self.world);
        let towers = query::towers(&self.world);
        let towers = towers
            .iter()
            .map(|tower| TowerLayoutTower {
                kind: tower.kind,
                origin: tower.region.origin(),
            })
            .collect();
        TowerLayoutSnapshot {
            columns: tile_grid.columns().get(),
            rows: tile_grid.rows().get(),
            tile_length: tile_grid.tile_length(),
            cells_per_tile: query::cells_per_tile(&self.world),
            towers,
        }
    }

    fn apply_layout_snapshot(
        &mut self,
        snapshot: &TowerLayoutSnapshot,
    ) -> Result<(), LayoutImportError> {
        let expected_towers = snapshot.towers.len();
        self.queue_layout_import(snapshot)?;
        self.process_layout_commands_immediately();
        let actual = query::towers(&self.world).iter().count();
        if actual != expected_towers {
            return Err(LayoutImportError::RestorationMismatch {
                expected: expected_towers,
                observed: actual,
            });
        }
        Ok(())
    }

    fn queue_layout_import(
        &mut self,
        snapshot: &TowerLayoutSnapshot,
    ) -> Result<(), LayoutImportError> {
        let tile_grid = query::tile_grid(&self.world);
        if tile_grid.columns().get() != snapshot.columns {
            return Err(LayoutImportError::ColumnMismatch {
                expected: tile_grid.columns().get(),
                observed: snapshot.columns,
            });
        }

        if tile_grid.rows().get() != snapshot.rows {
            return Err(LayoutImportError::RowMismatch {
                expected: tile_grid.rows().get(),
                observed: snapshot.rows,
            });
        }

        let cells_per_tile = query::cells_per_tile(&self.world);
        if cells_per_tile != snapshot.cells_per_tile {
            return Err(LayoutImportError::CellsPerTileMismatch {
                expected: cells_per_tile,
                observed: snapshot.cells_per_tile,
            });
        }

        let tile_length = tile_grid.tile_length();
        if (tile_length - snapshot.tile_length).abs() > TILE_LENGTH_TOLERANCE {
            return Err(LayoutImportError::TileLengthMismatch {
                expected: tile_length,
                observed: snapshot.tile_length,
            });
        }

        let current_mode = query::play_mode(&self.world);
        if current_mode != PlayMode::Builder {
            self.queued_commands.push(Command::SetPlayMode {
                mode: PlayMode::Builder,
            });
        }

        let towers = query::towers(&self.world);
        for tower in towers.iter() {
            self.queued_commands
                .push(Command::RemoveTower { tower: tower.id });
        }

        for layout_tower in &snapshot.towers {
            self.queued_commands.push(Command::PlaceTower {
                kind: layout_tower.kind,
                origin: layout_tower.origin,
            });
        }

        if current_mode != PlayMode::Builder {
            self.queued_commands
                .push(Command::SetPlayMode { mode: current_mode });
        }

        Ok(())
    }

    fn process_layout_commands_immediately(&mut self) {
        let builder_preview = self.compute_builder_preview();
        let builder_input = TowerBuilderInput::default();
        self.pending_events.clear();
        self.flush_queued_commands();
        let events_profile = self.process_pending_events(builder_preview, builder_input);
        self.builder_preview = self.compute_builder_preview();
        self.last_advance_profile = AdvanceProfile::new(Duration::ZERO, events_profile.pathfinding);
        self.last_announced_play_mode = query::play_mode(&self.world);
    }

    fn advance(&mut self, dt: Duration) {
        let frame_start = Instant::now();
        let builder_preview = self.compute_builder_preview();
        let builder_input = self.prepare_builder_input();

        self.pending_events.clear();
        self.flush_queued_commands();

        self.advance_bug_motions(dt);
        if !dt.is_zero() {
            let mut emitted = Vec::new();
            self.apply_command(Command::Tick { dt }, &mut emitted);
            self.pending_events.append(&mut emitted);
        }

        let events_profile = self.process_pending_events(builder_preview, builder_input);
        self.builder_preview = self.compute_builder_preview();
        self.last_advance_profile =
            AdvanceProfile::new(frame_start.elapsed(), events_profile.pathfinding);
        self.announce_builder_mode_if_changed();
    }

    fn announce_builder_mode_if_changed(&mut self) {
        let current_mode = query::play_mode(&self.world);
        if current_mode != self.last_announced_play_mode {
            let previous_mode = self.last_announced_play_mode;
            self.last_announced_play_mode = current_mode;
            if cfg!(test) {
                return;
            }
            if current_mode == PlayMode::Builder || previous_mode == PlayMode::Builder {
                let snapshot = self.capture_layout_snapshot();
                let encoded = snapshot.encode();
                println!("{encoded}");
            }
        }
    }

    fn advance_bug_motions(&mut self, dt: Duration) {
        if dt.is_zero() || self.bug_motions.is_empty() {
            return;
        }

        for motion in self.bug_motions.values_mut() {
            motion.advance(dt);
        }
    }

    fn apply_command(&mut self, command: Command, out_events: &mut Vec<Event>) {
        match command {
            Command::ConfigureBugStep { step_duration } => {
                self.bug_step_duration = step_duration;
                self.bug_motions.clear();
                self.spawning.set_step_duration(step_duration);
                world::apply(
                    &mut self.world,
                    Command::ConfigureBugStep { step_duration },
                    out_events,
                );
            }
            other => {
                world::apply(&mut self.world, other, out_events);
            }
        }
    }

    fn last_advance_profile(&self) -> AdvanceProfile {
        self.last_advance_profile
    }

    fn interpolated_bug_position_with_cell(&self, id: BugId, cell: Option<CellCoord>) -> Vec2 {
        if let Some(motion) = self.bug_motions.get(&id) {
            let from = Self::cell_center(motion.from);
            let to = Self::cell_center(motion.to);
            let progress = motion.progress();
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

    fn bug_heading_from_cells(from: CellCoord, to: CellCoord) -> Option<f32> {
        if from == to {
            return None;
        }

        let from_center = Self::cell_center(from);
        let to_center = Self::cell_center(to);
        let delta = to_center - from_center;
        if delta.length_squared() <= f32::EPSILON {
            return None;
        }

        let heading = delta.y.atan2(delta.x) + FRAC_PI_2;
        Some(Self::normalise_radians(heading))
    }

    fn normalise_radians(angle: f32) -> f32 {
        if !angle.is_finite() {
            return DEFAULT_BUG_HEADING;
        }

        let two_pi = 2.0 * PI;
        if two_pi <= f32::EPSILON {
            return angle.clamp(-PI, PI);
        }

        let mut wrapped = angle % two_pi;
        if wrapped > PI {
            wrapped -= two_pi;
        } else if wrapped < -PI {
            wrapped += two_pi;
        }

        wrapped.clamp(-PI, PI)
    }

    fn populate_scene(&mut self, scene: &mut Scene) {
        let use_sprite_visuals = self.visual_style == VisualStyle::Sprites;
        const DEFAULT_TURRET_HEADING: f32 = 0.0;

        scene.ground = if use_sprite_visuals {
            self.ground_tiles()
        } else {
            None
        };

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
            let health = BugHealthPresentation::new(bug.health.get(), bug.max_health.get());

            let presentation = if use_sprite_visuals {
                let stored_heading = self.bug_headings.get(&bug.id).copied();
                let heading = stored_heading
                    .or_else(|| {
                        self.bug_motions
                            .get(&bug.id)
                            .and_then(|motion| Self::bug_heading_from_cells(motion.from, motion.to))
                    })
                    .unwrap_or(DEFAULT_BUG_HEADING);
                let _ = self.bug_headings.entry(bug.id).or_insert(heading);
                let sprite_visual = visuals::bug_sprite_visual(
                    bug.cell.column(),
                    bug.cell.row(),
                    SpriteKey::BugBody,
                    heading,
                );
                let BugVisual::Sprite(sprite) = sprite_visual else {
                    unreachable!("bug sprite helper should return sprite visuals");
                };
                BugPresentation::new_sprite(bug.id, position, sprite, health)
            } else {
                BugPresentation::new_circle(
                    bug.id,
                    position,
                    Color::from_rgb_u8(color.red(), color.green(), color.blue()),
                    health,
                )
            };

            scene.bugs.push(presentation);
        }

        let tower_view = query::towers(&self.world);
        scene.towers.clear();
        scene.towers.extend(tower_view.iter().map(|tower| {
            let descriptor = SceneTower::new(tower.id, tower.kind, tower.region);
            if use_sprite_visuals {
                let visual = visuals::tower_sprite_visual(tower.region, DEFAULT_TURRET_HEADING);
                descriptor.with_visual(visual)
            } else {
                descriptor
            }
        }));

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
        scene.gold = Some(GoldPresentation::new(self.gold));
        scene.tier = Some(TierPresentation::new(self.difficulty_tier));
    }

    fn process_pending_events(
        &mut self,
        mut builder_preview: Option<BuilderPlacementPreview>,
        mut builder_input: TowerBuilderInput,
    ) -> ProcessEventsProfile {
        let mut events = std::mem::take(&mut self.pending_events);
        let mut next_events = Vec::new();
        let mut profile = ProcessEventsProfile::default();
        let mut emitted_events = Vec::new();

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
            self.update_gold_from_events(&events);
            self.update_difficulty_tier_from_events(&events);

            if events
                .iter()
                .any(|event| matches!(event, Event::RoundLost { .. }))
            {
                self.active_wave = None;
                self.awaiting_round_resolution = false;
                let _ = self.queue_round_outcome(RoundOutcome::Loss);
            }

            let play_mode = query::play_mode(&self.world);
            let spawners = query::bug_spawners(&self.world);
            self.scratch_commands.clear();
            if self.auto_spawn_enabled {
                self.spawning
                    .handle(&events, play_mode, &spawners, &mut self.scratch_commands);
                let mut commands = std::mem::take(&mut self.scratch_commands);
                for command in commands.drain(..) {
                    self.apply_command(command, &mut emitted_events);
                    next_events.append(&mut emitted_events);
                }
                self.scratch_commands = commands;
            }

            let mut wave_commands = Vec::new();
            let mut wave_complete = false;
            if let Some(wave) = self.active_wave.as_mut() {
                let elapsed = events
                    .iter()
                    .fold(Duration::ZERO, |acc, event| match event {
                        Event::TimeAdvanced { dt } => acc.saturating_add(*dt),
                        _ => acc,
                    });
                wave.advance(elapsed, &mut wave_commands);
                wave_complete = wave.finished();
            }
            for command in wave_commands.drain(..) {
                self.apply_command(command, &mut emitted_events);
                next_events.append(&mut emitted_events);
            }
            if wave_complete {
                self.active_wave = None;
                if query::bug_view(&self.world).iter().next().is_none() {
                    if self.queue_round_outcome(RoundOutcome::Win) || self.pending_outcome_command {
                        self.awaiting_round_resolution = false;
                    }
                }
            }

            self.scratch_commands.clear();
            if play_mode == PlayMode::Attack {
                let bug_view = query::bug_view(&self.world);
                let occupancy_view = query::occupancy_view(&self.world);
                let target_cells = query::target_cells(&self.world);
                let navigation_view = query::navigation_field(&self.world);
                let reservation_ledger = query::reservation_ledger(&self.world);
                let pathfinding_start = Instant::now();
                self.movement.handle(
                    &events,
                    &bug_view,
                    occupancy_view,
                    navigation_view,
                    reservation_ledger,
                    &target_cells,
                    |cell| query::is_cell_blocked(&self.world, cell),
                    &mut self.scratch_commands,
                );
                profile.add_pathfinding(pathfinding_start.elapsed());
            }
            let mut commands = std::mem::take(&mut self.scratch_commands);
            for command in commands.drain(..) {
                self.apply_command(command, &mut emitted_events);
                next_events.append(&mut emitted_events);
            }
            self.scratch_commands = commands;

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
            let mut commands = std::mem::take(&mut self.scratch_commands);
            for command in commands.drain(..) {
                self.apply_command(command, &mut emitted_events);
                next_events.append(&mut emitted_events);
            }
            self.scratch_commands = commands;

            if self.awaiting_round_resolution
                && self.active_wave.is_none()
                && query::bug_view(&self.world).iter().next().is_none()
            {
                if self.queue_round_outcome(RoundOutcome::Win) || self.pending_outcome_command {
                    self.awaiting_round_resolution = false;
                }
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
        let mut bug_view_cache: Option<BugView> = None;
        for event in events {
            match event {
                Event::BugAdvanced { bug_id, from, to } => {
                    let step_duration = self
                        .bug_specific_step_duration(*bug_id, &mut bug_view_cache)
                        .or_else(|| {
                            self.bug_motions
                                .get(bug_id)
                                .map(|motion| motion.step_duration)
                        })
                        .unwrap_or(self.bug_step_duration);
                    let _ = self
                        .bug_motions
                        .insert(*bug_id, BugMotion::new(*from, *to, step_duration));
                    if let Some(heading) = Self::bug_heading_from_cells(*from, *to) {
                        let _ = self.bug_headings.insert(*bug_id, heading);
                    }
                }
                Event::BugSpawned { bug_id, .. } => {
                    let _ = self.bug_motions.remove(bug_id);
                    let _ = self.bug_headings.insert(*bug_id, DEFAULT_BUG_HEADING);
                }
                Event::BugExited { bug_id, .. } => {
                    let _ = self.bug_motions.remove(bug_id);
                    let _ = self.bug_headings.remove(bug_id);
                }
                Event::BugDied { bug } => {
                    let _ = self.bug_motions.remove(bug);
                    let _ = self.bug_headings.remove(bug);
                }
                Event::PlayModeChanged { mode } if *mode == PlayMode::Builder => {
                    self.bug_motions.clear();
                    self.bug_headings.clear();
                }
                _ => {}
            }
        }
    }

    fn bug_specific_step_duration(
        &self,
        bug_id: BugId,
        bug_view_cache: &mut Option<BugView>,
    ) -> Option<Duration> {
        let bug_view = bug_view_cache.get_or_insert_with(|| query::bug_view(&self.world));
        bug_view.iter().find_map(|bug| {
            if bug.id == bug_id {
                Some(Duration::from_millis(u64::from(bug.step_ms)))
            } else {
                None
            }
        })
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

    fn update_gold_from_events(&mut self, events: &[Event]) {
        for event in events {
            if let Event::GoldChanged { amount } = event {
                self.gold = *amount;
            }
        }
    }

    fn update_difficulty_tier_from_events(&mut self, events: &[Event]) {
        for event in events {
            if let Event::DifficultyTierChanged { tier } = event {
                self.difficulty_tier = *tier;
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

    fn ground_tiles(&self) -> Option<GroundSpriteTiles> {
        if self.cells_per_tile == 0 {
            return None;
        }

        let footprint = Self::tower_footprint(TowerKind::Basic);
        if footprint.width() == 0 || footprint.height() == 0 {
            return None;
        }

        let base_tiles = Vec2::new(
            footprint.width() as f32 / self.cells_per_tile as f32,
            footprint.height() as f32 / self.cells_per_tile as f32,
        );

        if base_tiles.x <= f32::EPSILON || base_tiles.y <= f32::EPSILON {
            return None;
        }

        let span_tiles = base_tiles * GROUND_TILE_MULTIPLIER;
        visuals::ground_sprite_tiles(
            span_tiles,
            self.cells_per_tile,
            SpriteKey::GroundGrass,
            GroundKind::Grass,
        )
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
    fn bug_step_ms(&self) -> u32 {
        use std::convert::TryFrom;

        u32::try_from(self.bug_step_duration.as_millis()).unwrap_or(u32::MAX)
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

        let mut emitted = Vec::new();
        let mut queued = std::mem::take(&mut self.queued_commands);
        for command in queued.drain(..) {
            let resolves_round = matches!(command, Command::ResolveRound { .. });
            self.apply_command(command, &mut emitted);
            if resolves_round {
                self.pending_outcome_command = false;
                self.awaiting_round_resolution = false;
            }
            self.pending_events.append(&mut emitted);
        }
        self.queued_commands = queued;
    }
}

impl Drop for Simulation {
    fn drop(&mut self) {
        if cfg!(test) {
            return;
        }

        let snapshot = self.capture_layout_snapshot();
        let encoded = snapshot.encode();
        println!("{encoded}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::{BugColor, BugId, Health, ProjectileId};
    use maze_defence_rendering::{BugVisual, SpriteInstance, SpriteKey, TowerVisual};
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };

    fn new_simulation_with_style(style: VisualStyle) -> Simulation {
        Simulation::new(
            4,
            3,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Duration::from_millis(200),
            Duration::from_secs(1),
            style,
        )
    }

    fn new_simulation() -> Simulation {
        new_simulation_with_style(VisualStyle::Sprites)
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
            None,
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
            Some(ControlPanelView::new(200.0, Color::from_rgb_u8(0, 0, 0))),
            Some(GoldPresentation::new(Gold::new(0))),
            Some(TierPresentation::new(0)),
        )
    }

    #[test]
    fn cli_args_default_to_sprite_visuals() {
        let args =
            CliArgs::try_parse_from(["maze-defence"]).expect("default arguments should parse");
        assert_eq!(args.visual_style, VisualStyle::Sprites);
    }

    #[test]
    fn cli_args_allow_primitive_visuals() {
        let args = CliArgs::try_parse_from(["maze-defence", "--visual-style", "primitives"])
            .expect("primitive visuals should parse");
        assert_eq!(args.visual_style, VisualStyle::Primitives);
    }

    #[test]
    fn simulation_records_requested_visual_style() {
        let simulation = Simulation::new(
            4,
            3,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Duration::from_millis(200),
            Duration::from_secs(1),
            VisualStyle::Primitives,
        );
        assert_eq!(simulation.visual_style, VisualStyle::Primitives);
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
        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.advance(Duration::ZERO);
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(255, 0, 0),
            health: Health::new(5),
            step_ms: simulation.bug_step_ms(),
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

        assert!((bug.position() - expected_position).length() <= f32::EPSILON);
        match bug.style {
            BugVisual::Sprite(sprite) => {
                assert_eq!(sprite.sprite, SpriteKey::BugBody);
                let expected_heading =
                    Simulation::bug_heading_from_cells(from, to).unwrap_or(DEFAULT_BUG_HEADING);
                assert!((sprite.rotation_radians - expected_heading).abs() <= f32::EPSILON);
            }
            BugVisual::PrimitiveCircle { .. } => {
                panic!("sprite visual expected when sprites enabled");
            }
        }
    }

    #[test]
    fn populate_scene_interpolation_respects_bug_step_duration() {
        let mut simulation = new_simulation();
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

        let slow_step_ms = simulation.bug_step_ms().saturating_mul(2);
        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(255, 255, 255),
            health: Health::new(7),
            step_ms: slow_step_ms,
        });
        simulation.advance(Duration::ZERO);

        let slow_step_duration = Duration::from_millis(u64::from(slow_step_ms));
        simulation.advance(slow_step_duration);

        let (from, to) = simulation
            .last_frame_events()
            .iter()
            .find_map(|event| match event {
                Event::BugAdvanced { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .expect("bug should advance after enough time elapsed");

        let partial_dt = if slow_step_duration.is_zero() {
            Duration::ZERO
        } else {
            slow_step_duration / 4
        };
        simulation.advance(partial_dt);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.bugs.len(), 1);
        let bug = scene.bugs[0];
        let from_vec = Simulation::cell_center(from);
        let to_vec = Simulation::cell_center(to);
        let expected_progress = if slow_step_duration.is_zero() {
            1.0
        } else {
            (partial_dt.as_secs_f32() / slow_step_duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        let expected_position = from_vec + (to_vec - from_vec) * expected_progress;

        assert!((bug.position() - expected_position).length() <= f32::EPSILON);
    }

    #[test]
    fn interpolated_bug_position_returns_cell_center_without_motion() {
        let mut simulation = new_simulation();
        simulation.queued_commands.push(Command::SetPlayMode {
            mode: PlayMode::Attack,
        });
        simulation.advance(Duration::ZERO);
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(64, 96, 128),
            health: Health::new(3),
            step_ms: simulation.bug_step_ms(),
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

    #[test]
    fn populate_scene_sets_sprite_visuals_when_selected() {
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

        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");
        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(200, 100, 50),
            health: Health::new(3),
            step_ms: simulation.bug_step_ms(),
        });
        simulation.advance(Duration::ZERO);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.towers.len(), 1);
        let tower = scene.towers[0];
        match tower.visual {
            TowerVisual::Sprite { base, turret } => {
                assert_eq!(base.sprite, SpriteKey::TowerBase);
                assert_eq!(turret.sprite, SpriteKey::TowerTurret);
                assert!(turret.rotation_radians.abs() <= f32::EPSILON);
            }
            TowerVisual::PrimitiveRect => {
                panic!("sprite tower visual expected when sprites enabled");
            }
        }

        assert_eq!(scene.bugs.len(), 1);
        let bug = scene.bugs[0];
        match bug.style {
            BugVisual::Sprite(sprite) => {
                assert_eq!(sprite.sprite, SpriteKey::BugBody);
            }
            BugVisual::PrimitiveCircle { .. } => {
                panic!("sprite bug visual expected when sprites enabled");
            }
        }

        let ground = scene
            .ground
            .as_ref()
            .expect("ground tiles expected when sprites enabled");
        assert_eq!(ground.kind, GroundKind::Grass);
        assert_eq!(ground.sprite.sprite, SpriteKey::GroundGrass);
        let footprint = Simulation::tower_footprint(TowerKind::Basic);
        let base_tiles = Vec2::new(
            footprint.width() as f32 / simulation.cells_per_tile as f32,
            footprint.height() as f32 / simulation.cells_per_tile as f32,
        );
        let expected_size = base_tiles * GROUND_TILE_MULTIPLIER * simulation.cells_per_tile as f32;
        assert_eq!(ground.sprite.size, expected_size);
        assert_eq!(ground.sprite.pivot, Vec2::ZERO);
    }

    #[test]
    fn populate_scene_preserves_primitives_when_requested() {
        let mut simulation = new_simulation_with_style(VisualStyle::Primitives);

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

        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");
        simulation.queued_commands.push(Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(128, 64, 32),
            health: Health::new(3),
            step_ms: simulation.bug_step_ms(),
        });
        simulation.advance(Duration::ZERO);

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);

        assert_eq!(scene.towers.len(), 1);
        assert_eq!(scene.towers[0].visual, TowerVisual::PrimitiveRect);

        assert_eq!(scene.bugs.len(), 1);
        match scene.bugs[0].style {
            BugVisual::PrimitiveCircle { .. } => {}
            BugVisual::Sprite(_) => {
                panic!("primitive bug visual expected when primitives requested");
            }
        }

        assert!(scene.ground.is_none());
    }

    fn enter_builder_mode(simulation: &mut Simulation) {
        if query::play_mode(simulation.world()) == PlayMode::Builder {
            simulation.advance(Duration::ZERO);
            return;
        }

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
            ..FrameInput::default()
        });

        assert_eq!(
            simulation.queued_commands,
            vec![Command::SetPlayMode {
                mode: PlayMode::Attack,
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
            ..FrameInput::default()
        });

        assert_eq!(
            simulation.queued_commands,
            vec![Command::SetPlayMode {
                mode: PlayMode::Attack,
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
            cursor_world_space: Some(Vec2::new(16.0, 48.0)),
            cursor_tile_space: Some(initial_tile),
            ..FrameInput::default()
        });

        simulation.advance(Duration::ZERO);
        assert!(simulation.queued_commands.is_empty());

        let preview_tile = TileSpacePosition::from_indices(3, 2);
        simulation.handle_input(FrameInput {
            mode_toggle: false,
            cursor_world_space: Some(Vec2::new(96.0, 64.0)),
            cursor_tile_space: Some(preview_tile),
            ..FrameInput::default()
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
    fn advance_does_not_spawn_bug_without_wave() {
        let mut simulation = new_simulation();

        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.advance(Duration::from_millis(500));
        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.advance(Duration::from_millis(500));
        assert!(query::bug_view(simulation.world()).into_vec().is_empty());
    }

    #[test]
    fn automatic_spawning_disabled_in_both_modes() {
        let mut simulation = new_simulation();

        simulation.advance(Duration::from_secs(2));
        assert!(query::bug_view(simulation.world()).into_vec().is_empty());

        simulation.handle_input(FrameInput {
            mode_toggle: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        simulation.advance(Duration::from_secs(1));

        assert!(query::bug_view(simulation.world()).into_vec().is_empty());
    }

    #[test]
    fn spawn_wave_switches_to_attack_and_records_plan() {
        let mut simulation = new_simulation();

        simulation.handle_input(FrameInput {
            spawn_wave: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);

        assert_eq!(query::play_mode(simulation.world()), PlayMode::Attack);
        let plan = simulation
            .active_wave_plan()
            .expect("wave plan should be recorded");
        assert_eq!(plan.bursts().len(), 1);
        let burst = &plan.bursts()[0];
        assert_eq!(burst.count().get(), BASIC_WAVE_BUGS);
        assert_eq!(burst.cadence_ms().get(), BASIC_WAVE_CADENCE_MS);
        assert_eq!(burst.start_ms(), BASIC_WAVE_START_DELAY_MS);
        assert_eq!(burst.bug().color(), BASIC_WAVE_COLOR);
        assert_eq!(burst.bug().health(), BASIC_WAVE_HEALTH);
    }

    #[test]
    fn spawn_wave_emits_basic_wave_over_time() {
        let mut simulation = new_simulation();

        simulation.handle_input(FrameInput {
            spawn_wave: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);

        let mut spawned = simulation
            .last_frame_events()
            .iter()
            .filter(|event| matches!(event, Event::BugSpawned { .. }))
            .count();

        let cadence = Duration::from_millis(u64::from(BASIC_WAVE_CADENCE_MS));
        for _ in 0..(BASIC_WAVE_BUGS * 8) {
            if spawned > 0 {
                break;
            }
            simulation.advance(cadence);
            spawned += simulation
                .last_frame_events()
                .iter()
                .filter(|event| matches!(event, Event::BugSpawned { .. }))
                .count();
        }

        assert!(spawned > 0, "wave should spawn at least one bug");
        assert!(
            spawned as u32 <= BASIC_WAVE_BUGS,
            "wave should not spawn more bugs than planned"
        );
    }

    #[test]
    fn round_loss_clears_active_wave_state() {
        let mut simulation = new_simulation();
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("expected at least one bug spawner");
        let plan = simulation.basic_attack_plan(spawner);
        simulation.active_wave = Some(WaveState::new(plan));
        assert!(simulation.active_wave.is_some(), "wave should be active");

        simulation
            .pending_events
            .push(Event::RoundLost { bug: BugId::new(7) });
        let _ = simulation.process_pending_events(None, TowerBuilderInput::default());

        assert!(
            simulation.active_wave.is_none(),
            "round loss should clear wave"
        );
    }

    #[test]
    fn round_win_queues_resolution_and_blocks_new_wave_until_applied() {
        let mut simulation = new_simulation();
        let initial_tier = query::difficulty_tier(simulation.world());

        enter_builder_mode(&mut simulation);
        let first_tile = TileSpacePosition::from_indices(1, 1);
        let first_origin = simulation.tile_position_to_cell(first_tile);
        simulation.queued_commands.push(Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: first_origin,
        });
        let second_tile = TileSpacePosition::from_indices(2, 1);
        let second_origin = simulation.tile_position_to_cell(second_tile);
        simulation.queued_commands.push(Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: second_origin,
        });
        simulation.queued_commands.push(Command::ConfigureBugStep {
            step_duration: Duration::from_millis(800),
        });
        simulation.advance(Duration::ZERO);
        let gold_after_build = query::gold(simulation.world());
        assert_eq!(
            query::towers(simulation.world()).into_vec().len(),
            2,
            "two towers should be placed before the wave starts",
        );

        simulation.handle_input(FrameInput {
            spawn_wave: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);

        let step = Duration::from_millis(100);
        let mut resolve_command_detected = false;
        for _ in 0..2000 {
            simulation.advance(step);
            if simulation.queued_commands().iter().any(|command| {
                matches!(
                    command,
                    Command::ResolveRound {
                        outcome: RoundOutcome::Win,
                    }
                )
            }) {
                resolve_command_detected = true;
                break;
            }
        }

        let remaining_bugs = query::bug_view(simulation.world()).into_vec().len();
        let wave_active = simulation.active_wave.is_some();
        assert!(
            resolve_command_detected,
            "wave win should queue resolution command (queued: {:?}, last events: {:?}, remaining bugs: {}, wave active: {})",
            simulation.queued_commands(),
            simulation.last_frame_events(),
            remaining_bugs,
            wave_active,
        );
        assert!(
            simulation.pending_outcome_command,
            "queued resolution should mark outcome command pending",
        );

        let gold_after_wave = query::gold(simulation.world());
        let expected_reward =
            Gold::new(BASIC_WAVE_BUGS.saturating_mul(initial_tier.saturating_add(1)));
        assert_eq!(
            gold_after_wave,
            gold_after_build.saturating_add(expected_reward),
            "bug kills should award gold scaled by tier",
        );

        simulation.handle_input(FrameInput {
            spawn_wave: true,
            ..FrameInput::default()
        });
        assert!(
            simulation.active_wave.is_none(),
            "new wave should not start while resolution is pending",
        );

        simulation.advance(Duration::ZERO);

        let final_tier = query::difficulty_tier(simulation.world());
        assert_eq!(
            final_tier,
            initial_tier.saturating_add(1),
            "round win should increment difficulty tier",
        );
        let final_gold = query::gold(simulation.world());
        assert_eq!(
            final_gold, gold_after_wave,
            "resolving the round should preserve accumulated gold",
        );
        assert!(
            simulation.queued_commands().is_empty(),
            "queued commands should be flushed after resolution",
        );
        assert!(
            !simulation.pending_outcome_command,
            "pending outcome flag should clear once resolution applies",
        );

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        let presented_gold = scene
            .gold
            .expect("scene should include gold presentation")
            .amount();
        assert_eq!(
            presented_gold, final_gold,
            "scene presentation should reflect resolved gold",
        );
        let presented_tier = scene
            .tier
            .expect("scene should include tier presentation")
            .tier();
        assert_eq!(
            presented_tier, final_tier,
            "scene presentation should reflect resolved tier",
        );

        simulation.handle_input(FrameInput {
            spawn_wave: true,
            ..FrameInput::default()
        });
        simulation.advance(Duration::ZERO);
        assert!(
            simulation.active_wave.is_some(),
            "new wave should start once resolution completes",
        );
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
            step_ms: simulation.bug_step_ms(),
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
            step_ms: simulation.bug_step_ms(),
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
            step_ms: simulation.bug_step_ms(),
        });
        simulation.queued_commands.push(Command::SpawnBug {
            spawner: second_spawner,
            color: BugColor::from_rgb(0, 255, 0),
            health: Health::new(3),
            step_ms: simulation.bug_step_ms(),
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
            step_ms: simulation.bug_step_ms(),
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
            step_ms: simulation.bug_step_ms(),
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

    #[test]
    fn sprite_scene_population_is_deterministic() {
        let first = capture_scripted_scene(VisualStyle::Sprites);
        let second = capture_scripted_scene(VisualStyle::Sprites);

        assert_eq!(first, second, "sprite scenes diverged between runs");
        assert!(
            !first.towers.is_empty() && !first.bugs.is_empty(),
            "scripted scene should contain at least one tower and bug"
        );
        assert_eq!(
            first.tower_targets.len(),
            1,
            "expected a single tower target"
        );

        let first_fingerprint = scene_fingerprint(&first);
        let second_fingerprint = scene_fingerprint(&second);
        assert_eq!(first_fingerprint, second_fingerprint);

        let expected = 0xe425_7ce6_4f4f_b7dc;
        assert_eq!(
            first_fingerprint, expected,
            "sprite scene fingerprint mismatch: {first_fingerprint:#x}"
        );

        assert!(matches!(first.towers[0].visual, TowerVisual::Sprite { .. }));
        assert!(matches!(first.bugs[0].style, BugVisual::Sprite(_)));
    }

    #[test]
    fn primitive_scene_population_is_deterministic() {
        let first = capture_scripted_scene(VisualStyle::Primitives);
        let second = capture_scripted_scene(VisualStyle::Primitives);

        assert_eq!(first, second, "primitive scenes diverged between runs");
        assert!(
            !first.towers.is_empty() && !first.bugs.is_empty(),
            "scripted scene should contain at least one tower and bug"
        );
        assert_eq!(
            first.tower_targets.len(),
            1,
            "expected a single tower target"
        );

        let first_fingerprint = scene_fingerprint(&first);
        let second_fingerprint = scene_fingerprint(&second);
        assert_eq!(first_fingerprint, second_fingerprint);

        let expected = 0x52ac_8d62_88e0_a775;
        assert_eq!(
            first_fingerprint, expected,
            "primitive scene fingerprint mismatch: {first_fingerprint:#x}"
        );

        assert!(matches!(first.towers[0].visual, TowerVisual::PrimitiveRect));
        match first.bugs[0].style {
            BugVisual::PrimitiveCircle { .. } => {}
            BugVisual::Sprite(_) => {
                panic!("primitive style should emit circle visuals");
            }
        }
    }

    fn capture_scripted_scene(style: VisualStyle) -> Scene {
        let mut simulation = new_simulation_with_style(style);
        let spawner = query::bug_spawners(simulation.world())
            .into_iter()
            .next()
            .expect("bug spawner available");

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
            color: BugColor::from_rgb(160, 120, 80),
            health: Health::new(3),
            step_ms: simulation.bug_step_ms(),
        });
        simulation.advance(Duration::ZERO);

        simulation.advance(Duration::from_millis(16));

        let mut scene = make_scene();
        simulation.populate_scene(&mut scene);
        scene
    }

    fn scene_fingerprint(scene: &Scene) -> u64 {
        let digest = SceneDigest::from(scene);
        let mut hasher = DefaultHasher::new();
        digest.hash(&mut hasher);
        hasher.finish()
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct SceneDigest {
        tile_grid: TileGridDigest,
        wall_color: ColorDigest,
        ground: Option<GroundSpriteTilesDigest>,
        walls: Vec<SceneWall>,
        bugs: Vec<BugDigest>,
        towers: Vec<TowerDigest>,
        projectiles: Vec<ProjectileDigest>,
        tower_targets: Vec<TowerTargetDigest>,
        hovered_tower: Option<TowerId>,
        play_mode: PlayMode,
        tower_preview: Option<TowerPreview>,
        active_tower_footprint_tiles: Option<Vec2Digest>,
        tower_feedback: Option<TowerInteractionFeedback>,
    }

    impl From<&Scene> for SceneDigest {
        fn from(scene: &Scene) -> Self {
            Self {
                tile_grid: TileGridDigest::from(scene.tile_grid),
                wall_color: ColorDigest::from(scene.wall_color),
                ground: scene.ground.as_ref().map(GroundSpriteTilesDigest::from),
                walls: scene.walls.clone(),
                bugs: scene.bugs.iter().map(BugDigest::from).collect(),
                towers: scene.towers.iter().map(TowerDigest::from).collect(),
                projectiles: scene
                    .projectiles
                    .iter()
                    .map(ProjectileDigest::from)
                    .collect(),
                tower_targets: scene
                    .tower_targets
                    .iter()
                    .map(TowerTargetDigest::from)
                    .collect(),
                hovered_tower: scene.hovered_tower,
                play_mode: scene.play_mode,
                tower_preview: scene.tower_preview,
                active_tower_footprint_tiles: scene
                    .active_tower_footprint_tiles
                    .map(Vec2Digest::from),
                tower_feedback: scene.tower_feedback,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct GroundSpriteTilesDigest {
        kind: GroundKind,
        sprite: SpriteInstanceDigest,
    }

    impl From<&GroundSpriteTiles> for GroundSpriteTilesDigest {
        fn from(tiles: &GroundSpriteTiles) -> Self {
            Self {
                kind: tiles.kind,
                sprite: SpriteInstanceDigest::from(&tiles.sprite),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct TileGridDigest {
        columns: u32,
        rows: u32,
        tile_length: u32,
        cells_per_tile: u32,
        line_color: ColorDigest,
    }

    impl From<TileGridPresentation> for TileGridDigest {
        fn from(grid: TileGridPresentation) -> Self {
            Self {
                columns: grid.columns,
                rows: grid.rows,
                tile_length: grid.tile_length.to_bits(),
                cells_per_tile: grid.cells_per_tile,
                line_color: ColorDigest::from(grid.line_color),
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct ColorDigest {
        red: u32,
        green: u32,
        blue: u32,
        alpha: u32,
    }

    impl From<Color> for ColorDigest {
        fn from(color: Color) -> Self {
            Self {
                red: color.red.to_bits(),
                green: color.green.to_bits(),
                blue: color.blue.to_bits(),
                alpha: color.alpha.to_bits(),
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct Vec2Digest {
        x: u32,
        y: u32,
    }

    impl From<Vec2> for Vec2Digest {
        fn from(value: Vec2) -> Self {
            Self {
                x: value.x.to_bits(),
                y: value.y.to_bits(),
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct CellRectDigest {
        origin_column: u32,
        origin_row: u32,
        width: u32,
        height: u32,
    }

    impl From<CellRect> for CellRectDigest {
        fn from(rect: CellRect) -> Self {
            Self {
                origin_column: rect.origin().column(),
                origin_row: rect.origin().row(),
                width: rect.size().width(),
                height: rect.size().height(),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct SpriteInstanceDigest {
        sprite: SpriteKey,
        size: Vec2Digest,
        pivot: Vec2Digest,
        rotation: u32,
        offset: Option<Vec2Digest>,
    }

    impl From<&SpriteInstance> for SpriteInstanceDigest {
        fn from(instance: &SpriteInstance) -> Self {
            Self {
                sprite: instance.sprite,
                size: Vec2Digest::from(instance.size),
                pivot: Vec2Digest::from(instance.pivot),
                rotation: instance.rotation_radians.to_bits(),
                offset: instance.offset.map(Vec2Digest::from),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    enum TowerVisualDigest {
        PrimitiveRect,
        Sprite {
            base: SpriteInstanceDigest,
            turret: SpriteInstanceDigest,
        },
    }

    impl From<&TowerVisual> for TowerVisualDigest {
        fn from(visual: &TowerVisual) -> Self {
            match visual {
                TowerVisual::PrimitiveRect => Self::PrimitiveRect,
                TowerVisual::Sprite { base, turret } => Self::Sprite {
                    base: SpriteInstanceDigest::from(base),
                    turret: SpriteInstanceDigest::from(turret),
                },
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct TowerDigest {
        id: TowerId,
        kind: TowerKind,
        region: CellRectDigest,
        visual: TowerVisualDigest,
    }

    impl From<&SceneTower> for TowerDigest {
        fn from(tower: &SceneTower) -> Self {
            Self {
                id: tower.id,
                kind: tower.kind,
                region: CellRectDigest::from(tower.region),
                visual: TowerVisualDigest::from(&tower.visual),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    enum BugVisualDigest {
        PrimitiveCircle { color: ColorDigest },
        Sprite(SpriteInstanceDigest),
    }

    impl From<&BugVisual> for BugVisualDigest {
        fn from(visual: &BugVisual) -> Self {
            match visual {
                BugVisual::PrimitiveCircle { color } => Self::PrimitiveCircle {
                    color: ColorDigest::from(*color),
                },
                BugVisual::Sprite(instance) => Self::Sprite(SpriteInstanceDigest::from(instance)),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct BugDigest {
        id: BugId,
        column: u32,
        row: u32,
        offset: Vec2Digest,
        style: BugVisualDigest,
        health: (u32, u32),
    }

    impl From<&BugPresentation> for BugDigest {
        fn from(bug: &BugPresentation) -> Self {
            Self {
                id: bug.id,
                column: bug.column,
                row: bug.row,
                offset: Vec2Digest::from(bug.offset),
                style: BugVisualDigest::from(&bug.style),
                health: (bug.health.current, bug.health.maximum),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ProjectileDigest {
        id: ProjectileId,
        from: Vec2Digest,
        to: Vec2Digest,
        position: Vec2Digest,
        progress: u32,
    }

    impl From<&SceneProjectile> for ProjectileDigest {
        fn from(projectile: &SceneProjectile) -> Self {
            Self {
                id: projectile.id,
                from: Vec2Digest::from(projectile.from),
                to: Vec2Digest::from(projectile.to),
                position: Vec2Digest::from(projectile.position),
                progress: projectile.progress.to_bits(),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct TowerTargetDigest {
        tower: TowerId,
        bug: BugId,
        from: Vec2Digest,
        to: Vec2Digest,
    }

    impl From<&TowerTargetLine> for TowerTargetDigest {
        fn from(line: &TowerTargetLine) -> Self {
            Self {
                tower: line.tower,
                bug: line.bug,
                from: Vec2Digest::from(line.from),
                to: Vec2Digest::from(line.to),
            }
        }
    }
}
