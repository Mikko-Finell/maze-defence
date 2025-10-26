#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Authoritative world state management for Maze Defence.

mod navigation;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    convert::TryFrom,
    time::Duration,
};

#[cfg(any(test, feature = "tower_scaffolding"))]
use std::collections::VecDeque;

#[cfg(any(test, feature = "tower_scaffolding"))]
mod towers;

#[cfg(any(test, feature = "tower_scaffolding"))]
use towers::{footprint_for, TowerRegistry, TowerState};

use maze_defence_core::{
    BugColor, BugId, BurstGapRange, BurstSchedulingConfig, CadenceRange, CellCoord, CellPointHalf,
    CellRect, CellRectSize, Command, Damage, DifficultyLevel, Direction, DirichletWeight, Event,
    Gold, Health, LevelId, PendingWaveDifficulty, PlayMode, Pressure, PressureConfig,
    PressureCurve, PressureWaveInputs, PressureWavePlan, PressureWeight, ProjectileId,
    ReservationClaim, RoundOutcome, SpawnPatchDescriptor, SpawnPatchId, SpeciesDefinition,
    SpeciesId, SpeciesPrototype, SpeciesTableVersion, Target, TargetCell, TileCoord, TileGrid,
    WaveDifficulty, WaveId, PRESSURE_FIXED_POINT_SCALE, WELCOME_BANNER,
};

use maze_defence_pressure_v2::PressureV2;

#[cfg(any(test, feature = "tower_scaffolding"))]
use maze_defence_core::ProjectileRejection;

use maze_defence_core::structures::Wall as CellWall;

use navigation::NavigationField;

#[cfg(any(test, feature = "tower_scaffolding"))]
use maze_defence_core::{PlacementError, RemovalError, TowerKind};

use maze_defence_core::TowerId;

use std::num::NonZeroU32;

const DEFAULT_GRID_COLUMNS: TileCoord = TileCoord::new(10);
const DEFAULT_GRID_ROWS: TileCoord = TileCoord::new(10);
const DEFAULT_TILE_LENGTH: f32 = 100.0;
const DEFAULT_CELLS_PER_TILE: u32 = 1;

const DEFAULT_STEP_QUANTUM: Duration = Duration::from_millis(250);
const MIN_STEP_QUANTUM: Duration = Duration::from_micros(1);
const SIDE_BORDER_CELL_LAYERS: u32 = 1;
const TOP_BORDER_CELL_LAYERS: u32 = 1;
const BOTTOM_BORDER_CELL_LAYERS: u32 = 1;
const EXIT_CELL_LAYERS: u32 = 1;
const INITIAL_GOLD: Gold = Gold::new(100);
const HARD_WIN_DIFFICULTY_PROMOTION: u32 = 1;
const ROUND_LOSS_DIFFICULTY_PENALTY: u32 = 1;
#[cfg(any(test, feature = "tower_scaffolding"))]
const ROUND_LOSS_TOWER_REMOVAL_PERCENT: u32 = 50;
const DEFAULT_WAVE_GLOBAL_SEED: u64 = 0;
const DEFAULT_LEVEL_ID: LevelId = LevelId::new(0);

/// Represents the authoritative Maze Defence world state.
#[derive(Debug)]
pub struct World {
    banner: &'static str,
    tile_grid: TileGrid,
    cells_per_tile: u32,
    target: Target,
    targets: Vec<CellCoord>,
    bugs: Vec<Bug>,
    bug_positions: HashMap<BugId, usize>,
    bug_spawners: BugSpawnerRegistry,
    next_bug_id: u32,
    projectiles: BTreeMap<ProjectileId, ProjectileState>,
    #[cfg_attr(not(any(test, feature = "tower_scaffolding")), allow(dead_code))]
    next_projectile_id: ProjectileId,
    occupancy: OccupancyGrid,
    walls: MazeWalls,
    navigation_field: NavigationField,
    navigation_dirty: bool,
    gold: Gold,
    difficulty_level: u32,
    pending_wave_difficulty: PendingWaveDifficulty,
    species_table_version: SpeciesTableVersion,
    species_definitions: Vec<SpeciesDefinition>,
    spawn_patches: Vec<SpawnPatchDescriptor>,
    pressure_config: PressureConfig,
    pressure_wave_cache: HashMap<PressureWaveInputs, PressureWavePlan>,
    pressure_v2: PressureV2,
    wave_seed_global: u64,
    level_id: LevelId,
    active_wave: Option<ActiveWaveContext>,
    next_wave_id: WaveId,
    #[cfg(any(test, feature = "tower_scaffolding"))]
    towers: TowerRegistry,
    #[cfg(any(test, feature = "tower_scaffolding"))]
    tower_occupancy: BitGrid,
    reservations: ReservationFrame,
    tick_index: u64,
    step_quantum: Duration,
    play_mode: PlayMode,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct ActiveWaveContext {
    id: WaveId,
    difficulty: WaveDifficulty,
    effective_difficulty: u32,
    reward_multiplier: u32,
    pressure_scalar: u32,
}

impl ActiveWaveContext {
    fn reward_multiplier(&self) -> u32 {
        self.reward_multiplier
    }
}

fn default_species_table() -> (SpeciesTableVersion, Vec<SpeciesDefinition>) {
    let version = SpeciesTableVersion::new(1);
    let mut definitions = vec![SpeciesDefinition::new(
        SpeciesId::new(0),
        SpawnPatchId::new(0),
        SpeciesPrototype::new(
            BugColor::from_rgb(0x5a, 0xb4, 0xff),
            Health::new(5),
            NonZeroU32::new(400).expect("non-zero cadence"),
        ),
        PressureWeight::new(NonZeroU32::new(1_500).expect("non-zero weight")),
        DirichletWeight::new(NonZeroU32::new(2).expect("non-zero concentration")),
        0,
        NonZeroU32::new(10_000).expect("non-zero population cap"),
        CadenceRange::new(
            NonZeroU32::new(300).expect("non-zero cadence min"),
            NonZeroU32::new(600).expect("non-zero cadence max"),
        ),
        BurstGapRange::new(
            NonZeroU32::new(2_000).expect("non-zero gap min"),
            NonZeroU32::new(8_000).expect("non-zero gap max"),
        ),
    )];
    definitions.sort_by_key(|definition| definition.id());
    (version, definitions)
}

fn default_spawn_patches() -> Vec<SpawnPatchDescriptor> {
    let mut patches = vec![SpawnPatchDescriptor::new(
        SpawnPatchId::new(0),
        CellCoord::new(0, 0),
        CellRect::from_origin_and_size(CellCoord::new(0, 0), CellRectSize::new(1, 1)),
    )];
    patches.sort_by_key(|descriptor| descriptor.id());
    patches
}

fn default_pressure_config() -> PressureConfig {
    PressureConfig::new(
        PressureCurve::new(Pressure::new(35), Pressure::new(5)),
        DirichletWeight::new(NonZeroU32::new(2).expect("non-zero concentration")),
        BurstSchedulingConfig::new(
            NonZeroU32::new(20).expect("non-zero burst size"),
            NonZeroU32::new(8).expect("non-zero burst limit"),
        ),
        NonZeroU32::new(2_000).expect("non-zero spawn cap"),
    )
}

impl World {
    /// Creates a new Maze Defence world ready for simulation.
    #[must_use]
    pub fn new() -> Self {
        let tile_grid = TileGrid::new(DEFAULT_GRID_COLUMNS, DEFAULT_GRID_ROWS, DEFAULT_TILE_LENGTH);
        let cells_per_tile = DEFAULT_CELLS_PER_TILE;
        let (target, targets) = build_target(tile_grid.columns(), tile_grid.rows(), cells_per_tile);
        let total_columns = total_cell_columns(tile_grid.columns(), cells_per_tile);
        let total_rows = total_cell_rows(tile_grid.rows(), cells_per_tile);
        let occupancy = OccupancyGrid::new(total_columns, total_rows);
        let mut walls = MazeWalls::new(total_columns, total_rows);
        walls.rebuild(
            total_columns,
            total_rows,
            build_cell_walls(tile_grid.columns(), tile_grid.rows(), cells_per_tile),
        );
        let (species_table_version, species_definitions) = default_species_table();
        let spawn_patches = default_spawn_patches();
        let pressure_config = default_pressure_config();
        #[cfg(any(test, feature = "tower_scaffolding"))]
        let tower_occupancy = BitGrid::new(total_columns, total_rows);
        let mut world = Self {
            banner: WELCOME_BANNER,
            bugs: Vec::new(),
            bug_positions: HashMap::new(),
            bug_spawners: BugSpawnerRegistry::new(),
            next_bug_id: 0,
            projectiles: BTreeMap::new(),
            next_projectile_id: ProjectileId::new(0),
            occupancy,
            walls,
            navigation_field: NavigationField::default(),
            navigation_dirty: true,
            gold: INITIAL_GOLD,
            difficulty_level: 0,
            pending_wave_difficulty: PendingWaveDifficulty::Unset,
            species_table_version,
            species_definitions,
            spawn_patches,
            pressure_config,
            pressure_wave_cache: HashMap::new(),
            pressure_v2: PressureV2::default(),
            wave_seed_global: DEFAULT_WAVE_GLOBAL_SEED,
            level_id: DEFAULT_LEVEL_ID,
            active_wave: None,
            next_wave_id: WaveId::new(0),
            #[cfg(any(test, feature = "tower_scaffolding"))]
            towers: TowerRegistry::new(),
            #[cfg(any(test, feature = "tower_scaffolding"))]
            tower_occupancy,
            reservations: ReservationFrame::new(),
            target,
            targets,
            tile_grid,
            cells_per_tile,
            tick_index: 0,
            step_quantum: DEFAULT_STEP_QUANTUM,
            play_mode: PlayMode::Builder,
        };
        world.rebuild_bug_spawners();
        world.clear_bugs();
        world.rebuild_navigation_field_if_dirty();
        world
    }

