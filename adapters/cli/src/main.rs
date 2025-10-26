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
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    convert::TryFrom,
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
    BugColor, BugId, BugView, CellCoord, CellPointHalf, CellRect, CellRectSize, Command,
    DifficultyLevel, Event, Gold, Health, PendingWaveDifficulty, PlacementError, PlayMode,
    PressureWaveInputs, PressureWavePlan, ProjectileSnapshot, RemovalError, RoundOutcome,
    SpawnPatchId, SpeciesId, SpeciesPrototype, SpeciesTableVersion, TileCoord, TowerCooldownView,
    TowerId, TowerKind, TowerTarget, WaveDifficulty, WaveId,
};
use maze_defence_rendering::{
    visuals, BugHealthPresentation, BugPresentation, BugVisual, Color, ControlPanelView,
    DifficultyButtonPresentation, DifficultyPresentation, DifficultySelectionPresentation,
    FrameInput, FrameSimulationBreakdown, GoldPresentation, GroundKind, GroundSpriteTiles,
    Presentation, RenderingBackend, Scene, SceneProjectile, SceneTower, SceneWall, SpawnEffect,
    SpriteKey, TileGridPresentation, TileSpacePosition, TowerInteractionFeedback, TowerPreview,
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
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const DEFAULT_GRID_COLUMNS: u32 = 10;
const DEFAULT_GRID_ROWS: u32 = 10;
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_BUG_STEP_MS: u64 = 250;
const DEFAULT_BUG_SPAWN_INTERVAL_MS: u64 = 1_000;
const SPAWN_RNG_SEED: u64 = 0x4d59_5df4_d0f3_3173;
const TILE_LENGTH_TOLERANCE: f32 = 1e-3;
const DEFAULT_BUG_HEADING: f32 = 0.0;
const GROUND_TILE_MULTIPLIER: f32 = 4.0;
const MIN_SPAWN_BAND: usize = 5;
const MAX_SPAWN_BAND: usize = 10;
const SPAWN_BAND_FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const SPAWN_BAND_FNV_PRIME: u64 = 0x0000_0001_0000_01b3;
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

fn parse_difficulty_level(value: &str) -> std::result::Result<DifficultyLevel, String> {
    value
        .parse::<u32>()
        .map(DifficultyLevel::new)
        .map_err(|error| format!("invalid difficulty: {error}"))
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
    /// Sets the base difficulty level active when the simulation launches.
    #[arg(long = "difficulty", value_name = "LEVEL", value_parser = parse_difficulty_level)]
    difficulty: Option<DifficultyLevel>,
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
    let initial_difficulty = args.difficulty;
    let mut simulation = Simulation::new(
        columns,
        rows,
        DEFAULT_TILE_LENGTH,
        args.cells_per_tile,
        bug_step_duration,
        bug_spawn_interval,
        args.visual_style,
        initial_difficulty,
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
        Vec::new(),
        None,
        query::play_mode(simulation.world()),
        None,
        None,
        None,
        Some(ControlPanelView::new(200.0, Color::from_rgb_u8(0, 0, 0))),
        Some(GoldPresentation::new(query::gold(simulation.world()))),
        Some(DifficultyPresentation::new(
            query::difficulty_level(simulation.world()).get(),
        )),
        None,
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
    difficulty_level: DifficultyLevel,
    pending_wave_difficulty: PendingWaveDifficulty,
    pending_wave_launch: Option<PendingWaveLaunch>,
    last_placement_rejection: Option<PlacementRejection>,
    last_removal_rejection: Option<RemovalRejection>,
    bug_step_duration: Duration,
    bug_motions: HashMap<BugId, BugMotion>,
    bug_headings: HashMap<BugId, f32>,
    cells_per_tile: u32,
    species_table_version: SpeciesTableVersion,
    species_prototypes: HashMap<SpeciesId, SpeciesPrototype>,
    patch_origins: HashMap<SpawnPatchId, CellCoord>,
    visual_style: VisualStyle,
    last_advance_profile: AdvanceProfile,
    last_announced_play_mode: PlayMode,
    active_wave: Option<WaveState>,
    active_wave_plan: Option<PressureWavePlan>,
    ready_wave_launches: VecDeque<ReadyWaveLaunch>,
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
    color: BugColor,
    health: Health,
    step_ms: NonZeroU32,
}

impl ScheduledSpawn {
    #[allow(dead_code)]
    fn new(
        at: Duration,
        spawner: CellCoord,
        color: BugColor,
        health: Health,
        step_ms: NonZeroU32,
    ) -> Self {
        Self {
            at,
            spawner,
            color,
            health,
            step_ms,
        }
    }
}

#[derive(Clone, Debug)]
struct WaveState {
    scheduled: Vec<ScheduledSpawn>,
    next_spawn: usize,
    elapsed: Duration,
}

impl WaveState {
    #[allow(dead_code)]
    fn new(
        plan: &PressureWavePlan,
        species: &HashMap<SpeciesId, SpeciesPrototype>,
        spawners: &[CellCoord],
        seed: u64,
    ) -> Self {
        if plan.spawns().is_empty() || spawners.is_empty() {
            return Self {
                scheduled: Vec::new(),
                next_spawn: 0,
                elapsed: Duration::ZERO,
            };
        }

        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut ordered_spawners: Vec<CellCoord> = spawners.iter().copied().collect();
        ordered_spawners.sort_by_key(|coord| (coord.row(), coord.column()));

        let spawner_count = ordered_spawners.len();
        if spawner_count == 0 {
            return Self {
                scheduled: Vec::new(),
                next_spawn: 0,
                elapsed: Duration::ZERO,
            };
        }

        let mut species_ids = BTreeSet::new();
        for spawn in plan.spawns() {
            let _ = species_ids.insert(SpeciesId::new(spawn.species_id()));
        }
        if species_ids.is_empty() {
            return Self {
                scheduled: Vec::new(),
                next_spawn: 0,
                elapsed: Duration::ZERO,
            };
        }

        let mut next_band_start = if spawner_count == 0 {
            0
        } else {
            rng.gen_range(0..spawner_count)
        };

        #[derive(Clone, Debug)]
        struct BandState {
            cells: Vec<CellCoord>,
            cursor: usize,
        }

        let mut bands: HashMap<SpeciesId, BandState> = HashMap::new();

        for species_id in &species_ids {
            let max_band = MAX_SPAWN_BAND.min(spawner_count).max(1);
            let min_band = MIN_SPAWN_BAND.min(spawner_count).max(1);
            let band_len = if min_band >= max_band {
                min_band
            } else {
                rng.gen_range(min_band..=max_band)
            };

            let mut cells = Vec::with_capacity(band_len);
            if spawner_count == band_len {
                cells.extend(ordered_spawners.iter().copied());
            } else {
                for offset in 0..band_len {
                    let index = (next_band_start + offset) % spawner_count;
                    cells.push(ordered_spawners[index]);
                }
            }

            let band_state = BandState { cells, cursor: 0 };
            let _ = bands.insert(*species_id, band_state);
            if spawner_count > 0 {
                next_band_start = (next_band_start + band_len) % spawner_count;
            }
        }

        let mut availability: HashMap<CellCoord, Duration> = HashMap::new();
        for cell in &ordered_spawners {
            let _ = availability.insert(*cell, Duration::ZERO);
        }

        let mut scheduled = Vec::with_capacity(plan.spawns().len());
        for (index, spawn) in plan.spawns().iter().enumerate() {
            let species_id = SpeciesId::new(spawn.species_id());
            let band = if bands.contains_key(&species_id) {
                bands
                    .get_mut(&species_id)
                    .expect("species band should exist once assigned")
            } else {
                let fallback_id = *bands
                    .keys()
                    .next()
                    .expect("at least one band must be registered");
                bands
                    .get_mut(&fallback_id)
                    .expect("fallback band should exist")
            };
            let cell = if band.cells.is_empty() {
                ordered_spawners[index % spawner_count]
            } else {
                let cell = band.cells[band.cursor % band.cells.len()];
                band.cursor = band.cursor.wrapping_add(1);
                cell
            };

            let prototype = species
                .get(&species_id)
                .copied()
                .or_else(|| {
                    usize::try_from(species_id.get())
                        .ok()
                        .and_then(|index| plan.prototypes().get(index))
                        .copied()
                })
                .or_else(|| species.values().copied().next())
                .or_else(|| plan.prototypes().first().copied())
                .unwrap_or_else(|| {
                    SpeciesPrototype::new(
                        BugColor::from_rgb(0xff, 0xff, 0xff),
                        Health::new(spawn.hp()),
                        NonZeroU32::new(1).expect("non-zero fallback step"),
                    )
                });

            let step_ms = resolve_step_ms(prototype.step_ms(), spawn.speed_mult());
            let color = prototype.color();
            let health = Health::new(spawn.hp());
            let planned_at = Duration::from_millis(u64::from(spawn.time_ms()));
            let ready_at = availability.get(&cell).copied().unwrap_or_default();
            let scheduled_at = planned_at.max(ready_at);
            let cooldown = Duration::from_millis(u64::from(step_ms.get()));
            let _ = availability.insert(cell, scheduled_at.saturating_add(cooldown));

            scheduled.push((
                index,
                ScheduledSpawn {
                    at: scheduled_at,
                    spawner: cell,
                    color,
                    health,
                    step_ms,
                },
            ));
        }

        scheduled.sort_by(|left, right| {
            let time_order = left.1.at.cmp(&right.1.at);
            if time_order != std::cmp::Ordering::Equal {
                return time_order;
            }
            let cell_left = (left.1.spawner.row(), left.1.spawner.column());
            let cell_right = (right.1.spawner.row(), right.1.spawner.column());
            let cell_order = cell_left.cmp(&cell_right);
            if cell_order != std::cmp::Ordering::Equal {
                return cell_order;
            }
            left.0.cmp(&right.0)
        });

        let scheduled = scheduled.into_iter().map(|(_, spawn)| spawn).collect();

        Self {
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
                color: spawn.color,
                health: spawn.health,
                step_ms: spawn.step_ms.get(),
            });
            self.next_spawn += 1;
        }
    }

    fn pending_spawn_effects(&self) -> Vec<(CellCoord, BugColor)> {
        let mut effects = BTreeMap::new();
        for spawn in self.scheduled.iter().skip(self.next_spawn) {
            let key = (spawn.spawner.row(), spawn.spawner.column());
            let _ = effects.entry(key).or_insert((spawn.spawner, spawn.color));
        }
        effects.into_values().collect()
    }

    fn finished(&self) -> bool {
        self.next_spawn >= self.scheduled.len()
    }
}