    fn clear_bugs(&mut self) {
        self.bugs.clear();
        self.bug_positions.clear();
        self.occupancy.clear();
        self.reservations.clear();
        self.next_bug_id = 0;
    }

    fn transition_to_play_mode(&mut self, mode: PlayMode, out_events: &mut Vec<Event>) -> bool {
        if self.play_mode == mode {
            return false;
        }

        self.play_mode = mode;

        match mode {
            PlayMode::Attack => {
                self.rebuild_navigation_field_if_dirty();
            }
            PlayMode::Builder => {
                self.clear_bugs();
            }
        }

        out_events.push(Event::PlayModeChanged { mode });
        true
    }

    fn update_gold(&mut self, amount: Gold, out_events: &mut Vec<Event>) {
        if self.gold == amount {
            return;
        }

        self.gold = amount;
        out_events.push(Event::GoldChanged { amount });
    }

    fn update_difficulty_level(&mut self, level: u32, out_events: &mut Vec<Event>) {
        if self.difficulty_level == level {
            return;
        }

        self.difficulty_level = level;
        out_events.push(Event::DifficultyLevelChanged { level });
    }

    fn assign_pending_wave_difficulty(
        &mut self,
        pending: PendingWaveDifficulty,
        out_events: &mut Vec<Event>,
        force_event: bool,
    ) {
        if !force_event && self.pending_wave_difficulty == pending {
            return;
        }

        self.pending_wave_difficulty = pending;
        out_events.push(Event::PendingWaveDifficultyChanged { pending });
    }

    fn cache_pressure_wave(
        &mut self,
        inputs: PressureWaveInputs,
        plan: PressureWavePlan,
        out_events: &mut Vec<Event>,
    ) {
        self.apply_wave_prototypes(&plan);
        let cached_inputs = inputs.clone();
        let cached_plan = plan.clone();
        let _ = self.pressure_wave_cache.insert(cached_inputs, cached_plan);
        out_events.push(Event::PressureWaveReady { inputs, plan });
    }

    fn apply_wave_prototypes(&mut self, plan: &PressureWavePlan) {
        let prototypes = plan.prototypes();
        if prototypes.is_empty() {
            return;
        }

        let mut changed = false;
        let mut updated = Vec::with_capacity(self.species_definitions.len());
        for definition in &self.species_definitions {
            let replacement = usize::try_from(definition.id().get())
                .ok()
                .and_then(|index| prototypes.get(index))
                .copied()
                .unwrap_or_else(|| definition.prototype());
            if replacement != definition.prototype() {
                changed = true;
            }
            updated.push(SpeciesDefinition::new(
                definition.id(),
                definition.patch(),
                replacement,
                definition.weight(),
                definition.dirichlet_weight(),
                definition.min_burst_spawn(),
                definition.max_population(),
                definition.cadence_range(),
                definition.gap_range(),
            ));
        }

        if changed {
            self.species_definitions = updated;
            let next_version = self.species_table_version.get().saturating_add(1);
            self.species_table_version = SpeciesTableVersion::new(next_version);
        }
    }

    fn launch_wave(
        &mut self,
        wave: WaveId,
        difficulty: WaveDifficulty,
        out_events: &mut Vec<Event>,
    ) {
        if self.play_mode != PlayMode::Attack {
            return;
        }

        let context = self.prepare_wave_context(wave, difficulty);
        let effective_level = DifficultyLevel::new(context.effective_difficulty);
        let inputs =
            PressureWaveInputs::new(self.wave_seed_global, self.level_id, wave, effective_level);

        let Some(plan) = self.pressure_wave_cache.get(&inputs) else {
            return;
        };

        let (plan_pressure, plan_burst_count) = self.summarise_plan(plan);

        self.active_wave = Some(context);
        self.assign_pending_wave_difficulty(PendingWaveDifficulty::Unset, out_events, true);
        out_events.push(Event::WaveStarted {
            wave,
            difficulty,
            effective_difficulty: context.effective_difficulty,
            reward_multiplier: context.reward_multiplier,
            pressure_scalar: context.pressure_scalar,
            plan_pressure,
            plan_species_table_version: self.species_table_version,
            plan_burst_count,
        });
    }

    #[allow(dead_code)]
    fn prepare_wave_context(
        &mut self,
        wave: WaveId,
        difficulty: WaveDifficulty,
    ) -> ActiveWaveContext {
        let expected = self.next_wave_id;
        debug_assert!(
            wave.get() >= expected.get(),
            "wave identifiers must be monotonic"
        );
        self.next_wave_id = WaveId::new(wave.get().saturating_add(1));
        let effective_difficulty = match difficulty {
            WaveDifficulty::Normal => self.difficulty_level,
            WaveDifficulty::Hard => self.difficulty_level.saturating_add(1),
        };
        let reward_multiplier = effective_difficulty.saturating_add(1);
        let pressure_scalar = effective_difficulty.saturating_add(1);
        ActiveWaveContext {
            id: wave,
            difficulty,
            effective_difficulty,
            reward_multiplier,
            pressure_scalar,
        }
    }

    fn reward_multiplier(&self) -> u32 {
        self.active_wave
            .as_ref()
            .map(ActiveWaveContext::reward_multiplier)
            .unwrap_or_else(|| self.difficulty_level.saturating_add(1))
    }

    fn resolve_round_win(
        &mut self,
        active_wave: Option<ActiveWaveContext>,
        out_events: &mut Vec<Event>,
    ) {
        let previous_level = self.difficulty_level;
        let mut hard_wave = None;

        if let Some(context) = active_wave {
            if context.difficulty == WaveDifficulty::Hard {
                let new_level = previous_level.saturating_add(HARD_WIN_DIFFICULTY_PROMOTION);
                self.update_difficulty_level(new_level, out_events);
                hard_wave = Some(context);
            }
        }

        if let Some(context) = hard_wave {
            out_events.push(Event::HardWinAchieved {
                wave: context.id,
                previous_level,
                new_level: self.difficulty_level,
            });
        }
    }

    fn resolve_round_loss(
        &mut self,
        active_wave: Option<ActiveWaveContext>,
        out_events: &mut Vec<Event>,
    ) {
        let _ = active_wave;
        let new_level = self
            .difficulty_level
            .saturating_sub(ROUND_LOSS_DIFFICULTY_PENALTY);
        self.update_difficulty_level(new_level, out_events);

        #[cfg(any(test, feature = "tower_scaffolding"))]
        self.remove_towers_after_loss(out_events);
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn remove_towers_after_loss(&mut self, out_events: &mut Vec<Event>) {
        let total_towers = self.towers.iter().count();
        if total_towers == 0 {
            return;
        }

        let percent = ROUND_LOSS_TOWER_REMOVAL_PERCENT as usize;
        if percent == 0 {
            return;
        }

        let numerator = total_towers.saturating_mul(percent);
        let mut to_remove = numerator / 100;
        if numerator % 100 != 0 {
            to_remove = to_remove.saturating_add(1);
        }
        to_remove = to_remove.max(1);

        let mut ordered_ids: Vec<_> = self.towers.iter().map(|state| state.id).collect();
        ordered_ids.reverse();
        let candidate_ids: Vec<_> = ordered_ids.into_iter().take(to_remove).collect();

        if candidate_ids.is_empty() {
            return;
        }

        let mut removed_states = Vec::with_capacity(candidate_ids.len());
        for tower_id in candidate_ids {
            if let Some(state) = self.towers.remove(tower_id) {
                self.mark_tower_region(state.region, false);
                removed_states.push(state);
            }
        }

        if removed_states.is_empty() {
            return;
        }

        self.mark_navigation_dirty();
        self.rebuild_navigation_field_if_dirty();

        for state in removed_states {
            out_events.push(Event::TowerRemoved {
                tower: state.id,
                region: state.region,
            });
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn set_gold_for_tests(&mut self, amount: Gold) {
        self.gold = amount;
    }

    fn mark_navigation_dirty(&mut self) {
        self.navigation_dirty = true;
    }

    fn rebuild_navigation_field_if_dirty(&mut self) {
        if !self.navigation_dirty {
            return;
        }

        let (columns, rows) = self.occupancy.dimensions();
        let walls = &self.walls;
        #[cfg(any(test, feature = "tower_scaffolding"))]
        let tower_occupancy = &self.tower_occupancy;
        self.navigation_field
            .rebuild_with(columns, rows, &self.targets, |cell| {
                if walls.contains(cell) {
                    return true;
                }

                #[cfg(any(test, feature = "tower_scaffolding"))]
                {
                    if tower_occupancy.contains(cell) {
                        return true;
                    }
                }

                false
            });
        let field_width = self.navigation_field.width();
        let field_height = self.navigation_field.height();
        debug_assert_eq!(field_width, columns);
        debug_assert_eq!(field_height, rows);
        if let (Ok(width), Ok(height)) =
            (usize::try_from(field_width), usize::try_from(field_height))
        {
            debug_assert_eq!(
                self.navigation_field.cells().len(),
                width.checked_mul(height).unwrap_or(0),
            );
        }
        if field_width > 0 && field_height > 0 {
            debug_assert!(self
                .targets
                .iter()
                .all(|exit| { self.navigation_field.distance(*exit) == Some(0) }));
        }
        self.navigation_dirty = false;
    }

    fn iter_bugs_mut(&mut self) -> impl Iterator<Item = &mut Bug> {
        self.bugs.iter_mut()
    }

    fn bug_index(&self, bug_id: BugId) -> Option<usize> {
        self.bug_positions.get(&bug_id).copied()
    }

    fn remove_bug_at_index(&mut self, index: usize) {
        let removed = self.bugs.swap_remove(index);
        let _ = self.bug_positions.remove(&removed.id);
        if index < self.bugs.len() {
            let moved_bug = &self.bugs[index];
            let replaced = self.bug_positions.insert(moved_bug.id, index);
            debug_assert_eq!(replaced, Some(self.bugs.len()));
        }
    }

    fn spawn_from_spawner(
        &mut self,
        cell: CellCoord,
        color: BugColor,
        health: Health,
        step_ms: u32,
        out_events: &mut Vec<Event>,
    ) {
        if !self.bug_spawners.contains(cell) {
            return;
        }

        if self.occupancy.index(cell).is_none() || !self.occupancy.can_enter(cell) {
            return;
        }

        if self.walls.contains(cell) {
            return;
        }

        let bug_id = self.next_bug_identifier();
        let bug = Bug::new(bug_id, cell, color, health, step_ms);
        let bug_health = bug.health();
        self.occupancy.occupy(bug_id, cell);
        let index = self.bugs.len();
        self.bugs.push(bug);
        let replaced = self.bug_positions.insert(bug_id, index);
        debug_assert!(replaced.is_none());
        out_events.push(Event::BugSpawned {
            bug_id,
            cell,
            color,
            health: bug_health,
        });
    }

    fn next_bug_identifier(&mut self) -> BugId {
        let bug_id = BugId::new(self.next_bug_id);
        self.next_bug_id = self.next_bug_id.saturating_add(1);
        bug_id
    }

    #[cfg_attr(not(any(test, feature = "tower_scaffolding")), allow(dead_code))]
    fn next_projectile_identifier(&mut self) -> ProjectileId {
        let id = self.next_projectile_id;
        let next = self.next_projectile_id.get().saturating_add(1);
        self.next_projectile_id = ProjectileId::new(next);
        id
    }

    fn resolve_pending_steps(&mut self, out_events: &mut Vec<Event>) {
        let requests = self.reservations.drain_sorted();
        if requests.is_empty() {
            return;
        }

        let (columns, rows) = self.occupancy.dimensions();
        for claim in requests {
            let Some(index) = self.bug_index(claim.bug_id()) else {
                continue;
            };

            let (before, after) = self.bugs.split_at_mut(index);
            let bug = &mut after[0];
            let from = bug.cell;

            if bug.accum_ms < bug.step_ms {
                continue;
            }

            let Some(next_cell) = advance_cell(from, claim.direction(), columns, rows) else {
                continue;
            };

            if !self.occupancy.can_enter(next_cell) {
                continue;
            }

            if self.walls.contains(next_cell) {
                continue;
            }

            self.occupancy.vacate(from);
            self.occupancy.occupy(bug.id, next_cell);
            bug.advance(next_cell);
            bug.accum_ms = bug.accum_ms.saturating_sub(bug.step_ms);
            out_events.push(Event::BugAdvanced {
                bug_id: bug.id,
                from,
                to: next_cell,
            });

            let _ = before;
        }
    }

    fn process_exit_cells(&mut self, out_events: &mut Vec<Event>) {
        if self.targets.is_empty() {
            return;
        }

        let mut exited = Vec::new();
        for bug in &self.bugs {
            if self.targets.contains(&bug.cell) {
                exited.push((bug.id, bug.cell));
            }
        }

        let triggering_bug = exited.first().map(|(bug_id, _)| *bug_id);

        for (bug_id, cell) in exited {
            self.occupancy.vacate(cell);
            if let Some(position) = self.bug_index(bug_id) {
                self.remove_bug_at_index(position);
            }
            out_events.push(Event::BugExited { bug_id, cell });
        }

        if let Some(bug) = triggering_bug {
            let _ = self.transition_to_play_mode(PlayMode::Builder, out_events);
            out_events.push(Event::RoundLost { bug });
        }
    }

    #[allow(dead_code)]
    fn cleanup_dead_bugs(&mut self) {
        let mut index = 0;
        while index < self.bugs.len() {
            if self.bugs[index].is_dead() {
                let cell = self.bugs[index].cell;
                self.occupancy.vacate(cell);
                self.remove_bug_at_index(index);
            } else {
                index += 1;
            }
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
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
            world.tile_grid = TileGrid::new(columns, rows, tile_length);
            let normalized_cells = cells_per_tile.max(1);
            world.cells_per_tile = normalized_cells;
            let (target, targets) = build_target(columns, rows, normalized_cells);
            world.target = target;
            world.targets = targets;
            let total_columns = total_cell_columns(columns, normalized_cells);
            let total_rows = total_cell_rows(rows, normalized_cells);
            world.occupancy = OccupancyGrid::new(total_columns, total_rows);
            world.walls.rebuild(
                total_columns,
                total_rows,
                build_cell_walls(columns, rows, normalized_cells),
            );
            let (species_table_version, species_definitions) = default_species_table();
            let spawn_patches = default_spawn_patches();
            let pressure_config = default_pressure_config();
            world.species_table_version = species_table_version;
            world.species_definitions = species_definitions;
            world.spawn_patches = spawn_patches;
            world.pressure_config = pressure_config.clone();
            world.pressure_wave_cache.clear();
            world.wave_seed_global = DEFAULT_WAVE_GLOBAL_SEED;
            world.level_id = DEFAULT_LEVEL_ID;
            world.mark_navigation_dirty();
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.tower_occupancy = BitGrid::new(total_columns, total_rows);
                world.towers = TowerRegistry::new();
            }
            world.rebuild_bug_spawners();
            world.clear_bugs();
            world.rebuild_navigation_field_if_dirty();
            world.update_difficulty_level(0, out_events);
            world.assign_pending_wave_difficulty(PendingWaveDifficulty::Unset, out_events, true);
            out_events.push(Event::PressureConfigChanged {
                species_table_version: world.species_table_version,
                pressure: pressure_config,
            });
        }
        Command::Tick { dt } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }

            world.rebuild_navigation_field_if_dirty();
            world.tick_index = world.tick_index.saturating_add(1);
            out_events.push(Event::TimeAdvanced { dt });

            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                let tower_ids: Vec<_> = world.towers.iter().map(|state| state.id).collect();
                for tower_id in tower_ids {
                    if let Some(state) = world.towers.get_mut(tower_id) {
                        state.cooldown_remaining = state.cooldown_remaining.saturating_sub(dt);
                    }
                }
            }

            let dt_millis = u32::try_from(dt.as_millis()).unwrap_or(u32::MAX);
            let projectile_ids: Vec<_> = world.projectiles.keys().copied().collect();
            let mut completed = Vec::new();
            for projectile_id in projectile_ids {
                if let Some(projectile) = world.projectiles.get_mut(&projectile_id) {
                    if projectile.travel_time_ms == 0 {
                        projectile.travelled_half = projectile.distance_half;
                        completed.push((projectile_id, projectile.target, projectile.damage));
                        continue;
                    }

                    let new_elapsed = projectile
                        .elapsed_ms
                        .saturating_add(u128::from(dt_millis))
                        .min(projectile.travel_time_ms);
                    projectile.elapsed_ms = new_elapsed;

                    let travelled = projectile.distance_half.saturating_mul(new_elapsed)
                        / projectile.travel_time_ms;
                    projectile.travelled_half = travelled;

                    if projectile.elapsed_ms >= projectile.travel_time_ms {
                        completed.push((projectile_id, projectile.target, projectile.damage));
                    }
                }
            }

            for (projectile_id, target, damage) in completed {
                world.resolve_projectile_completion(projectile_id, target, damage, out_events);
            }

            for bug in world.iter_bugs_mut() {
                let advanced = bug.accum_ms.saturating_add(dt_millis);
                bug.accum_ms = advanced.min(bug.step_ms);
            }
        }
        Command::ConfigureBugStep { step_duration } => {
            let clamped = step_duration.max(MIN_STEP_QUANTUM);
            world.step_quantum = clamped;
        }
        Command::StepBug { bug_id, direction } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }
            world
                .reservations
                .queue(world.tick_index, ReservationClaim::new(bug_id, direction));
            world.resolve_pending_steps(out_events);
            world.process_exit_cells(out_events);
        }
        Command::SetPlayMode { mode } => {
            let _ = world.transition_to_play_mode(mode, out_events);
        }
        Command::SetDifficultyLevel { level } => {
            world.update_difficulty_level(level.get(), out_events);
        }
        Command::SpawnBug {
            spawner,
            color,
            health,
            step_ms,
        } => {
            if world.play_mode == PlayMode::Builder {
                return;
            }

            world.spawn_from_spawner(spawner, color, health, step_ms, out_events);
        }
        Command::FireProjectile { tower, target } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_fire_projectile(tower, target, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = (tower, target);
        }
        Command::PlaceTower { kind, origin } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_place_tower(kind, origin, TowerPlacementCost::SpendGold, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = (kind, origin);
        }
        Command::ImportTower { kind, origin } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_place_tower(kind, origin, TowerPlacementCost::IgnoreGold, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = (kind, origin);
        }
        Command::RemoveTower { tower } => {
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.handle_remove_tower(tower, out_events);
            }

            #[cfg(not(any(test, feature = "tower_scaffolding")))]
            let _ = tower;
        }
        Command::GeneratePressureWave { inputs } => {
            let mut spawns = Vec::new();
            let mut prototypes = Vec::new();
            world
                .pressure_v2
                .generate(&inputs, &mut spawns, &mut prototypes);
            let plan = PressureWavePlan::new(spawns, prototypes);
            world.cache_pressure_wave(inputs, plan, out_events);
        }
        Command::CachePressureWave { inputs, plan } => {
            world.cache_pressure_wave(inputs, plan, out_events);
        }
        Command::StartWave { wave, difficulty } => {
            world.launch_wave(wave, difficulty, out_events);
        }
        Command::ResolveRound { outcome } => {
            let active_wave = world.active_wave.take();
            match outcome {
                RoundOutcome::Win => world.resolve_round_win(active_wave, out_events),
                RoundOutcome::Loss => world.resolve_round_loss(active_wave, out_events),
            }
        }
    }
}