#[derive(Clone, Debug)]
struct PendingWaveLaunch {
    inputs: PressureWaveInputs,
    wave: WaveId,
    difficulty: WaveDifficulty,
}

#[derive(Clone, Debug)]
struct ReadyWaveLaunch {
    inputs: PressureWaveInputs,
    wave: WaveId,
    difficulty: WaveDifficulty,
    plan: PressureWavePlan,
}

fn resolve_step_ms(base: NonZeroU32, speed_mult: f32) -> NonZeroU32 {
    let baseline = base.get().max(1);
    let multiplier = if speed_mult.is_finite() && speed_mult > f32::EPSILON {
        speed_mult
    } else {
        1.0
    };
    let adjusted = (baseline as f32 / multiplier).round() as u32;
    NonZeroU32::new(adjusted.max(1)).expect("non-zero step duration")
}

fn spawn_band_seed(inputs: &PressureWaveInputs) -> u64 {
    fn fnv1a(mut state: u64, bytes: &[u8]) -> u64 {
        for byte in bytes {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(SPAWN_BAND_FNV_PRIME);
        }
        state
    }

    let mut hash = SPAWN_BAND_FNV_OFFSET_BASIS;
    hash = fnv1a(hash, &inputs.game_seed().to_le_bytes());
    hash = fnv1a(hash, &inputs.level_id().get().to_le_bytes());
    hash = fnv1a(hash, &inputs.wave().get().to_le_bytes());
    fnv1a(hash, &inputs.difficulty().get().to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{DifficultyLevel, LevelId, PressureSpawnRecord, WaveDifficulty};
    use std::{collections::HashSet, time::Duration};

    fn species_proto(color: BugColor, health: u32, step_ms: u32) -> SpeciesPrototype {
        SpeciesPrototype::new(
            color,
            Health::new(health),
            NonZeroU32::new(step_ms.max(1)).expect("non-zero step"),
        )
    }

    fn build_plan(
        spawn_count: usize,
        species: u32,
        spacing: u32,
        prototype: SpeciesPrototype,
    ) -> PressureWavePlan {
        let mut spawns = Vec::with_capacity(spawn_count);
        for index in 0..spawn_count {
            spawns.push(PressureSpawnRecord::new(
                spacing.saturating_mul(u32::try_from(index).unwrap_or(0)),
                10,
                1.0,
                species,
            ));
        }
        let mut prototypes = Vec::new();
        let index = usize::try_from(species).unwrap_or(0);
        if prototypes.len() <= index {
            prototypes.resize(
                index + 1,
                SpeciesPrototype::new(
                    BugColor::from_rgb(0xff, 0xff, 0xff),
                    Health::new(1),
                    NonZeroU32::new(1).expect("non-zero default step"),
                ),
            );
        }
        prototypes[index] = prototype;
        PressureWavePlan::new(spawns, prototypes)
    }

    fn band_spawners(count: u32) -> Vec<CellCoord> {
        (0..count).map(|column| CellCoord::new(column, 0)).collect()
    }

    #[test]
    fn species_uses_multiple_spawners() {
        let mut species = HashMap::new();
        let color = BugColor::from_rgb(0x12, 0x34, 0x56);
        let prototype = species_proto(color, 5, 500);
        let plan = build_plan(12, 0, 200, prototype);
        let _ = species.insert(SpeciesId::new(0), prototype);
        let spawners = band_spawners(20);

        let wave = WaveState::new(&plan, &species, &spawners, 0xfeed_beef);
        assert_eq!(wave.scheduled.len(), plan.spawns().len());

        let unique: BTreeSet<_> = wave.scheduled.iter().map(|spawn| spawn.spawner).collect();
        assert!(unique.len() >= MIN_SPAWN_BAND.min(spawners.len()));
    }

    #[test]
    fn respects_per_spawner_cadence() {
        let mut species = HashMap::new();
        let prototype = species_proto(BugColor::from_rgb(1, 2, 3), 5, 400);
        let plan = build_plan(30, 0, 10, prototype);
        let _ = species.insert(SpeciesId::new(0), prototype);
        let spawners = band_spawners(5);

        let wave = WaveState::new(&plan, &species, &spawners, 0x1234_5678);

        let mut per_cell: HashMap<CellCoord, Vec<(Duration, u32)>> = HashMap::new();
        for spawn in &wave.scheduled {
            per_cell
                .entry(spawn.spawner)
                .or_default()
                .push((spawn.at, spawn.step_ms.get()));
        }

        for entries in per_cell.values_mut() {
            if entries.len() < 2 {
                continue;
            }
            entries.sort_by_key(|entry| entry.0);
            for window in entries.windows(2) {
                let previous = window[0];
                let next = window[1];
                let required_gap = Duration::from_millis(u64::from(previous.1));
                assert!(next.0 >= previous.0 + required_gap);
            }
        }
    }

    #[test]
    fn deterministic_band_assignment() {
        let mut species = HashMap::new();
        let prototype = species_proto(BugColor::from_rgb(5, 6, 7), 9, 360);
        let plan = build_plan(16, 0, 120, prototype);
        let _ = species.insert(SpeciesId::new(0), prototype);
        let spawners = band_spawners(12);

        let left = WaveState::new(&plan, &species, &spawners, 0x77aa_bbcc);
        let right = WaveState::new(&plan, &species, &spawners, 0x77aa_bbcc);

        assert_eq!(left.scheduled.len(), right.scheduled.len());
        for (lhs, rhs) in left.scheduled.iter().zip(right.scheduled.iter()) {
            assert_eq!(lhs.spawner, rhs.spawner);
            assert_eq!(lhs.at, rhs.at);
            assert_eq!(lhs.step_ms, rhs.step_ms);
        }
    }

    #[test]
    fn wave_state_uses_plan_prototypes_for_missing_species() {
        let prototype_a = species_proto(BugColor::from_rgb(0x10, 0x20, 0x30), 7, 420);
        let prototype_b = species_proto(BugColor::from_rgb(0x40, 0x50, 0x60), 9, 360);
        let spawns = vec![
            PressureSpawnRecord::new(0, 10, 1.0, 0),
            PressureSpawnRecord::new(100, 12, 1.0, 1),
        ];
        let plan = PressureWavePlan::new(spawns, vec![prototype_a, prototype_b]);
        let mut species = HashMap::new();
        let _ = species.insert(SpeciesId::new(0), prototype_a);
        let spawners = band_spawners(4);

        let wave = WaveState::new(&plan, &species, &spawners, 0xfeed_face);
        let colors: HashSet<_> = wave.scheduled.iter().map(|spawn| spawn.color).collect();
        assert!(colors.contains(&prototype_a.color()));
        assert!(colors.contains(&prototype_b.color()));
    }

    #[test]
    fn spawn_effect_preview_uses_ready_launch_plan() {
        let mut simulation = Simulation::new(
            4,
            4,
            48.0,
            1,
            Duration::from_millis(400),
            Duration::from_millis(1_000),
            VisualStyle::Primitives,
            None,
        );
        simulation.ready_wave_launches.clear();
        simulation.active_wave = None;
        simulation.active_wave_plan = None;

        let tint = BugColor::from_rgb(0xaa, 0xbb, 0xcc);
        simulation.species_prototypes.clear();
        let prototype = species_proto(tint, 12, 360);
        let _ = simulation
            .species_prototypes
            .insert(SpeciesId::new(1), prototype);

        let inputs = PressureWaveInputs::new(
            0x1234_5678,
            LevelId::new(2),
            WaveId::new(3),
            DifficultyLevel::new(4),
        );
        let plan = build_plan(8, 1, 90, prototype);
        simulation.ready_wave_launches.push_back(ReadyWaveLaunch {
            inputs,
            wave: WaveId::new(3),
            difficulty: WaveDifficulty::Normal,
            plan,
        });

        let effects = simulation.spawn_effects();
        assert!(!effects.is_empty());
        let expected = Color::from_rgb_u8(tint.red(), tint.green(), tint.blue());
        let effect = effects
            .first()
            .expect("at least one spawn effect should be generated");
        assert!((effect.color.red - expected.red).abs() <= f32::EPSILON);
        assert!((effect.color.green - expected.green).abs() <= f32::EPSILON);
        assert!((effect.color.blue - expected.blue).abs() <= f32::EPSILON);
    }

    #[test]
    fn layout_import_bypasses_gold_costs() {
        let snapshot = TowerLayoutSnapshot::decode("maze:v2:10x10:BAAAyEJDABMlAA8jAAklAA0ZABEdABUfABkjAB8lAB8hABsdABcZABMXAA8TAAkVAAkRAA0NABENABURABkTAB0XAB8RACMVACMZAB8dABsNABcLABMHAA8HAAsHAAcLAAUBAAkBAA0BABEBABUBABkBAB0BACEBACUBACUFACUJACUNACURACUdACUhACUlACEJACEFABsJABcHAB8NAB0FAAcHAA0dAAEBAAEFAAEJAAENAAEVAAkdAAEZAAEdAAchAAcZAAMRAAUlAAEj")
            .expect("snapshot should decode");

        let mut simulation = Simulation::new(
            snapshot.columns,
            snapshot.rows,
            snapshot.tile_length,
            snapshot.cells_per_tile,
            Duration::from_millis(DEFAULT_BUG_STEP_MS),
            Duration::from_millis(DEFAULT_BUG_SPAWN_INTERVAL_MS),
            VisualStyle::Primitives,
            None,
        );

        let initial_gold = query::gold(simulation.world());
        let required_gold: u32 = snapshot
            .towers
            .iter()
            .map(|tower| tower.kind.build_cost().get())
            .sum();
        assert!(required_gold > initial_gold.get());

        simulation
            .apply_layout_snapshot(&snapshot)
            .expect("layout restoration should succeed");

        let restored = query::towers(simulation.world());
        assert_eq!(restored.iter().count(), snapshot.towers.len());
        assert_eq!(query::gold(simulation.world()), initial_gold);
    }

    #[test]
    fn initial_difficulty_level_applies_immediately() {
        let simulation = Simulation::new(
            4,
            4,
            48.0,
            1,
            Duration::from_millis(400),
            Duration::from_millis(1_000),
            VisualStyle::Primitives,
            Some(DifficultyLevel::new(7)),
        );

        assert_eq!(simulation.difficulty_level, DifficultyLevel::new(7));
        let emitted = simulation
            .last_frame_events
            .iter()
            .any(|event| matches!(event, Event::DifficultyLevelChanged { level } if *level == DifficultyLevel::new(7)));
        assert!(emitted, "difficulty change should emit an event");
    }
}

#[cfg_attr(test, allow(dead_code))]
impl Simulation {
    fn new(
        columns: u32,
        rows: u32,
        tile_length: f32,
        cells_per_tile: u32,
        bug_step: Duration,
        bug_spawn_interval: Duration,
        visual_style: VisualStyle,
        initial_difficulty: Option<DifficultyLevel>,
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

        if let Some(level) = initial_difficulty {
            world::apply(
                &mut world,
                Command::SetDifficultyLevel { level },
                &mut pending_events,
            );
        }

        let initial_play_mode = query::play_mode(&world);
        pending_events.push(Event::PlayModeChanged {
            mode: initial_play_mode,
        });
        let gold = query::gold(&world);
        let difficulty_level = query::difficulty_level(&world);
        let pending_wave_difficulty = query::pending_wave_difficulty(&world);
        let species_table = query::species_table(&world);
        let mut species_prototypes = HashMap::new();
        for definition in species_table.iter() {
            let _ = species_prototypes.insert(definition.id(), definition.prototype());
        }
        let species_table_version = species_table.version();
        let mut patch_origins = HashMap::new();
        for descriptor in query::patch_table(&world).iter() {
            let _ = patch_origins.insert(descriptor.id(), descriptor.origin());
        }
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
            difficulty_level,
            pending_wave_difficulty,
            pending_wave_launch: None,
            last_placement_rejection: None,
            last_removal_rejection: None,
            bug_step_duration: bug_step,
            bug_motions: HashMap::new(),
            bug_headings: HashMap::new(),
            cells_per_tile,
            species_table_version,
            species_prototypes,
            patch_origins,
            visual_style,
            last_advance_profile: AdvanceProfile::default(),
            last_announced_play_mode: initial_play_mode,
            active_wave: None,
            active_wave_plan: None,
            ready_wave_launches: VecDeque::new(),
            auto_spawn_enabled: false,
            pending_outcome_command: false,
            awaiting_round_resolution: false,
            #[cfg(test)]
            last_frame_events: Vec::new(),
        };
        simulation.refresh_species_and_patches();
        let _ = simulation.process_pending_events(None, TowerBuilderInput::default());
        simulation.builder_preview = simulation.compute_builder_preview();
        simulation.last_announced_play_mode = query::play_mode(&simulation.world);
        simulation
    }

    fn world(&self) -> &World {
        &self.world
    }

    #[cfg(test)]
    fn active_wave_plan(&self) -> Option<&PressureWavePlan> {
        self.active_wave_plan.as_ref()
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

        if let Some(difficulty) = input.start_wave {
            self.initiate_wave_launch(difficulty);
        }

        self.pending_input = FrameInput {
            mode_toggle: false,
            start_wave: None,
            ..input
        };
    }

    fn initiate_wave_launch(&mut self, difficulty: WaveDifficulty) {
        if query::play_mode(&self.world) != PlayMode::Attack {
            return;
        }

        if self.pending_wave_launch.is_some()
            || self.active_wave.is_some()
            || self.awaiting_round_resolution
        {
            return;
        }

        let context = query::wave_seed_context(&self.world);
        let level_id = query::level_id(&self.world);
        let base_difficulty = context.difficulty_level();
        let effective_level = match difficulty {
            WaveDifficulty::Normal => base_difficulty,
            WaveDifficulty::Hard => base_difficulty.saturating_add(1),
        };

        let inputs = PressureWaveInputs::new(
            context.global_seed(),
            level_id,
            context.wave(),
            effective_level,
        );

        self.pending_wave_launch = Some(PendingWaveLaunch {
            inputs: inputs.clone(),
            wave: context.wave(),
            difficulty,
        });
        self.queued_commands
            .push(Command::GeneratePressureWave { inputs });
    }

    fn record_attack_plan_events(&mut self, events: &[Event]) -> Vec<ReadyWaveLaunch> {
        for event in events {
            if let Event::PressureWaveReady { inputs, plan } = event {
                if let Some(pending) = &self.pending_wave_launch {
                    if pending.inputs == *inputs {
                        let launch = ReadyWaveLaunch {
                            inputs: inputs.clone(),
                            wave: pending.wave,
                            difficulty: pending.difficulty,
                            plan: plan.clone(),
                        };
                        self.ready_wave_launches.push_back(launch);
                        self.pending_wave_launch = None;
                        self.refresh_species_and_patches();
                    }
                }
            }
        }

        Vec::new()
    }

    fn take_ready_wave_launch(&mut self) -> Option<ReadyWaveLaunch> {
        self.ready_wave_launches.pop_front()
    }

    fn activate_wave(
        &mut self,
        launch: ReadyWaveLaunch,
        emitted_events: &mut Vec<Event>,
        next_events: &mut Vec<Event>,
    ) {
        let ReadyWaveLaunch {
            inputs,
            wave,
            difficulty,
            plan,
        } = launch;

        self.active_wave_plan = Some(plan);
        let plan_ref = self
            .active_wave_plan
            .as_ref()
            .expect("wave plan should be stored before activation");

        self.print_wave_launch_summary(wave, difficulty, plan_ref);

        let spawners = query::bug_spawners(&self.world);
        let band_seed = spawn_band_seed(&inputs);
        let wave_state = WaveState::new(plan_ref, &self.species_prototypes, &spawners, band_seed);
        self.active_wave = Some(wave_state);
        self.awaiting_round_resolution = true;
        self.pending_outcome_command = false;

        self.apply_command(Command::StartWave { wave, difficulty }, emitted_events);
        next_events.append(emitted_events);
    }

    fn print_wave_launch_summary(
        &self,
        wave: WaveId,
        difficulty: WaveDifficulty,
        plan: &PressureWavePlan,
    ) {
        if plan.prototypes().is_empty() {
            println!(
                "\n=== Wave {} ({:?}) ===\n  No species scheduled.\n",
                wave.get(),
                difficulty
            );
            return;
        }

        let mut counts = vec![0u32; plan.prototypes().len()];
        for spawn in plan.spawns() {
            if let Ok(index) = usize::try_from(spawn.species_id()) {
                if let Some(count) = counts.get_mut(index) {
                    *count += 1;
                }
            }
        }

        println!(
            "\n=== Wave {} ({:?}) ===\nSpecies breakdown:",
            wave.get(),
            difficulty
        );

        for (index, prototype) in plan.prototypes().iter().enumerate() {
            let color = prototype.color();
            let hp = prototype.health().get();
            let cadence_ms = prototype.step_ms().get();
            let steps_per_second = 1000.0 / (cadence_ms as f32);
            let count = counts.get(index).copied().unwrap_or_default();

            println!(
                "  • Species {index}\n    Bugs: {count}\n    HP: {hp}\n    Speed: {steps_per_second:.2} steps/s ({cadence_ms} ms cadence)\n    Color: #{:02X}{:02X}{:02X}",
                color.red(),
                color.green(),
                color.blue()
            );
        }

        println!();
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
            self.queued_commands.push(Command::ImportTower {
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
                self.refresh_species_and_patches();
            }
            Command::GeneratePressureWave { inputs } => {
                let command = Command::GeneratePressureWave { inputs };
                world::apply(&mut self.world, command, out_events);
                self.refresh_species_and_patches();
            }
            Command::CachePressureWave { inputs, plan } => {
                let command = Command::CachePressureWave { inputs, plan };
                world::apply(&mut self.world, command, out_events);
                self.refresh_species_and_patches();
            }
            other => {
                world::apply(&mut self.world, other, out_events);
            }
        }
    }

    fn refresh_species_and_patches(&mut self) {
        let table = query::species_table(&self.world);
        self.species_table_version = table.version();
        self.species_prototypes.clear();
        for definition in table.iter() {
            let _ = self
                .species_prototypes
                .insert(definition.id(), definition.prototype());
        }
        self.patch_origins.clear();
        for descriptor in query::patch_table(&self.world).iter() {
            let _ = self
                .patch_origins
                .insert(descriptor.id(), descriptor.origin());
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
            let bug_color = bug.color;
            let position = self.interpolated_bug_position_with_cell(bug.id, Some(bug.cell));
            let _ = bug_positions.insert(bug.id, position);
            let health = BugHealthPresentation::new(bug.health.get(), bug.max_health.get());
            let tint = Color::from_rgb_u8(bug_color.red(), bug_color.green(), bug_color.blue());

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
                    tint,
                    heading,
                );
                let BugVisual::Sprite { sprite, tint } = sprite_visual else {
                    unreachable!("bug sprite helper should return sprite visuals");
                };
                BugPresentation::new_sprite(bug.id, position, sprite, tint, health)
            } else {
                BugPresentation::new_circle(bug.id, position, tint, health)
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

        scene.spawn_effects.clear();
        scene.spawn_effects.extend(self.spawn_effects());

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
        scene.difficulty = Some(DifficultyPresentation::new(self.difficulty_level.get()));
        scene.difficulty_selection = Some(self.difficulty_selection_presentation());
    }

    fn spawn_effects(&self) -> Vec<SpawnEffect> {
        let effect_sources = if let Some(wave) = &self.active_wave {
            wave.pending_spawn_effects()
        } else if let Some(launch) = self.ready_wave_launches.front() {
            self.spawn_effect_cells_for_plan(&launch.plan, &launch.inputs)
        } else {
            Vec::new()
        };

        effect_sources
            .into_iter()
            .map(|(cell, color)| {
                SpawnEffect::new(
                    cell.column(),
                    cell.row(),
                    Color::from_rgb_u8(color.red(), color.green(), color.blue()),
                )
            })
            .collect()
    }

    fn spawn_effect_cells_for_plan(
        &self,
        plan: &PressureWavePlan,
        inputs: &PressureWaveInputs,
    ) -> Vec<(CellCoord, BugColor)> {
        if plan.spawns().is_empty() {
            return Vec::new();
        }

        let spawners = query::bug_spawners(&self.world);
        if spawners.is_empty() {
            return Vec::new();
        }

        let seed = spawn_band_seed(inputs);
        let preview = WaveState::new(plan, &self.species_prototypes, &spawners, seed);
        preview.pending_spawn_effects()
    }

    fn difficulty_selection_presentation(&self) -> DifficultySelectionPresentation {
        let (normal_selected, hard_selected) = match self.pending_wave_difficulty {
            PendingWaveDifficulty::Selected(WaveDifficulty::Normal) => (true, false),
            PendingWaveDifficulty::Selected(WaveDifficulty::Hard) => (false, true),
            PendingWaveDifficulty::Unset => (false, false),
        };

        let normal_level = self.difficulty_level.get();
        let hard_level = self.difficulty_level.saturating_add(1).get();
        let normal_multiplier = normal_level.saturating_add(1);
        let hard_multiplier = hard_level.saturating_add(1);

        DifficultySelectionPresentation::new(
            DifficultyButtonPresentation::new(
                WaveDifficulty::Normal,
                normal_selected,
                normal_level,
                normal_multiplier,
            ),
            DifficultyButtonPresentation::new(
                WaveDifficulty::Hard,
                hard_selected,
                hard_level,
                hard_multiplier,
            ),
        )
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

            let mut launches = self.record_attack_plan_events(&events);
            while let Some(launch) = self.take_ready_wave_launch() {
                launches.push(launch);
            }
            self.handle_bug_motion_events(&events);
            self.record_tower_feedback(&events);
            self.update_gold_from_events(&events);
            self.update_difficulty_level_from_events(&events);
            self.update_pending_wave_difficulty_from_events(&events);
            self.update_pressure_configuration_from_events(&events);

            for launch in launches {
                self.activate_wave(launch, &mut emitted_events, &mut next_events);
            }

            if events
                .iter()
                .any(|event| matches!(event, Event::RoundLost { .. }))
            {
                self.active_wave = None;
                self.active_wave_plan = None;
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
                self.active_wave_plan = None;
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

    fn update_difficulty_level_from_events(&mut self, events: &[Event]) {
        for event in events {
            if let Event::DifficultyLevelChanged { level } = event {
                self.difficulty_level = *level;
            }
        }
    }

    fn update_pending_wave_difficulty_from_events(&mut self, events: &[Event]) {
        for event in events {
            if let Event::PendingWaveDifficultyChanged { pending } = event {
                self.pending_wave_difficulty = *pending;
            }
        }
    }

    fn update_pressure_configuration_from_events(&mut self, events: &[Event]) {
        if events
            .iter()
            .any(|event| matches!(event, Event::PressureConfigChanged { .. }))
        {
            self.refresh_species_and_patches();
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