impl World {
    fn summarise_plan(&self, plan: &PressureWavePlan) -> (Pressure, u32) {
        let mut counts: HashMap<SpeciesId, u32> = HashMap::new();
        for spawn in plan.spawns() {
            let species_id = SpeciesId::new(spawn.species_id());
            let entry = counts.entry(species_id).or_insert(0);
            *entry = entry.saturating_add(1);
        }

        let burst_config = self.pressure_config.burst_scheduling();
        let nominal = burst_config.nominal_burst_size().get();
        let max_bursts = burst_config.burst_count_max().get();

        let mut total_pressure_fixed: u64 = 0;
        let mut total_bursts: u32 = 0;

        for (species_id, count) in counts {
            if count == 0 {
                continue;
            }

            if let Some(definition) = self
                .species_definitions
                .iter()
                .find(|definition| definition.id() == species_id)
            {
                let weight_fixed = definition.weight().get().get();
                total_pressure_fixed =
                    total_pressure_fixed.saturating_add(u64::from(count) * u64::from(weight_fixed));

                let bursts = Self::burst_count_for(count, nominal, max_bursts);
                total_bursts = total_bursts.saturating_add(bursts);
            }
        }

        let pressure_value = (total_pressure_fixed / u64::from(PRESSURE_FIXED_POINT_SCALE))
            .min(u64::from(u32::MAX)) as u32;

        (Pressure::new(pressure_value), total_bursts)
    }

    fn burst_count_for(count: u32, nominal: u32, max_bursts: u32) -> u32 {
        if count == 0 {
            return 0;
        }

        let nominal = nominal.max(1);
        let max_bursts = max_bursts.max(1);
        let mut bursts = count / nominal;
        if count % nominal != 0 {
            bursts = bursts.saturating_add(1);
        }
        bursts = bursts.max(1);
        bursts.min(max_bursts)
    }

    fn rebuild_bug_spawners(&mut self) {
        let (columns, rows) = self.occupancy.dimensions();
        self.bug_spawners.assign_outer_rim(columns, rows);
        self.bug_spawners.remove_bottom_row(columns, rows);
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn handle_fire_projectile(
        &mut self,
        tower: TowerId,
        target: BugId,
        out_events: &mut Vec<Event>,
    ) {
        if self.play_mode != PlayMode::Attack {
            out_events.push(Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::InvalidMode,
            });
            return;
        }

        let Some(tower_state) = self.towers.get(tower) else {
            out_events.push(Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::MissingTower,
            });
            return;
        };

        if tower_state.cooldown_remaining > Duration::ZERO {
            out_events.push(Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::CooldownActive,
            });
            return;
        }

        let tower_region = tower_state.region;
        let tower_kind = tower_state.kind;

        let Some(bug_index) = self.bug_index(target) else {
            out_events.push(Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::MissingTarget,
            });
            return;
        };

        let bug_cell = {
            let bug = &self.bugs[bug_index];
            if bug.health.is_zero() {
                out_events.push(Event::ProjectileRejected {
                    tower,
                    target,
                    reason: ProjectileRejection::MissingTarget,
                });
                return;
            }
            bug.cell
        };

        let projectile_id = self.next_projectile_identifier();
        let start = tower_center_half(tower_region);
        let end = bug_center_half(bug_cell);
        let distance_half = start.distance_to(end);
        let range_cells = tower_kind.range_in_cells(self.cells_per_tile);
        let max_range_half = u128::from(range_cells) * 2;
        let base_time_ms = u128::from(tower_kind.projectile_travel_time_ms());
        let travel_time_ms =
            compute_projectile_travel_time(distance_half, max_range_half, base_time_ms);
        let projectile_state = ProjectileState {
            id: projectile_id,
            tower,
            target,
            start,
            end,
            distance_half,
            travelled_half: 0,
            travel_time_ms,
            elapsed_ms: 0,
            damage: tower_kind.projectile_damage(),
        };
        let replaced = self.projectiles.insert(projectile_id, projectile_state);
        debug_assert!(replaced.is_none());

        if let Some(state) = self.towers.get_mut(tower) {
            state.cooldown_remaining =
                Duration::from_millis(u64::from(tower_kind.fire_cooldown_ms()));
        }

        out_events.push(Event::ProjectileFired {
            projectile: projectile_id,
            tower,
            target,
        });
    }

    fn resolve_projectile_completion(
        &mut self,
        projectile_id: ProjectileId,
        target: BugId,
        damage: Damage,
        out_events: &mut Vec<Event>,
    ) {
        let removed = self.projectiles.remove(&projectile_id);
        debug_assert!(removed.is_some());

        let Some(index) = self.bug_index(target) else {
            out_events.push(Event::ProjectileExpired {
                projectile: projectile_id,
            });
            return;
        };

        if self.bugs[index].health.is_zero() {
            out_events.push(Event::ProjectileExpired {
                projectile: projectile_id,
            });
            return;
        }

        let (remaining, death_cell) = {
            let bug = &mut self.bugs[index];
            let updated = bug.health.saturating_sub(damage);
            let death_cell = if updated.is_zero() {
                Some(bug.cell)
            } else {
                None
            };
            bug.health = updated;
            (updated, death_cell)
        };

        out_events.push(Event::BugDamaged {
            bug: target,
            remaining,
        });

        if let Some(cell) = death_cell {
            self.occupancy.vacate(cell);
            self.remove_bug_at_index(index);
            let base_reward = Gold::new(1);
            let multiplier = self.reward_multiplier();
            let scaled_reward = Gold::new(base_reward.get().saturating_mul(multiplier));
            let updated = self.gold.saturating_add(scaled_reward);
            self.update_gold(updated, out_events);
            out_events.push(Event::BugDied { bug: target });
        }

        out_events.push(Event::ProjectileHit {
            projectile: projectile_id,
            target,
            damage,
        });
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn handle_place_tower(
        &mut self,
        kind: TowerKind,
        origin: CellCoord,
        cost_policy: TowerPlacementCost,
        out_events: &mut Vec<Event>,
    ) {
        if self.play_mode != PlayMode::Builder {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::InvalidMode,
            });
            return;
        }

        if let Some(stride) = self.tower_alignment_stride() {
            let Some(column_alignment) = origin.column().checked_sub(SIDE_BORDER_CELL_LAYERS)
            else {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            };
            let Some(row_alignment) = origin.row().checked_sub(TOP_BORDER_CELL_LAYERS) else {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            };
            if column_alignment % stride != 0 || row_alignment % stride != 0 {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::Misaligned,
                });
                return;
            }
        }

        let footprint = footprint_for(kind);
        let region = CellRect::from_origin_and_size(origin, footprint);

        if !self.tower_region_within_bounds(region) {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::OutOfBounds,
            });
            return;
        }

        if self.tower_region_occupied(region) {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::Occupied,
            });
            return;
        }

        if !self.exit_path_remains_available(region) {
            out_events.push(Event::TowerPlacementRejected {
                kind,
                origin,
                reason: PlacementError::PathBlocked,
            });
            return;
        }

        if matches!(cost_policy, TowerPlacementCost::SpendGold) {
            let cost = kind.build_cost();
            if self.gold.get() < cost.get() {
                out_events.push(Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason: PlacementError::InsufficientFunds,
                });
                return;
            }

            let remaining = self.gold.saturating_sub(cost);
            self.update_gold(remaining, out_events);
        }

        let id = self.towers.allocate();
        self.mark_tower_region(region, true);
        self.mark_navigation_dirty();
        self.rebuild_navigation_field_if_dirty();
        self.towers.insert(TowerState {
            id,
            kind,
            region,
            cooldown_remaining: Duration::ZERO,
        });
        debug_assert!(self.towers.get(id).is_some());
        out_events.push(Event::TowerPlaced {
            tower: id,
            kind,
            region,
        });
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn handle_remove_tower(&mut self, tower: TowerId, out_events: &mut Vec<Event>) {
        if self.play_mode != PlayMode::Builder {
            out_events.push(Event::TowerRemovalRejected {
                tower,
                reason: RemovalError::InvalidMode,
            });
            return;
        }

        let Some(state) = self.towers.remove(tower) else {
            out_events.push(Event::TowerRemovalRejected {
                tower,
                reason: RemovalError::MissingTower,
            });
            return;
        };

        self.mark_tower_region(state.region, false);
        self.mark_navigation_dirty();
        self.rebuild_navigation_field_if_dirty();
        out_events.push(Event::TowerRemoved {
            tower: state.id,
            region: state.region,
        });
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_alignment_stride(&self) -> Option<u32> {
        let stride = self.cells_per_tile / 2;
        if stride <= 1 {
            None
        } else {
            Some(stride)
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_within_bounds(&self, region: CellRect) -> bool {
        let (columns, rows) = self.tower_occupancy.dimensions();
        let size = region.size();
        if size.width() == 0 || size.height() == 0 {
            return false;
        }

        let origin = region.origin();
        if origin.column() >= columns || origin.row() >= rows {
            return false;
        }

        let Some(end_column) = origin.column().checked_add(size.width()) else {
            return false;
        };
        let Some(end_row) = origin.row().checked_add(size.height()) else {
            return false;
        };

        end_column <= columns && end_row <= rows
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_occupied(&self, region: CellRect) -> bool {
        let origin = region.origin();
        let size = region.size();

        for column_offset in 0..size.width() {
            for row_offset in 0..size.height() {
                let column = origin
                    .column()
                    .checked_add(column_offset)
                    .expect("column bounded by region");
                let row = origin
                    .row()
                    .checked_add(row_offset)
                    .expect("row bounded by region");
                let cell = CellCoord::new(column, row);
                if self.tower_occupancy.contains(cell) {
                    return true;
                }
                if self.walls.contains(cell) {
                    return true;
                }
                if !self.occupancy.can_enter(cell) {
                    return true;
                }
            }
        }

        false
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn mark_tower_region(&mut self, region: CellRect, occupied: bool) {
        let origin = region.origin();
        let size = region.size();

        for column_offset in 0..size.width() {
            for row_offset in 0..size.height() {
                let column = origin
                    .column()
                    .checked_add(column_offset)
                    .expect("column bounded by region");
                let row = origin
                    .row()
                    .checked_add(row_offset)
                    .expect("row bounded by region");
                let cell = CellCoord::new(column, row);
                if occupied {
                    self.tower_occupancy.set(cell);
                } else {
                    self.tower_occupancy.clear(cell);
                }
            }
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn exit_path_remains_available(&self, candidate: CellRect) -> bool {
        if self.targets.is_empty() {
            return true;
        }

        let mut spawners = self.bug_spawners.iter();
        if spawners.next().is_none() {
            return true;
        }

        let (columns, rows) = self.occupancy.dimensions();
        let cells_u64 = u64::from(columns) * u64::from(rows);
        let Ok(capacity) = usize::try_from(cells_u64) else {
            return false;
        };

        if capacity == 0 {
            return true;
        }

        let mut visited = vec![false; capacity];
        let mut queue = VecDeque::new();

        for &start in &self.targets {
            if self.is_cell_blocked_with_candidate(start, candidate) {
                continue;
            }

            if let Some(index) = self.occupancy.index(start) {
                if !visited[index] {
                    visited[index] = true;
                    queue.push_back(start);
                }
            }
        }

        if queue.is_empty() {
            return false;
        }

        const DIRECTIONS: [Direction; 4] = [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ];

        while let Some(cell) = queue.pop_front() {
            if self.bug_spawners.contains(cell) {
                return true;
            }

            for direction in DIRECTIONS {
                if let Some(neighbor) = advance_cell(cell, direction, columns, rows) {
                    if self.is_cell_blocked_with_candidate(neighbor, candidate) {
                        continue;
                    }

                    if let Some(index) = self.occupancy.index(neighbor) {
                        if !visited[index] {
                            visited[index] = true;
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        false
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn is_cell_blocked_with_candidate(&self, cell: CellCoord, candidate: CellRect) -> bool {
        if cell_rect_contains(candidate, cell) {
            return true;
        }

        if self.occupancy.index(cell).is_none() {
            return true;
        }

        if !self.occupancy.can_enter(cell) {
            return true;
        }

        if self.walls.contains(cell) {
            return true;
        }

        if self.tower_occupancy.contains(cell) {
            return true;
        }

        false
    }
}

#[cfg(any(test, feature = "tower_scaffolding"))]
#[derive(Clone, Copy)]
enum TowerPlacementCost {
    SpendGold,
    IgnoreGold,
}

#[cfg(any(test, feature = "tower_scaffolding"))]
fn cell_rect_contains(region: CellRect, cell: CellCoord) -> bool {
    let origin = region.origin();
    let size = region.size();
    let column = u64::from(cell.column());
    let row = u64::from(cell.row());
    let origin_column = u64::from(origin.column());
    let origin_row = u64::from(origin.row());
    let width = u64::from(size.width());
    let height = u64::from(size.height());

    column >= origin_column
        && column < origin_column.saturating_add(width)
        && row >= origin_row
        && row < origin_row.saturating_add(height)
}

/// Query functions that provide read-only access to the world state.
pub mod query {
    use super::{Bug, World};
    use maze_defence_core::{
        BugSnapshot, BugView, CellCoord, Goal, Gold, LevelId, NavigationFieldView, OccupancyView,
        PendingWaveDifficulty, PlayMode, PressureConfig, PressureWaveInputs, PressureWavePlan,
        ProjectileSnapshot, ReservationLedgerView, SpawnPatchTableView, SpeciesTableView, Target,
        TileGrid, WaveSeedContext,
    };

    use maze_defence_core::structures::{Wall as CellWall, WallView as CellWallView};

    #[cfg(any(test, feature = "tower_scaffolding"))]
    use maze_defence_core::{
        CellRect, TowerCooldownSnapshot, TowerCooldownView, TowerId, TowerSnapshot, TowerView,
    };

    /// Reports the active play mode for the world.
    #[must_use]
    pub fn play_mode(world: &World) -> PlayMode {
        world.play_mode
    }

    /// Reports the amount of gold owned by the defender.
    #[must_use]
    pub fn gold(world: &World) -> Gold {
        world.gold
    }

    /// Reports the current difficulty level tracked by the world.
    #[must_use]
    pub fn difficulty_level(world: &World) -> u32 {
        world.difficulty_level
    }

    /// Reports the pending wave difficulty stored inside the world.
    #[must_use]
    pub fn pending_wave_difficulty(world: &World) -> PendingWaveDifficulty {
        world.pending_wave_difficulty
    }

    /// Provides read-only access to the authoritative species table.
    #[must_use]
    pub fn species_table(world: &World) -> SpeciesTableView<'_> {
        SpeciesTableView::new(world.species_table_version, &world.species_definitions)
    }

    /// Provides read-only access to the configured spawn patches.
    #[must_use]
    pub fn patch_table(world: &World) -> SpawnPatchTableView<'_> {
        SpawnPatchTableView::new(&world.spawn_patches)
    }

    /// Reports the level identifier associated with the current world configuration.
    #[must_use]
    pub fn level_id(world: &World) -> LevelId {
        world.level_id
    }

    /// Returns the global pressure configuration stored by the world.
    #[must_use]
    pub fn pressure_config(world: &World) -> &PressureConfig {
        &world.pressure_config
    }

    /// Retrieves a cached pressure wave plan for the specified inputs, if present.
    #[must_use]
    pub fn pressure_wave_plan<'a>(
        world: &'a World,
        inputs: &PressureWaveInputs,
    ) -> Option<&'a PressureWavePlan> {
        world.pressure_wave_cache.get(inputs)
    }

    /// Captures the wave seed derivation context for the next generated wave.
    #[must_use]
    pub fn wave_seed_context(world: &World) -> WaveSeedContext {
        WaveSeedContext::new(
            world.wave_seed_global,
            world.next_wave_id,
            world.difficulty_level,
        )
    }

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

    /// Reports the number of navigation cells contained within a single tile.
    ///
    /// Always ≥ 1; the world normalizes zero-valued configuration inputs at
    /// set-up, and this query enforces the invariant at the read boundary as
    /// well.
    #[must_use]
    pub fn cells_per_tile(world: &World) -> u32 {
        world.cells_per_tile.max(1)
    }

    /// Provides read-only access to the target carved into the perimeter wall.
    #[must_use]
    pub fn target(world: &World) -> &Target {
        &world.target
    }

    /// Selects the goal cell nearest to the provided origin.
    #[must_use]
    pub fn select_goal(origin: CellCoord, candidates: &[CellCoord]) -> Option<Goal> {
        candidates
            .iter()
            .copied()
            .min_by(|left, right| {
                let left_distance = origin.manhattan_distance(*left);
                let right_distance = origin.manhattan_distance(*right);
                left_distance
                    .cmp(&right_distance)
                    .then_with(|| left.column().cmp(&right.column()))
                    .then_with(|| left.row().cmp(&right.row()))
            })
            .map(Goal::at)
    }

    /// Computes the canonical goal for an entity starting from the provided cell.
    #[must_use]
    pub fn goal_for(world: &World, origin: CellCoord) -> Option<Goal> {
        select_goal(origin, &world.targets)
    }

    /// Captures a read-only view of the bugs inhabiting the maze.
    #[must_use]
    pub fn bug_view(world: &World) -> BugView {
        let snapshots: Vec<BugSnapshot> = world
            .bugs
            .iter()
            .filter(|bug| !bug.health.is_zero())
            .map(assemble_bug_snapshot)
            .collect();
        BugView::from_snapshots(snapshots)
    }

    fn assemble_bug_snapshot(bug: &Bug) -> BugSnapshot {
        let ready_for_step = bug.ready_for_step();

        BugSnapshot {
            id: bug.id,
            cell: bug.cell,
            color: bug.color,
            max_health: bug.max_health(),
            health: bug.health,
            step_ms: bug.step_ms,
            accum_ms: bug.accum_ms,
            ready_for_step,
        }
    }

    /// Provides an immutable view of the pre-computed navigation distances.
    ///
    /// The world rebuilds the field eagerly whenever maze geometry changes, so
    /// callers may assume the returned slice reflects the latest layout. The
    /// data is exposed in row-major order and includes the virtual exit row so
    /// systems can reason about the boundary conditions without duplicating the
    /// buffer.
    #[must_use]
    pub fn navigation_field(world: &World) -> NavigationFieldView<'_> {
        debug_assert!(
            !world.navigation_dirty,
            "navigation field must be rebuilt before queries"
        );

        NavigationFieldView::from_slice(
            world.navigation_field.cells(),
            world.navigation_field.width(),
            world.navigation_field.height(),
        )
    }

    /// Exposes a read-only view of the dense occupancy grid.
    #[must_use]
    pub fn occupancy_view(world: &World) -> OccupancyView<'_> {
        let (columns, rows) = world.occupancy.dimensions();
        OccupancyView::new(world.occupancy.cells(), columns, rows)
    }

    /// Captures a read-only view of the pending movement reservations for the active tick.
    #[must_use]
    pub fn reservation_ledger(world: &World) -> ReservationLedgerView<'_> {
        ReservationLedgerView::from_slice(world.reservations.claims())
    }

    /// Captures a read-only view of the cell-sized walls stored in the world.
    #[must_use]
    pub fn walls(world: &World) -> CellWallView {
        let walls: Vec<CellWall> = world.walls.walls().to_vec();
        CellWallView::from_walls(walls)
    }

    /// Captures a read-only snapshot of all towers stored in the world.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[must_use]
    pub fn towers(world: &World) -> TowerView {
        if world.towers.is_empty() {
            return TowerView::from_snapshots(Vec::new());
        }

        let snapshots: Vec<TowerSnapshot> = world
            .towers
            .iter()
            .map(|tower| TowerSnapshot {
                id: tower.id,
                kind: tower.kind,
                region: tower.region,
            })
            .collect();
        TowerView::from_snapshots(snapshots)
    }

    /// Reports whether the provided cell is blocked by the world state.
    #[must_use]
    pub fn is_cell_blocked(world: &World, cell: CellCoord) -> bool {
        if world.occupancy.index(cell).is_none() || !world.occupancy.can_enter(cell) {
            return true;
        }

        if world.walls.contains(cell) {
            return true;
        }

        #[cfg(any(test, feature = "tower_scaffolding"))]
        if world.tower_occupancy.contains(cell) {
            return true;
        }

        false
    }

    /// Identifies the tower occupying the provided cell, if any.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[must_use]
    pub fn tower_at(world: &World, cell: CellCoord) -> Option<TowerId> {
        if !world.tower_occupancy.contains(cell) {
            return None;
        }

        world
            .towers
            .iter()
            .find(|tower| tower_region_contains_cell(tower.region, cell))
            .map(|tower| tower.id)
    }

    /// Captures a read-only snapshot of tower cooldown progress.
    #[cfg(any(test, feature = "tower_scaffolding"))]
    #[must_use]
    pub fn tower_cooldowns(world: &World) -> TowerCooldownView {
        let snapshots: Vec<TowerCooldownSnapshot> = world
            .towers
            .iter()
            .map(|tower| TowerCooldownSnapshot {
                tower: tower.id,
                kind: tower.kind,
                ready_in: tower.cooldown_remaining,
            })
            .collect();
        TowerCooldownView::from_snapshots(snapshots)
    }

    /// Enumerates the wall target cells bugs should attempt to reach.
    #[must_use]
    pub fn target_cells(world: &World) -> Vec<CellCoord> {
        world.targets.clone()
    }

    /// Enumerates the bug spawners ringing the maze.
    #[must_use]
    pub fn bug_spawners(world: &World) -> Vec<CellCoord> {
        world.bug_spawners.iter().collect()
    }

    /// Iterates over the projectile states stored within the world.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn projectiles(world: &World) -> impl Iterator<Item = ProjectileSnapshot> + '_ {
        world
            .projectiles
            .values()
            .map(|projectile| ProjectileSnapshot {
                projectile: projectile.id,
                tower: projectile.tower,
                target: projectile.target,
                origin_half: projectile.start,
                dest_half: projectile.end,
                distance_half: projectile.distance_half,
                travelled_half: projectile.travelled_half,
            })
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn tower_region_contains_cell(region: CellRect, cell: CellCoord) -> bool {
        super::cell_rect_contains(region, cell)
    }
}

#[cfg(any(test, feature = "tower_scaffolding"))]
fn tower_center_half(region: CellRect) -> CellPointHalf {
    let origin = region.origin();
    let size = region.size();
    CellPointHalf::new(
        i64::from(origin.column()) * 2 + i64::from(size.width()),
        i64::from(origin.row()) * 2 + i64::from(size.height()),
    )
}

#[cfg(any(test, feature = "tower_scaffolding"))]
fn bug_center_half(cell: CellCoord) -> CellPointHalf {
    CellPointHalf::new(
        i64::from(cell.column()) * 2 + 1,
        i64::from(cell.row()) * 2 + 1,
    )
}

#[cfg_attr(not(any(test, feature = "tower_scaffolding")), allow(dead_code))]
fn compute_projectile_travel_time(
    distance_half: u128,
    max_range_half: u128,
    base_time_ms: u128,
) -> u128 {
    if distance_half == 0 || max_range_half == 0 || base_time_ms == 0 {
        return 0;
    }

    let scaled = base_time_ms.saturating_mul(distance_half);
    let time = scaled.div_ceil(max_range_half);
    time.max(1)
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct ProjectileState {
    id: ProjectileId,
    tower: TowerId,
    target: BugId,
    start: CellPointHalf,
    end: CellPointHalf,
    distance_half: u128,
    travelled_half: u128,
    travel_time_ms: u128,
    elapsed_ms: u128,
    damage: Damage,
}

#[derive(Clone, Debug)]
struct Bug {
    id: BugId,
    cell: CellCoord,
    color: BugColor,
    max_health: Health,
    health: Health,
    step_ms: u32,
    accum_ms: u32,
}

impl Bug {
    fn new(id: BugId, cell: CellCoord, color: BugColor, health: Health, step_ms: u32) -> Self {
        Self {
            id,
            cell,
            color,
            max_health: health,
            health,
            step_ms,
            accum_ms: step_ms,
        }
    }

    fn health(&self) -> Health {
        self.health
    }

    fn max_health(&self) -> Health {
        self.max_health
    }

    #[allow(dead_code)]
    fn is_dead(&self) -> bool {
        self.health.is_zero()
    }

    fn advance(&mut self, destination: CellCoord) {
        self.cell = destination;
    }

    fn ready_for_step(&self) -> bool {
        self.accum_ms >= self.step_ms
    }
}

#[derive(Clone, Debug)]
struct BugSpawnerRegistry {
    cells: BTreeSet<CellCoord>,
}

impl BugSpawnerRegistry {
    fn new() -> Self {
        Self {
            cells: BTreeSet::new(),
        }
    }

    fn assign_outer_rim(&mut self, columns: u32, rows: u32) {
        self.cells.clear();

        if columns == 0 || rows == 0 {
            return;
        }

        let last_column = columns.saturating_sub(1);
        let last_row = rows.saturating_sub(1);

        for column in 0..columns {
            let _ = self.cells.insert(CellCoord::new(column, 0));
            let _ = self.cells.insert(CellCoord::new(column, last_row));
        }

        for row in 0..rows {
            let _ = self.cells.insert(CellCoord::new(0, row));
            let _ = self.cells.insert(CellCoord::new(last_column, row));
        }
    }

    fn remove_bottom_row(&mut self, columns: u32, rows: u32) {
        if columns == 0 || rows == 0 {
            return;
        }

        for exit_offset in 0..EXIT_CELL_LAYERS {
            let Some(row) = rows.checked_sub(exit_offset + 1) else {
                break;
            };

            for column in 0..columns {
                let _ = self.cells.remove(&CellCoord::new(column, row));
            }
        }

        for border_offset in 0..BOTTOM_BORDER_CELL_LAYERS {
            let Some(row) = rows.checked_sub(EXIT_CELL_LAYERS + border_offset + 1) else {
                break;
            };

            for column in 0..columns {
                let _ = self.cells.remove(&CellCoord::new(column, row));
            }
        }
    }

    fn contains(&self, cell: CellCoord) -> bool {
        self.cells.contains(&cell)
    }

    fn iter(&self) -> impl Iterator<Item = CellCoord> + '_ {
        self.cells.iter().copied()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn cells(&self) -> &BTreeSet<CellCoord> {
        &self.cells
    }
}

#[derive(Debug)]
struct ReservationFrame {
    tick_index: u64,
    claims: Vec<ReservationClaim>,
}

impl ReservationFrame {
    fn new() -> Self {
        Self {
            tick_index: 0,
            claims: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.tick_index = 0;
        self.claims.clear();
    }

    fn queue(&mut self, tick_index: u64, claim: ReservationClaim) {
        if self.tick_index != tick_index {
            self.tick_index = tick_index;
            self.claims.clear();
        }
        self.claims.push(claim);
        self.claims.sort_by_key(|claim| claim.bug_id());
    }

    fn claims(&self) -> &[ReservationClaim] {
        &self.claims
    }

    fn drain_sorted(&mut self) -> Vec<ReservationClaim> {
        self.claims.drain(..).collect()
    }
}

#[derive(Clone, Debug)]
struct OccupancyGrid {
    columns: u32,
    rows: u32,
    cells: Vec<Option<BugId>>,
}

#[derive(Clone, Debug)]
struct MazeWalls {
    grid: BitGrid,
    walls: Vec<CellWall>,
}

impl MazeWalls {
    fn new(columns: u32, rows: u32) -> Self {
        Self {
            grid: BitGrid::new(columns, rows),
            walls: Vec::new(),
        }
    }

    fn rebuild(&mut self, columns: u32, rows: u32, walls: Vec<CellWall>) {
        let mut walls = walls;
        walls.sort_by_key(|wall| (wall.column(), wall.row()));
        walls.dedup();

        self.grid = BitGrid::new(columns, rows);
        for wall in &walls {
            self.grid.set(wall.cell());
        }
        self.walls = walls;
    }

    fn contains(&self, cell: CellCoord) -> bool {
        self.grid.contains(cell)
    }

    fn walls(&self) -> &[CellWall] {
        &self.walls
    }
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

    fn clear(&mut self) {
        self.cells.fill(None);
    }

    pub(crate) fn can_enter(&self, cell: CellCoord) -> bool {
        self.index(cell)
            .is_none_or(|index| self.cells.get(index).copied().unwrap_or(None).is_none())
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

#[derive(Clone, Debug)]
struct BitGrid {
    columns: u32,
    rows: u32,
    words: Vec<u64>,
}

impl BitGrid {
    fn new(columns: u32, rows: u32) -> Self {
        let cell_count = u64::from(columns) * u64::from(rows);
        let word_count = if cell_count == 0 {
            0
        } else {
            ((cell_count - 1) / 64) + 1
        };
        let capacity = usize::try_from(word_count).unwrap_or(0);
        Self {
            columns,
            rows,
            words: vec![0; capacity],
        }
    }

    fn contains(&self, cell: CellCoord) -> bool {
        let Some((index, bit_offset)) = self.bit_position(cell) else {
            return false;
        };
        self.words
            .get(index)
            .is_some_and(|word| (*word & (1_u64 << bit_offset)) != 0)
    }

    fn set(&mut self, cell: CellCoord) {
        if let Some((index, bit_offset)) = self.bit_position(cell) {
            if let Some(word) = self.words.get_mut(index) {
                *word |= 1_u64 << bit_offset;
            }
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn clear(&mut self, cell: CellCoord) {
        if let Some((index, bit_offset)) = self.bit_position(cell) {
            if let Some(word) = self.words.get_mut(index) {
                *word &= !(1_u64 << bit_offset);
            }
        }
    }

    #[cfg(any(test, feature = "tower_scaffolding"))]
    fn dimensions(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }

    fn bit_position(&self, cell: CellCoord) -> Option<(usize, u32)> {
        if cell.column() >= self.columns || cell.row() >= self.rows {
            return None;
        }

        let width = usize::try_from(self.columns).ok()?;
        let row = usize::try_from(cell.row()).ok()?;
        let column = usize::try_from(cell.column()).ok()?;
        let offset = row.checked_mul(width)?.checked_add(column)?;
        let word_index = offset / 64;
        let bit_offset = u32::try_from(offset % 64).ok()?;
        Some((word_index, bit_offset))
    }
}

fn interior_cell_columns(columns: TileCoord, cells_per_tile: u32) -> u32 {
    columns.get().saturating_mul(cells_per_tile)
}

fn interior_cell_rows(rows: TileCoord, cells_per_tile: u32) -> u32 {
    rows.get().saturating_mul(cells_per_tile)
}

fn total_cell_columns(columns: TileCoord, cells_per_tile: u32) -> u32 {
    let interior = interior_cell_columns(columns, cells_per_tile);
    if interior == 0 {
        0
    } else {
        interior.saturating_add(SIDE_BORDER_CELL_LAYERS.saturating_mul(2))
    }
}

fn total_cell_rows(rows: TileCoord, cells_per_tile: u32) -> u32 {
    let interior = interior_cell_rows(rows, cells_per_tile);
    if interior == 0 {
        0
    } else {
        interior
            .saturating_add(TOP_BORDER_CELL_LAYERS)
            .saturating_add(BOTTOM_BORDER_CELL_LAYERS)
            .saturating_add(EXIT_CELL_LAYERS)
    }
}

fn exit_row_for_tile_grid(rows: TileCoord, cells_per_tile: u32) -> u32 {
    total_cell_rows(rows, cells_per_tile).saturating_sub(1)
}

fn exit_columns_for_tile_grid(columns: TileCoord, cells_per_tile: u32) -> Vec<u32> {
    let tile_columns = columns.get();
    if tile_columns == 0 || cells_per_tile == 0 {
        return Vec::new();
    }

    let center_tile = if tile_columns % 2 == 0 {
        tile_columns.saturating_sub(1) / 2
    } else {
        tile_columns / 2
    };
    let left_margin = SIDE_BORDER_CELL_LAYERS;
    let start_column = left_margin.saturating_add(center_tile.saturating_mul(cells_per_tile));

    (0..cells_per_tile)
        .map(|offset| start_column.saturating_add(offset))
        .collect()
}

fn visible_wall_row_for_tile_grid(rows: TileCoord, cells_per_tile: u32) -> Option<u32> {
    if interior_cell_rows(rows, cells_per_tile) == 0 {
        return None;
    }

    let exit_row = exit_row_for_tile_grid(rows, cells_per_tile);
    let bottom_border_end = exit_row.checked_sub(EXIT_CELL_LAYERS)?;
    let offset = BOTTOM_BORDER_CELL_LAYERS.saturating_sub(1);
    bottom_border_end.checked_sub(offset)
}

#[cfg(test)]
#[allow(dead_code)]
fn walkway_row_for_tile_grid(rows: TileCoord, cells_per_tile: u32) -> Option<u32> {
    let wall_row = visible_wall_row_for_tile_grid(rows, cells_per_tile)?;
    wall_row.checked_sub(1)
}

fn build_cell_walls(columns: TileCoord, rows: TileCoord, cells_per_tile: u32) -> Vec<CellWall> {
    let total_columns = total_cell_columns(columns, cells_per_tile);
    let Some(visible_wall_row) = visible_wall_row_for_tile_grid(rows, cells_per_tile) else {
        return Vec::new();
    };

    if total_columns == 0 {
        return Vec::new();
    }

    let exit_columns = exit_columns_for_tile_grid(columns, cells_per_tile);
    let mut walls = Vec::with_capacity(usize::try_from(total_columns).unwrap_or_default());

    for column in 0..total_columns {
        if exit_columns.binary_search(&column).is_ok() {
            continue;
        }

        walls.push(CellWall::at(CellCoord::new(column, visible_wall_row)));
    }

    walls
}

fn build_target(
    columns: TileCoord,
    rows: TileCoord,
    cells_per_tile: u32,
) -> (Target, Vec<CellCoord>) {
    let cells = target_cells(columns, rows, cells_per_tile);
    let target_cells: Vec<CellCoord> = cells.iter().map(|cell| cell.cell()).collect();
    (Target::new(cells), target_cells)
}

fn target_cells(columns: TileCoord, rows: TileCoord, cells_per_tile: u32) -> Vec<TargetCell> {
    if columns.get() == 0 || rows.get() == 0 || cells_per_tile == 0 {
        return Vec::new();
    }

    let exit_row = exit_row_for_tile_grid(rows, cells_per_tile);
    exit_columns_for_tile_grid(columns, cells_per_tile)
        .into_iter()
        .map(|column| TargetCell::new(column, exit_row))
        .collect()
}

fn advance_cell(
    from: CellCoord,
    direction: Direction,
    columns: u32,
    rows: u32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{
        BugColor, DifficultyLevel, Health, LevelId, PlayMode, PressureSpawnRecord,
        PressureWaveInputs, PressureWavePlan, SpeciesPrototype, WaveDifficulty, WaveId,
    };
    use std::num::NonZeroU32;

    #[test]
    fn generate_pressure_wave_caches_plan_and_emits_event() {
        let mut world = World::new();
        let mut events = Vec::new();
        let inputs =
            PressureWaveInputs::new(42, LevelId::new(7), WaveId::new(3), DifficultyLevel::new(5));

        apply(
            &mut world,
            Command::GeneratePressureWave {
                inputs: inputs.clone(),
            },
            &mut events,
        );

        assert_eq!(events.len(), 1);
        let Some(Event::PressureWaveReady {
            inputs: ready_inputs,
            plan,
        }) = events.last()
        else {
            panic!("expected pressure wave ready event");
        };
        assert_eq!(ready_inputs, &inputs);
        assert!(
            !plan.spawns().is_empty(),
            "generated plan should contain spawns"
        );
        assert!(
            !plan.prototypes().is_empty(),
            "generated plan should contain prototypes"
        );

        let cached =
            query::pressure_wave_plan(&world, &inputs).expect("world should cache generated plan");
        assert_eq!(cached, plan);
    }

    #[test]
    fn cache_pressure_wave_stores_plan_and_emits_event() {
        let mut world = World::new();
        let mut events = Vec::new();
        let inputs =
            PressureWaveInputs::new(7, LevelId::new(2), WaveId::new(1), DifficultyLevel::new(3));
        let initial_version = query::species_table(&world).version();
        let tinted = SpeciesPrototype::new(
            BugColor::from_rgb(0x11, 0x22, 0x33),
            Health::new(15),
            NonZeroU32::new(320).expect("non-zero cadence"),
        );
        let plan =
            PressureWavePlan::new(vec![PressureSpawnRecord::new(0, 15, 1.2, 0)], vec![tinted]);

        apply(
            &mut world,
            Command::CachePressureWave {
                inputs: inputs.clone(),
                plan: plan.clone(),
            },
            &mut events,
        );

        assert_eq!(events.len(), 1);
        let Some(Event::PressureWaveReady {
            inputs: ready_inputs,
            plan: cached_plan,
        }) = events.last()
        else {
            panic!("expected pressure wave ready event");
        };
        assert_eq!(ready_inputs, &inputs);
        assert_eq!(cached_plan, &plan);

        let cached =
            query::pressure_wave_plan(&world, &inputs).expect("world should cache supplied plan");
        assert_eq!(cached, &plan);

        let table = query::species_table(&world);
        assert!(table.version().get() > initial_version.get());
        let first = table
            .definitions()
            .first()
            .expect("species table should contain at least one definition");
        assert_eq!(first.prototype(), tinted);
    }

    #[test]
    fn launch_wave_emits_wave_started_with_plan_summary() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let context = query::wave_seed_context(&world);
        let level_id = query::level_id(&world);
        let inputs = PressureWaveInputs::new(
            context.global_seed(),
            level_id,
            context.wave(),
            DifficultyLevel::new(context.difficulty_level()),
        );
        let plan = PressureWavePlan::new(
            vec![
                PressureSpawnRecord::new(0, 20, 1.0, 0),
                PressureSpawnRecord::new(250, 25, 1.0, 0),
                PressureSpawnRecord::new(500, 30, 1.0, 0),
            ],
            vec![SpeciesPrototype::new(
                BugColor::from_rgb(0x44, 0x55, 0x66),
                Health::new(20),
                NonZeroU32::new(400).expect("non-zero cadence"),
            )],
        );

        apply(
            &mut world,
            Command::CachePressureWave {
                inputs: inputs.clone(),
                plan: plan.clone(),
            },
            &mut events,
        );
        events.clear();

        world.launch_wave(context.wave(), WaveDifficulty::Normal, &mut events);

        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            Event::PendingWaveDifficultyChanged { .. }
        ));
        let Some(Event::WaveStarted {
            wave,
            difficulty,
            effective_difficulty,
            reward_multiplier,
            pressure_scalar,
            plan_pressure,
            plan_species_table_version,
            plan_burst_count,
        }) = events.get(1)
        else {
            panic!("expected wave started event");
        };
        assert_eq!(*wave, context.wave());
        assert_eq!(*difficulty, WaveDifficulty::Normal);
        assert_eq!(*effective_difficulty, context.difficulty_level());
        assert_eq!(
            *reward_multiplier,
            context.difficulty_level().saturating_add(1)
        );
        assert_eq!(
            *pressure_scalar,
            context.difficulty_level().saturating_add(1)
        );
        assert_eq!(plan_pressure.get(), 4);
        assert_eq!(plan_species_table_version, &world.species_table_version);
        assert_eq!(*plan_burst_count, 1);
        assert!(world.active_wave.is_some());
    }
}
