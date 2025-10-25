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
    collections::{btree_map::Entry, BTreeMap, BTreeSet, HashMap},
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
    AttackPlan, BugColor, BugId, BurstGapRange, BurstSchedulingConfig, CadenceRange, CellCoord,
    CellPointHalf, CellRect, CellRectSize, Command, Damage, Direction, DirichletWeight, Event,
    Gold, Health, PendingWaveDifficulty, PlayMode, Pressure, PressureConfig, PressureCurve,
    PressureWeight, ProjectileId, ReservationClaim, RoundOutcome, SpawnPatchDescriptor,
    SpawnPatchId, SpeciesDefinition, SpeciesId, SpeciesPrototype, SpeciesTableVersion, Target,
    TargetCell, TileCoord, TileGrid, WaveDifficulty, WaveId, WELCOME_BANNER,
};

#[cfg(test)]
use maze_defence_core::BurstPlan;

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
const HARD_WIN_TIER_PROMOTION: u32 = 1;
const ROUND_LOSS_TIER_PENALTY: u32 = 1;
#[cfg(any(test, feature = "tower_scaffolding"))]
const ROUND_LOSS_TOWER_REMOVAL_PERCENT: u32 = 50;
const DEFAULT_WAVE_GLOBAL_SEED: u64 = 0;

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
    difficulty_tier: u32,
    pending_wave_difficulty: PendingWaveDifficulty,
    species_table_version: SpeciesTableVersion,
    species_definitions: Vec<SpeciesDefinition>,
    spawn_patches: Vec<SpawnPatchDescriptor>,
    pressure_config: PressureConfig,
    wave_seed_global: u64,
    attack_plans: BTreeMap<WaveId, StoredAttackPlan>,
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

#[derive(Clone, Copy, Debug)]
struct ActiveWaveContext {
    id: WaveId,
    difficulty: WaveDifficulty,
    tier_effective: u32,
    reward_multiplier: u32,
    pressure_scalar: u32,
}

impl ActiveWaveContext {
    fn reward_multiplier(&self) -> u32 {
        self.reward_multiplier
    }
}

#[derive(Clone, Debug)]
struct StoredAttackPlan {
    difficulty: WaveDifficulty,
    plan: AttackPlan,
}

impl StoredAttackPlan {
    fn new(difficulty: WaveDifficulty, plan: AttackPlan) -> Self {
        Self { difficulty, plan }
    }

    fn difficulty(&self) -> WaveDifficulty {
        self.difficulty
    }

    fn plan(&self) -> &AttackPlan {
        &self.plan
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
        PressureCurve::new(Pressure::new(1_200), Pressure::new(250)),
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
            difficulty_tier: 0,
            pending_wave_difficulty: PendingWaveDifficulty::Unset,
            species_table_version,
            species_definitions,
            spawn_patches,
            pressure_config,
            wave_seed_global: DEFAULT_WAVE_GLOBAL_SEED,
            attack_plans: BTreeMap::new(),
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

    fn update_difficulty_tier(&mut self, tier: u32, out_events: &mut Vec<Event>) {
        if self.difficulty_tier == tier {
            return;
        }

        self.difficulty_tier = tier;
        out_events.push(Event::DifficultyTierChanged { tier });
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

    fn cache_attack_plan(&mut self, wave: WaveId, difficulty: WaveDifficulty, plan: AttackPlan) {
        match self.attack_plans.entry(wave) {
            Entry::Occupied(entry) => {
                let stored = entry.get();
                debug_assert_eq!(stored.difficulty(), difficulty);
                debug_assert_eq!(stored.plan(), &plan);
            }
            Entry::Vacant(entry) => {
                let _ = entry.insert(StoredAttackPlan::new(difficulty, plan));
            }
        }
    }

    fn launch_wave(
        &mut self,
        wave: WaveId,
        difficulty: WaveDifficulty,
        out_events: &mut Vec<Event>,
    ) {
        self.assign_pending_wave_difficulty(
            PendingWaveDifficulty::Selected(difficulty),
            out_events,
            false,
        );
        let context = self.prepare_wave_context(wave, difficulty);
        self.active_wave = Some(context);
        let plan_summary = self.attack_plans.get(&wave);
        let (plan_pressure, plan_species_table_version, plan_burst_count) =
            if let Some(stored) = plan_summary {
                let plan = stored.plan();
                let bursts = plan.bursts().len();
                let burst_count = u32::try_from(bursts).unwrap_or(u32::MAX);
                (plan.pressure(), plan.species_table_version(), burst_count)
            } else {
                (Pressure::new(0), self.species_table_version, 0)
            };
        out_events.push(Event::WaveStarted {
            wave: context.id,
            difficulty: context.difficulty,
            tier_effective: context.tier_effective,
            reward_multiplier: context.reward_multiplier,
            pressure_scalar: context.pressure_scalar,
            plan_pressure,
            plan_species_table_version,
            plan_burst_count,
        });
    }

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
        let tier_effective = match difficulty {
            WaveDifficulty::Normal => self.difficulty_tier,
            WaveDifficulty::Hard => self.difficulty_tier.saturating_add(1),
        };
        let reward_multiplier = tier_effective.saturating_add(1);
        let pressure_scalar = tier_effective.saturating_add(1);
        ActiveWaveContext {
            id: wave,
            difficulty,
            tier_effective,
            reward_multiplier,
            pressure_scalar,
        }
    }

    fn reward_multiplier(&self) -> u32 {
        self.active_wave
            .as_ref()
            .map(ActiveWaveContext::reward_multiplier)
            .unwrap_or_else(|| self.difficulty_tier.saturating_add(1))
    }

    fn resolve_round_win(
        &mut self,
        active_wave: Option<ActiveWaveContext>,
        out_events: &mut Vec<Event>,
    ) {
        let previous_tier = self.difficulty_tier;
        let mut hard_wave = None;

        if let Some(context) = active_wave {
            if context.difficulty == WaveDifficulty::Hard {
                let new_tier = previous_tier.saturating_add(HARD_WIN_TIER_PROMOTION);
                self.update_difficulty_tier(new_tier, out_events);
                hard_wave = Some(context);
            }
        }

        if let Some(context) = hard_wave {
            out_events.push(Event::HardWinAchieved {
                wave: context.id,
                previous_tier,
                new_tier: self.difficulty_tier,
            });
        }
    }

    fn resolve_round_loss(
        &mut self,
        active_wave: Option<ActiveWaveContext>,
        out_events: &mut Vec<Event>,
    ) {
        let _ = active_wave;
        let new_tier = self.difficulty_tier.saturating_sub(ROUND_LOSS_TIER_PENALTY);
        self.update_difficulty_tier(new_tier, out_events);

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
            world.wave_seed_global = DEFAULT_WAVE_GLOBAL_SEED;
            world.attack_plans.clear();
            world.mark_navigation_dirty();
            #[cfg(any(test, feature = "tower_scaffolding"))]
            {
                world.tower_occupancy = BitGrid::new(total_columns, total_rows);
                world.towers = TowerRegistry::new();
            }
            world.rebuild_bug_spawners();
            world.clear_bugs();
            world.rebuild_navigation_field_if_dirty();
            world.update_difficulty_tier(0, out_events);
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
                world.handle_place_tower(kind, origin, out_events);
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
        Command::GenerateAttackPlan { wave, difficulty } => {
            let _ = (wave, difficulty);
        }
        Command::CacheAttackPlan {
            wave,
            difficulty,
            plan,
        } => {
            world.cache_attack_plan(wave, difficulty, plan);
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
        AttackPlan, BugSnapshot, BugView, CellCoord, Goal, Gold, NavigationFieldView,
        OccupancyView, PendingWaveDifficulty, PlayMode, PressureConfig, ProjectileSnapshot,
        ReservationLedgerView, SpawnPatchTableView, SpeciesTableView, Target, TileGrid, WaveId,
        WaveSeedContext,
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

    /// Reports the current difficulty tier tracked by the world.
    #[must_use]
    pub fn difficulty_tier(world: &World) -> u32 {
        world.difficulty_tier
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

    /// Returns the global pressure configuration stored by the world.
    #[must_use]
    pub fn pressure_config(world: &World) -> &PressureConfig {
        &world.pressure_config
    }

    /// Retrieves a cached attack plan for the provided wave identifier.
    #[must_use]
    pub fn attack_plan(world: &World, wave: WaveId) -> Option<&AttackPlan> {
        world.attack_plans.get(&wave).map(|stored| stored.plan())
    }

    /// Captures the wave seed derivation context for the next generated wave.
    #[must_use]
    pub fn wave_seed_context(world: &World) -> WaveSeedContext {
        WaveSeedContext::new(
            world.wave_seed_global,
            world.next_wave_id,
            world.difficulty_tier,
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
        BugColor, BugId, BugSnapshot, Direction, Goal, Health, NavigationFieldView,
        PendingWaveDifficulty, PlayMode, ProjectileId, ProjectileSnapshot, SpawnPatchId, SpeciesId,
        SpeciesTableVersion, TowerCooldownSnapshot, TowerId, TowerKind, WaveDifficulty, WaveId,
    };

    use std::{
        collections::hash_map::DefaultHasher,
        convert::TryFrom,
        hash::{Hash, Hasher},
        time::Duration,
    };

    fn navigation_fingerprint(view: &NavigationFieldView<'_>) -> u64 {
        let mut hasher = DefaultHasher::new();
        view.width().hash(&mut hasher);
        view.height().hash(&mut hasher);
        view.cells().hash(&mut hasher);
        hasher.finish()
    }

    fn expected_outer_rim(columns: u32, rows: u32) -> BTreeSet<CellCoord> {
        let mut cells = BTreeSet::new();

        if columns == 0 || rows == 0 {
            return cells;
        }

        let last_column = columns.saturating_sub(1);
        let last_row = rows.saturating_sub(1);

        for column in 0..columns {
            let _ = cells.insert(CellCoord::new(column, 0));
            let _ = cells.insert(CellCoord::new(column, last_row));
        }

        for row in 0..rows {
            let _ = cells.insert(CellCoord::new(0, row));
            let _ = cells.insert(CellCoord::new(last_column, row));
        }

        for exit_offset in 0..EXIT_CELL_LAYERS {
            if let Some(row) = rows.checked_sub(exit_offset + 1) {
                for column in 0..columns {
                    let _ = cells.remove(&CellCoord::new(column, row));
                }
            }
        }

        for border_offset in 0..BOTTOM_BORDER_CELL_LAYERS {
            if let Some(row) = rows.checked_sub(EXIT_CELL_LAYERS + border_offset + 1) {
                for column in 0..columns {
                    let _ = cells.remove(&CellCoord::new(column, row));
                }
            }
        }

        cells
    }

    fn world_step_ms(world: &World) -> u32 {
        u32::try_from(world.step_quantum.as_millis()).unwrap_or(u32::MAX)
    }

    fn ensure_attack_mode(world: &mut World, events: &mut Vec<Event>) {
        apply(
            world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            events,
        );
        events.clear();
    }

    #[test]
    fn species_table_query_reports_default_configuration() {
        let world = World::new();
        let table = query::species_table(&world);
        let mut definitions: Vec<_> = table.iter().collect();

        assert_eq!(table.version(), SpeciesTableVersion::new(1));
        assert_eq!(definitions.len(), 1);
        let definition = definitions.pop().expect("species definition");
        assert_eq!(definition.id(), SpeciesId::new(0));
        assert_eq!(definition.patch(), SpawnPatchId::new(0));
    }

    #[test]
    fn patch_table_query_reports_default_patch() {
        let world = World::new();
        let table = query::patch_table(&world);
        let descriptors: Vec<_> = table.iter().collect();

        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id(), SpawnPatchId::new(0));
    }

    #[test]
    fn pressure_config_query_exposes_defaults() {
        let world = World::new();
        let config = query::pressure_config(&world);
        let curve = config.curve();
        let burst = config.burst_scheduling();

        assert_eq!(curve.mean().get(), 1_200);
        assert_eq!(curve.deviation().get(), 250);
        assert_eq!(config.dirichlet_beta().get().get(), 2);
        assert_eq!(burst.nominal_burst_size().get(), 20);
        assert_eq!(burst.burst_count_max().get(), 8);
        assert_eq!(config.spawn_per_tick_max().get(), 2_000);
    }

    #[test]
    fn wave_seed_context_query_exposes_seed_inputs() {
        let world = World::new();
        let context = query::wave_seed_context(&world);

        assert_eq!(context.global_seed(), DEFAULT_WAVE_GLOBAL_SEED);
        assert_eq!(context.wave(), WaveId::new(0));
        assert_eq!(context.difficulty_tier(), 0);
    }

    fn strip_pending_wave_events(events: &mut Vec<Event>) {
        events.retain(|event| {
            !matches!(
                event,
                Event::PendingWaveDifficultyChanged { .. }
                    | Event::WaveStarted { .. }
                    | Event::PressureConfigChanged { .. }
            )
        });
    }

    fn gold_after_bug_death_at_tier(tier: u32) -> Gold {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(2, 2),
            },
            &mut events,
        );

        let tower = events
            .iter()
            .find_map(|event| match event {
                Event::TowerPlaced { tower, .. } => Some(*tower),
                _ => None,
            })
            .expect("tower placement should emit placement event");
        events.clear();

        ensure_attack_mode(&mut world, &mut events);

        world.set_gold_for_tests(Gold::ZERO);
        world.difficulty_tier = tier;

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected bug spawner for combat");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x33, 0x44, 0x55),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );

        let bug_id = match events.as_slice() {
            [Event::BugSpawned { bug_id, .. }] => *bug_id,
            other => panic!("unexpected events when spawning bug: {other:?}"),
        };
        events.clear();

        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );

        let projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => panic!("unexpected events when firing projectile: {other:?}"),
        };
        events.clear();

        let travel_time_ms = world
            .projectiles
            .get(&projectile)
            .expect("projectile state must exist")
            .travel_time_ms;
        let travel_time = Duration::from_millis(
            u64::try_from(travel_time_ms).expect("projectile travel time fits in u64"),
        );

        apply(&mut world, Command::Tick { dt: travel_time }, &mut events);

        let amount = events
            .iter()
            .find_map(|event| match event {
                Event::GoldChanged { amount } => Some(*amount),
                _ => None,
            })
            .expect("bug death should update gold");

        assert!(events
            .iter()
            .any(|event| matches!(event, Event::BugDied { bug } if *bug == bug_id)));
        assert_eq!(query::gold(&world), amount);

        amount
    }

    #[test]
    fn bug_kill_reward_scales_with_difficulty_tier() {
        assert_eq!(gold_after_bug_death_at_tier(0), Gold::new(1));
        assert_eq!(gold_after_bug_death_at_tier(3), Gold::new(4));
        assert_eq!(gold_after_bug_death_at_tier(7), Gold::new(8));
    }

    #[test]
    fn bug_kill_reward_saturates_at_multiplier_limit() {
        assert_eq!(gold_after_bug_death_at_tier(u32::MAX), Gold::new(u32::MAX));
    }

    #[test]
    fn hard_wave_scales_bug_reward_using_effective_tier() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(2, 2),
            },
            &mut events,
        );

        let tower = events
            .iter()
            .find_map(|event| match event {
                Event::TowerPlaced { tower, .. } => Some(*tower),
                _ => None,
            })
            .expect("tower placement should emit placement event");
        events.clear();

        ensure_attack_mode(&mut world, &mut events);

        world.set_gold_for_tests(Gold::ZERO);
        world.difficulty_tier = 3;

        let wave = query::wave_seed_context(&world).wave();

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Hard,
            },
            &mut events,
        );
        strip_pending_wave_events(&mut events);

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected bug spawner for combat");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x33, 0x44, 0x55),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );

        let bug_id = match events.as_slice() {
            [Event::BugSpawned { bug_id, .. }] => *bug_id,
            other => panic!("unexpected events when spawning bug: {other:?}"),
        };
        events.clear();

        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );

        let projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => panic!("unexpected events when firing projectile: {other:?}"),
        };
        events.clear();

        let travel_time_ms = world
            .projectiles
            .get(&projectile)
            .expect("projectile state must exist")
            .travel_time_ms;
        let travel_time = Duration::from_millis(
            u64::try_from(travel_time_ms).expect("projectile travel time fits in u64"),
        );

        apply(&mut world, Command::Tick { dt: travel_time }, &mut events);

        let amount = events
            .iter()
            .find_map(|event| match event {
                Event::GoldChanged { amount } => Some(*amount),
                _ => None,
            })
            .expect("bug death should update gold");

        assert_eq!(amount, Gold::new(5));
    }

    #[test]
    fn resolving_normal_round_win_keeps_tier() {
        let mut world = World::new();
        let mut events = Vec::new();

        world.difficulty_tier = 2;

        apply(
            &mut world,
            Command::ResolveRound {
                outcome: RoundOutcome::Win,
            },
            &mut events,
        );

        assert_eq!(query::difficulty_tier(&world), 2);
        assert!(events
            .iter()
            .all(|event| !matches!(event, Event::DifficultyTierChanged { .. })));
    }

    #[test]
    fn hard_wave_win_promotes_tier_and_emits_event() {
        let mut world = World::new();
        let mut events = Vec::new();

        world.difficulty_tier = 2;

        let wave = query::wave_seed_context(&world).wave();

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Hard,
            },
            &mut events,
        );

        events.clear();

        apply(
            &mut world,
            Command::ResolveRound {
                outcome: RoundOutcome::Win,
            },
            &mut events,
        );

        assert_eq!(query::difficulty_tier(&world), 3);
        assert!(
            world.active_wave.is_none(),
            "hard win should clear active wave"
        );

        let hard_win = events.iter().find_map(|event| match event {
            Event::HardWinAchieved {
                wave,
                previous_tier,
                new_tier,
            } => Some((*wave, *previous_tier, *new_tier)),
            _ => None,
        });
        let (wave_id, previous_tier, new_tier) =
            hard_win.expect("hard victory must emit HardWinAchieved event");
        assert_eq!(wave_id, WaveId::new(0));
        assert_eq!(previous_tier, 2);
        assert_eq!(new_tier, 3);
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Event::DifficultyTierChanged { tier } if *tier == 3
            )
        }));
    }

    #[test]
    fn hard_wave_win_emits_event_even_when_tier_saturated() {
        let mut world = World::new();
        let mut events = Vec::new();

        world.difficulty_tier = u32::MAX;

        let wave = query::wave_seed_context(&world).wave();

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Hard,
            },
            &mut events,
        );

        events.clear();

        apply(
            &mut world,
            Command::ResolveRound {
                outcome: RoundOutcome::Win,
            },
            &mut events,
        );

        assert_eq!(query::difficulty_tier(&world), u32::MAX);
        assert!(world.active_wave.is_none());

        let hard_win = events.iter().find_map(|event| match event {
            Event::HardWinAchieved {
                wave,
                previous_tier,
                new_tier,
            } => Some((*wave, *previous_tier, *new_tier)),
            _ => None,
        });
        let (wave_id, previous_tier, new_tier) =
            hard_win.expect("hard victory must emit HardWinAchieved event");
        assert_eq!(wave_id, WaveId::new(0));
        assert_eq!(previous_tier, u32::MAX);
        assert_eq!(new_tier, u32::MAX);
        assert!(events
            .iter()
            .all(|event| !matches!(event, Event::DifficultyTierChanged { .. })));
    }

    #[test]
    fn resolving_round_loss_decrements_tier_and_removes_towers() {
        let mut world = World::new();
        let mut events = Vec::new();

        let origins = [
            CellCoord::new(2, 2),
            CellCoord::new(6, 2),
            CellCoord::new(2, 6),
            CellCoord::new(6, 6),
        ];

        let mut tower_ids = Vec::new();
        for origin in origins {
            apply(
                &mut world,
                Command::PlaceTower {
                    kind: TowerKind::Basic,
                    origin,
                },
                &mut events,
            );

            let placed = events
                .iter()
                .find_map(|event| match event {
                    Event::TowerPlaced { tower, .. } => Some(*tower),
                    _ => None,
                })
                .expect("placement emits tower identifier");
            tower_ids.push(placed);
            events.clear();
        }

        assert_eq!(
            tower_ids,
            vec![
                TowerId::new(0),
                TowerId::new(1),
                TowerId::new(2),
                TowerId::new(3),
            ]
        );

        world.difficulty_tier = 3;

        apply(
            &mut world,
            Command::ResolveRound {
                outcome: RoundOutcome::Loss,
            },
            &mut events,
        );

        assert_eq!(query::difficulty_tier(&world), 2);

        let removed_towers: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::TowerRemoved { tower, .. } => Some(*tower),
                _ => None,
            })
            .collect();
        assert_eq!(removed_towers, vec![TowerId::new(3), TowerId::new(2)]);

        assert!(events.iter().any(|event| {
            matches!(
                event,
                Event::DifficultyTierChanged { tier } if *tier == 2
            )
        }));

        assert!(world.towers.get(TowerId::new(3)).is_none());
        assert!(world.towers.get(TowerId::new(2)).is_none());
        assert!(world.towers.get(TowerId::new(1)).is_some());
        assert!(world.towers.get(TowerId::new(0)).is_some());

        events.clear();
    }

    #[test]
    fn resolving_round_loss_at_zero_tier_does_not_emit_tier_change() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ResolveRound {
                outcome: RoundOutcome::Loss,
            },
            &mut events,
        );

        assert_eq!(query::difficulty_tier(&world), 0);
        assert!(events
            .iter()
            .all(|event| !matches!(event, Event::DifficultyTierChanged { .. })));
    }

    #[test]
    fn start_wave_records_pending_difficulty_and_emits_event() {
        let mut world = World::new();
        let mut events = Vec::new();

        let wave = query::wave_seed_context(&world).wave();

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Hard,
            },
            &mut events,
        );

        assert_eq!(
            query::pending_wave_difficulty(&world),
            PendingWaveDifficulty::Selected(WaveDifficulty::Hard)
        );
        let pending = events.iter().any(|event| {
            matches!(
                event,
                Event::PendingWaveDifficultyChanged {
                    pending: PendingWaveDifficulty::Selected(WaveDifficulty::Hard)
                }
            )
        });
        assert!(pending, "expected pending difficulty event");
        let wave_started = events.iter().find_map(|event| match event {
            Event::WaveStarted {
                wave,
                difficulty,
                tier_effective,
                reward_multiplier,
                pressure_scalar,
                plan_pressure,
                plan_species_table_version,
                plan_burst_count,
            } => Some((
                *wave,
                *difficulty,
                *tier_effective,
                *reward_multiplier,
                *pressure_scalar,
                *plan_pressure,
                *plan_species_table_version,
                *plan_burst_count,
            )),
            _ => None,
        });
        let (
            wave_id,
            difficulty,
            tier_effective,
            reward_multiplier,
            pressure_scalar,
            plan_pressure,
            plan_species_table_version,
            plan_burst_count,
        ) = wave_started.expect("wave start event must be emitted");
        assert_eq!(wave_id, WaveId::new(0));
        assert_eq!(difficulty, WaveDifficulty::Hard);
        assert_eq!(tier_effective, 1);
        assert_eq!(reward_multiplier, 2);
        assert_eq!(pressure_scalar, 2);
        assert_eq!(plan_pressure, Pressure::new(0));
        assert_eq!(plan_species_table_version, world.species_table_version);
        assert_eq!(plan_burst_count, 0);
        let active_wave = world
            .active_wave
            .expect("launching a wave should record active context");
        assert_eq!(active_wave.id, wave_id);
        assert_eq!(active_wave.difficulty, WaveDifficulty::Hard);
        assert_eq!(active_wave.reward_multiplier, reward_multiplier);
    }

    #[test]
    fn cached_attack_plan_is_persisted_and_reflected_in_events() {
        let mut world = World::new();
        let mut events = Vec::new();

        let wave = query::wave_seed_context(&world).wave();
        let bursts = vec![BurstPlan::new(
            SpeciesId::new(0),
            SpawnPatchId::new(0),
            NonZeroU32::new(3).expect("burst count"),
            NonZeroU32::new(400).expect("cadence"),
            250,
        )];
        let plan = AttackPlan::new(Pressure::new(1_500), world.species_table_version, bursts);

        apply(
            &mut world,
            Command::CacheAttackPlan {
                wave,
                difficulty: WaveDifficulty::Normal,
                plan: plan.clone(),
            },
            &mut events,
        );

        assert!(events.is_empty(), "caching a plan should not emit events");
        let stored_plan = query::attack_plan(&world, wave).expect("plan should be cached");
        assert_eq!(stored_plan, &plan);

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Normal,
            },
            &mut events,
        );

        let wave_started = events.iter().find_map(|event| match event {
            Event::WaveStarted {
                wave,
                difficulty,
                tier_effective,
                reward_multiplier,
                pressure_scalar,
                plan_pressure,
                plan_species_table_version,
                plan_burst_count,
            } => Some((
                *wave,
                *difficulty,
                *tier_effective,
                *reward_multiplier,
                *pressure_scalar,
                *plan_pressure,
                *plan_species_table_version,
                *plan_burst_count,
            )),
            _ => None,
        });

        let (
            wave_id,
            difficulty,
            tier_effective,
            reward_multiplier,
            pressure_scalar,
            plan_pressure,
            plan_species_table_version,
            plan_burst_count,
        ) = wave_started.expect("wave start event must be emitted");

        assert_eq!(wave_id, wave);
        assert_eq!(difficulty, WaveDifficulty::Normal);
        assert_eq!(tier_effective, 0);
        assert_eq!(reward_multiplier, 1);
        assert_eq!(pressure_scalar, 1);
        assert_eq!(plan_pressure, plan.pressure());
        assert_eq!(plan_species_table_version, plan.species_table_version());
        assert_eq!(plan_burst_count, 1);
    }

    #[test]
    fn configuring_tile_grid_resets_pending_wave_difficulty() {
        let mut world = World::new();
        let mut events = Vec::new();

        let wave = query::wave_seed_context(&world).wave();

        apply(
            &mut world,
            Command::StartWave {
                wave,
                difficulty: WaveDifficulty::Normal,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(10),
                rows: TileCoord::new(10),
                tile_length: 100.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        assert_eq!(
            query::pending_wave_difficulty(&world),
            PendingWaveDifficulty::Unset
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Event::PendingWaveDifficultyChanged {
                    pending: PendingWaveDifficulty::Unset
                }
            )
        }));
    }

    #[test]
    fn build_cell_walls_returns_empty_when_dimensions_zero() {
        assert!(build_cell_walls(TileCoord::new(0), TileCoord::new(1), 1).is_empty());
        assert!(build_cell_walls(TileCoord::new(1), TileCoord::new(0), 1).is_empty());
        assert!(build_cell_walls(TileCoord::new(1), TileCoord::new(1), 0).is_empty());
    }

    #[test]
    fn build_cell_walls_spans_visible_row_with_gap() {
        let columns = TileCoord::new(5);
        let rows = TileCoord::new(4);
        let cells_per_tile = 2;

        let walls = build_cell_walls(columns, rows, cells_per_tile);
        let total_columns = total_cell_columns(columns, cells_per_tile);
        let visible_wall_row = visible_wall_row_for_tile_grid(rows, cells_per_tile)
            .expect("expected visible wall row for configured grid");
        let exit_columns = exit_columns_for_tile_grid(columns, cells_per_tile);

        assert_eq!(
            exit_columns.len(),
            usize::try_from(cells_per_tile).expect("cells_per_tile fits in usize"),
        );

        let expected_cells: Vec<CellCoord> = (0..total_columns)
            .filter(|column| exit_columns.binary_search(column).is_err())
            .map(|column| CellCoord::new(column, visible_wall_row))
            .collect();
        let actual_cells: Vec<CellCoord> = walls.iter().map(|wall| wall.cell()).collect();

        assert_eq!(actual_cells, expected_cells);
    }

    #[test]
    fn world_defaults_to_builder_mode() {
        let world = World::new();

        assert_eq!(query::play_mode(&world), PlayMode::Builder);
    }

    #[test]
    fn world_starts_without_bugs() {
        let world = World::new();

        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn world_starts_without_projectiles() {
        let world = World::new();

        assert!(world.projectiles.is_empty());
        assert_eq!(world.next_projectile_id, ProjectileId::new(0));
    }

    #[test]
    fn bug_view_reports_health_and_filters_dead_bugs() {
        let mut world = World::new();
        let mut events = Vec::new();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected bug spawner");
        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0xaa, 0xbb, 0xcc),
                health: Health::new(5),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.health, Health::new(5));

        world.bugs[0].health = Health::ZERO;
        let filtered = query::bug_view(&world).into_vec();
        assert!(filtered.is_empty());
    }

    #[test]
    fn bug_view_carries_cadence_state_through_queries() {
        let mut world = World::new();
        let mut events = Vec::new();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected bug spawner");
        let step_ms = 320;
        ensure_attack_mode(&mut world, &mut events);

        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x44, 0x55, 0x66),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        let bug_id = events
            .iter()
            .find_map(|event| {
                if let Event::BugSpawned { bug_id, .. } = event {
                    Some(*bug_id)
                } else {
                    None
                }
            })
            .expect("spawn should emit bug identifier");
        events.clear();

        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.step_ms, step_ms);
        assert_eq!(snapshot.accum_ms, step_ms);
        assert!(snapshot.ready_for_step);

        let index = world
            .bug_index(bug_id)
            .expect("bug should be tracked after spawning");
        world.bugs[index].accum_ms = 0;

        let stalled = query::bug_view(&world).into_vec();
        assert_eq!(stalled.len(), 1);
        let snapshot = &stalled[0];
        assert_eq!(snapshot.accum_ms, 0);
        assert!(!snapshot.ready_for_step);

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(u64::from(step_ms) * 2),
            },
            &mut events,
        );
        events.clear();

        let refreshed = query::bug_view(&world).into_vec();
        assert_eq!(refreshed.len(), 1);
        let snapshot = &refreshed[0];
        assert_eq!(snapshot.accum_ms, step_ms);
        assert!(snapshot.ready_for_step);
    }

    #[test]
    fn projectile_identifiers_increment_monotonically() {
        let mut world = World::new();

        let first = world.next_projectile_identifier();
        let second = world.next_projectile_identifier();

        assert_eq!(first, ProjectileId::new(0));
        assert_eq!(second, ProjectileId::new(1));
        assert_eq!(world.next_projectile_id, ProjectileId::new(2));
    }

    #[test]
    fn default_cells_per_tile_is_one() {
        let world = World::new();

        assert_eq!(query::cells_per_tile(&world), 1);
    }

    #[test]
    fn configured_cells_per_tile_reflects_world_settings() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: DEFAULT_GRID_COLUMNS,
                rows: DEFAULT_GRID_ROWS,
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 4,
            },
            &mut events,
        );

        assert_eq!(query::cells_per_tile(&world), 4);
    }

    #[test]
    fn zero_cells_per_tile_is_normalised_to_one() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: DEFAULT_GRID_COLUMNS,
                rows: DEFAULT_GRID_ROWS,
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 0,
            },
            &mut events,
        );

        assert_eq!(query::cells_per_tile(&world), 1);
    }

    #[test]
    fn tower_occupancy_does_not_block_when_empty() {
        let world = World::new();
        let occupancy = query::occupancy_view(&world);
        let (columns, rows) = occupancy.dimensions();
        let wall_cells: Vec<CellCoord> = query::walls(&world)
            .iter()
            .map(|wall| wall.cell())
            .collect();

        for column in 0..columns {
            for row in 0..rows {
                let cell = CellCoord::new(column, row);
                let blocked_by_wall = wall_cells.contains(&cell);
                let expected = !occupancy.is_free(cell) || blocked_by_wall;
                assert_eq!(query::is_cell_blocked(&world, cell), expected);
            }
        }
    }

    #[test]
    fn wall_cells_block_cell_queries() {
        let mut world = World::new();
        let (columns, rows) = world.occupancy.dimensions();
        world
            .walls
            .rebuild(columns, rows, vec![CellWall::at(CellCoord::new(1, 1))]);

        assert!(query::is_cell_blocked(&world, CellCoord::new(1, 1)));

        let view = query::walls(&world);
        let cells: Vec<CellCoord> = view.iter().map(|wall| wall.cell()).collect();
        assert_eq!(cells, vec![CellCoord::new(1, 1)]);
    }

    #[test]
    fn navigation_field_query_borrows_world_buffer() {
        let world = World::new();

        let view = query::navigation_field(&world);

        assert_eq!(view.width(), world.navigation_field.width());
        assert_eq!(view.height(), world.navigation_field.height());
        assert!(std::ptr::eq(
            view.cells().as_ptr(),
            world.navigation_field.cells().as_ptr()
        ));
    }

    #[test]
    fn navigation_field_descends_toward_exit() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(3),
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 1,
            },
            &mut events,
        );

        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());

        let expected_columns = total_cell_columns(TileCoord::new(3), 1);
        let expected_rows = total_cell_rows(TileCoord::new(3), 1);
        assert_eq!(world.navigation_field.width(), expected_columns);
        assert_eq!(world.navigation_field.height(), expected_rows);
        assert!(!world.navigation_dirty);

        let exit_row = exit_row_for_tile_grid(TileCoord::new(3), 1);
        let exit_columns = exit_columns_for_tile_grid(TileCoord::new(3), 1);
        for column in exit_columns.iter().copied() {
            let exit_cell = CellCoord::new(column, exit_row);
            assert_eq!(
                world.navigation_field.distance(exit_cell),
                Some(0),
                "exit cells must be zero distance",
            );
        }

        let visible_wall_row = visible_wall_row_for_tile_grid(TileCoord::new(3), 1)
            .expect("visible wall row must exist");
        let walkway_row =
            walkway_row_for_tile_grid(TileCoord::new(3), 1).expect("walkway row must exist");
        assert!(walkway_row > 0, "walkway row should have interior above");
        let interior_row = walkway_row - 1;

        let exit_column = exit_columns[0];
        let exit_distance = world
            .navigation_field
            .distance(CellCoord::new(exit_column, exit_row))
            .expect("exit distance available");
        let wall_distance = world
            .navigation_field
            .distance(CellCoord::new(exit_column, visible_wall_row))
            .expect("wall distance available");
        let walkway_distance = world
            .navigation_field
            .distance(CellCoord::new(exit_column, walkway_row))
            .expect("walkway distance available");
        let interior_distance = world
            .navigation_field
            .distance(CellCoord::new(exit_column, interior_row))
            .expect("interior distance available");

        assert!(exit_distance < wall_distance);
        assert!(wall_distance < walkway_distance);
        assert!(walkway_distance < interior_distance);

        let blocked_column = (0..expected_columns)
            .find(|column| !exit_columns.contains(column))
            .expect("non-exit column should exist");
        let blocked_cell = CellCoord::new(blocked_column, visible_wall_row);
        assert_eq!(
            world.navigation_field.distance(blocked_cell),
            Some(u16::MAX),
            "static walls remain unreachable",
        );
    }

    #[test]
    fn navigation_field_updates_when_grid_resized() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(3),
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 1,
            },
            &mut events,
        );
        events.clear();

        let initial_width = world.navigation_field.width();
        let initial_height = world.navigation_field.height();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(4),
                rows: TileCoord::new(2),
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 2,
            },
            &mut events,
        );

        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());

        let expected_width = total_cell_columns(TileCoord::new(4), 2);
        let expected_height = total_cell_rows(TileCoord::new(2), 2);

        assert_ne!(world.navigation_field.width(), initial_width);
        assert_ne!(world.navigation_field.height(), initial_height);
        assert_eq!(world.navigation_field.width(), expected_width);
        assert_eq!(world.navigation_field.height(), expected_height);
        assert!(!world.navigation_dirty);
    }

    #[test]
    fn navigation_field_query_updates_after_world_changes() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(2),
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 1,
            },
            &mut events,
        );
        events.clear();

        let initial = query::navigation_field(&world);
        let first_dimensions = (initial.width(), initial.height());
        let exit_row = exit_row_for_tile_grid(TileCoord::new(2), 1);
        let exit_columns = exit_columns_for_tile_grid(TileCoord::new(3), 1);
        for column in exit_columns.iter().copied() {
            assert_eq!(
                initial.distance(CellCoord::new(column, exit_row)),
                Some(0),
                "exit cell distance should be zero",
            );
        }
        drop(initial);

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(4),
                rows: TileCoord::new(3),
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 2,
            },
            &mut events,
        );

        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());

        let updated = query::navigation_field(&world);
        let expected_dimensions = (
            total_cell_columns(TileCoord::new(4), 2),
            total_cell_rows(TileCoord::new(3), 2),
        );
        assert_ne!(first_dimensions, expected_dimensions);
        assert_eq!((updated.width(), updated.height()), expected_dimensions);

        let new_exit_row = exit_row_for_tile_grid(TileCoord::new(3), 2);
        let new_exit_columns = exit_columns_for_tile_grid(TileCoord::new(4), 2);
        for column in new_exit_columns.iter().copied() {
            assert_eq!(
                updated.distance(CellCoord::new(column, new_exit_row)),
                Some(0),
                "exit cells remain zero after rebuild",
            );
        }
    }

    #[test]
    fn navigation_field_blocks_tower_cells_after_builder_edits() {
        let mut world = World::new();
        let mut events = Vec::new();

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }],
        );
        events.clear();

        let origin = CellCoord::new(2, 2);
        let footprint = footprint_for(TowerKind::Basic);

        let (columns, rows) = world.tower_occupancy.dimensions();
        assert!(
            origin.column().saturating_add(footprint.width()) <= columns,
            "tower footprint must remain within the maze columns",
        );
        assert!(
            origin.row().saturating_add(footprint.height()) <= rows,
            "tower footprint must remain within the maze rows",
        );

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![
                Event::GoldChanged {
                    amount: Gold::new(90),
                },
                Event::TowerPlaced {
                    tower: TowerId::new(0),
                    kind: TowerKind::Basic,
                    region: CellRect::from_origin_and_size(origin, footprint),
                },
            ],
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Attack,
            }],
        );
        assert!(
            !world.navigation_dirty,
            "navigation rebuild should complete when switching to attack",
        );

        let navigation = query::navigation_field(&world);
        for column_offset in 0..footprint.width() {
            for row_offset in 0..footprint.height() {
                let cell = CellCoord::new(
                    origin.column().saturating_add(column_offset),
                    origin.row().saturating_add(row_offset),
                );
                assert_eq!(
                    navigation.distance(cell),
                    Some(u16::MAX),
                    "tower-covered cell should be unreachable",
                );
            }
        }
    }

    #[test]
    fn cleanup_dead_bugs_reclaims_occupancy() {
        let mut world = World::new();
        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected at least one spawner");
        let mut events = Vec::new();
        let step_ms = world_step_ms(&world);

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0, 255, 0),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );

        assert_eq!(world.bugs.len(), 1);
        world.bugs[0].health = Health::ZERO;

        world.cleanup_dead_bugs();

        assert!(world.bugs.is_empty());
        assert!(world.bug_positions.is_empty());
        assert!(world.occupancy.can_enter(spawner));
    }

    #[test]
    fn fire_projectile_rejects_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(3);
        let target = BugId::new(2);
        apply(
            &mut world,
            Command::FireProjectile { tower, target },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::InvalidMode,
            }]
        );
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn fire_projectile_rejects_when_tower_missing() {
        let mut world = World::new();
        let mut events = Vec::new();

        ensure_attack_mode(&mut world, &mut events);
        let tower = TowerId::new(9);
        let target = BugId::new(4);
        apply(
            &mut world,
            Command::FireProjectile { tower, target },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::MissingTower,
            }]
        );
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn fire_projectile_rejects_when_cooldown_active() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let cooldown = Duration::from_millis(125);
        world
            .towers
            .get_mut(tower)
            .expect("tower present")
            .cooldown_remaining = cooldown;

        let target = BugId::new(6);
        apply(
            &mut world,
            Command::FireProjectile { tower, target },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::CooldownActive,
            }]
        );
        assert!(world.projectiles.is_empty());
        assert_eq!(
            world
                .towers
                .get(tower)
                .expect("tower present")
                .cooldown_remaining,
            cooldown
        );
    }

    #[test]
    fn fire_projectile_rejects_when_target_missing() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(4, 4);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let target = BugId::new(42);
        apply(
            &mut world,
            Command::FireProjectile { tower, target },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::ProjectileRejected {
                tower,
                target,
                reason: ProjectileRejection::MissingTarget,
            }]
        );
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn fire_projectile_rejects_when_bug_is_dead() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(2, 4);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x44, 0x44, 0x44),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let bug_id = world.bugs[0].id;
        world.bugs[0].health = Health::ZERO;

        let tower = TowerId::new(0);
        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::ProjectileRejected {
                tower,
                target: bug_id,
                reason: ProjectileRejection::MissingTarget,
            }]
        );
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn fire_projectile_spawns_projectile_and_resets_cooldown() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(3, 3);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0xaa, 0xbb, 0xcc),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let bug_id = world.bugs[0].id;
        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );

        let projectile = ProjectileId::new(0);
        assert_eq!(
            events,
            vec![Event::ProjectileFired {
                projectile,
                tower,
                target: bug_id,
            }]
        );
        assert_eq!(world.projectiles.len(), 1);
        let state = world
            .projectiles
            .get(&projectile)
            .expect("projectile stored");

        let tower_region = world.towers.get(tower).expect("tower present").region;
        let expected_start = super::tower_center_half(tower_region);
        let bug_cell = world.bugs[0].cell;
        let expected_end = super::bug_center_half(bug_cell);
        let expected_distance = expected_start.distance_to(expected_end);

        assert_eq!(state.id, projectile);
        assert_eq!(state.tower, tower);
        assert_eq!(state.target, bug_id);
        assert_eq!(state.start, expected_start);
        assert_eq!(state.end, expected_end);
        assert_eq!(state.distance_half, expected_distance);
        assert_eq!(state.travelled_half, 0);
        let max_range_half =
            u128::from(TowerKind::Basic.range_in_cells(query::cells_per_tile(&world)))
                .saturating_mul(2);
        let expected_time = super::compute_projectile_travel_time(
            expected_distance,
            max_range_half,
            u128::from(TowerKind::Basic.projectile_travel_time_ms()),
        );
        assert_eq!(state.travel_time_ms, expected_time);
        assert_eq!(state.elapsed_ms, 0);
        assert_eq!(state.damage, TowerKind::Basic.projectile_damage());

        let cooldown = Duration::from_millis(u64::from(TowerKind::Basic.fire_cooldown_ms()));
        assert_eq!(
            world
                .towers
                .get(tower)
                .expect("tower present")
                .cooldown_remaining,
            cooldown
        );
        assert_eq!(world.next_projectile_id, ProjectileId::new(1));
    }

    #[test]
    fn tower_cooldown_query_reflects_remaining_duration() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(2, 2),
            },
            &mut events,
        );
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(6, 2),
            },
            &mut events,
        );
        events.clear();

        let first = TowerId::new(0);
        let second = TowerId::new(1);
        world
            .towers
            .get_mut(first)
            .expect("first tower present")
            .cooldown_remaining = Duration::from_millis(750);
        world
            .towers
            .get_mut(second)
            .expect("second tower present")
            .cooldown_remaining = Duration::from_millis(250);

        let view = query::tower_cooldowns(&world);
        let snapshots = view.into_vec();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].tower, first);
        assert_eq!(snapshots[0].kind, TowerKind::Basic);
        assert_eq!(snapshots[0].ready_in, Duration::from_millis(750));
        assert_eq!(snapshots[1].tower, second);
        assert_eq!(snapshots[1].kind, TowerKind::Basic);
        assert_eq!(snapshots[1].ready_in, Duration::from_millis(250));
    }

    #[test]
    fn projectile_query_reports_snapshots_in_id_order() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(2, 2),
            },
            &mut events,
        );
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(6, 2),
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x11, 0x22, 0x33),
                health: Health::new(6),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let bug_id = world.bugs[0].id;
        let original_cell = world.bugs[0].cell;
        world.occupancy.vacate(original_cell);
        let target_cell = CellCoord::new(8, 4);
        world.bugs[0].cell = target_cell;
        world.occupancy.occupy(bug_id, target_cell);

        apply(
            &mut world,
            Command::FireProjectile {
                tower: TowerId::new(0),
                target: bug_id,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::FireProjectile {
                tower: TowerId::new(1),
                target: bug_id,
            },
            &mut events,
        );
        events.clear();

        let snapshots: Vec<_> = query::projectiles(&world).collect();
        assert_eq!(snapshots.len(), 2);

        let bug_end = super::bug_center_half(target_cell);

        let first_region = world
            .towers
            .get(TowerId::new(0))
            .expect("first tower present")
            .region;
        let second_region = world
            .towers
            .get(TowerId::new(1))
            .expect("second tower present")
            .region;
        let first_start = super::tower_center_half(first_region);
        let second_start = super::tower_center_half(second_region);

        assert_eq!(snapshots[0].projectile, ProjectileId::new(0));
        assert_eq!(snapshots[0].tower, TowerId::new(0));
        assert_eq!(snapshots[0].target, bug_id);
        assert_eq!(snapshots[0].origin_half, first_start);
        assert_eq!(snapshots[0].dest_half, bug_end);
        assert_eq!(snapshots[0].travelled_half, 0);
        assert_eq!(snapshots[0].distance_half, first_start.distance_to(bug_end),);

        assert_eq!(snapshots[1].projectile, ProjectileId::new(1));
        assert_eq!(snapshots[1].tower, TowerId::new(1));
        assert_eq!(snapshots[1].target, bug_id);
        assert_eq!(snapshots[1].origin_half, second_start);
        assert_eq!(snapshots[1].dest_half, bug_end);
        assert_eq!(snapshots[1].travelled_half, 0);
        assert_eq!(
            snapshots[1].distance_half,
            second_start.distance_to(bug_end),
        );
    }

    #[test]
    fn tick_updates_tower_cooldowns_only_in_attack_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        world
            .towers
            .get_mut(tower)
            .expect("tower present")
            .cooldown_remaining = Duration::from_millis(600);

        let attack_dt = Duration::from_millis(250);
        apply(&mut world, Command::Tick { dt: attack_dt }, &mut events);

        assert_eq!(events, vec![Event::TimeAdvanced { dt: attack_dt }]);
        assert_eq!(
            world
                .towers
                .get(tower)
                .expect("tower present")
                .cooldown_remaining,
            Duration::from_millis(350),
        );

        events.clear();
        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        world
            .towers
            .get_mut(tower)
            .expect("tower present")
            .cooldown_remaining = Duration::from_millis(200);

        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(150),
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert_eq!(
            world
                .towers
                .get(tower)
                .expect("tower present")
                .cooldown_remaining,
            Duration::from_millis(200),
        );

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let resume_dt = Duration::from_millis(250);
        apply(&mut world, Command::Tick { dt: resume_dt }, &mut events);

        assert_eq!(events, vec![Event::TimeAdvanced { dt: resume_dt }]);
        assert_eq!(
            world
                .towers
                .get(tower)
                .expect("tower present")
                .cooldown_remaining,
            Duration::ZERO,
        );
    }

    #[test]
    fn tick_advances_projectiles_and_damages_bugs() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(3, 3);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x55, 0x66, 0x77),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let bug_id = world.bugs[0].id;
        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );
        events.clear();

        let projectile_id = *world.projectiles.keys().next().expect("projectile stored");
        let projectile = world
            .projectiles
            .get(&projectile_id)
            .expect("projectile present");

        let travel_ms = projectile.travel_time_ms;
        assert!(travel_ms > 0, "projectile travel time should be positive");
        let dt = Duration::from_millis(u64::try_from(travel_ms).expect("travel time fits in u64"));

        apply(&mut world, Command::Tick { dt }, &mut events);

        let damage = TowerKind::Basic.projectile_damage();
        assert_eq!(
            events,
            vec![
                Event::TimeAdvanced { dt },
                Event::BugDamaged {
                    bug: bug_id,
                    remaining: Health::new(2),
                },
                Event::ProjectileHit {
                    projectile: projectile_id,
                    target: bug_id,
                    damage,
                },
            ],
        );
        assert!(world.projectiles.is_empty());
        assert_eq!(world.bugs.len(), 1);
        assert_eq!(world.bugs[0].health, Health::new(2));
        assert!(!world.occupancy.can_enter(world.bugs[0].cell));
    }

    #[test]
    fn tick_kills_bug_and_removes_projectile_on_hit() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(4, 4);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x10, 0x20, 0x30),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let bug_id = world.bugs[0].id;
        let bug_cell = world.bugs[0].cell;
        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );
        events.clear();

        let projectile_id = *world.projectiles.keys().next().expect("projectile stored");
        let projectile = world
            .projectiles
            .get(&projectile_id)
            .expect("projectile present");
        let travel_ms = projectile.travel_time_ms;
        assert!(travel_ms > 0, "projectile travel time should be positive");
        let dt = Duration::from_millis(u64::try_from(travel_ms).expect("travel time fits in u64"));

        apply(&mut world, Command::Tick { dt }, &mut events);

        let damage = TowerKind::Basic.projectile_damage();
        assert_eq!(
            events,
            vec![
                Event::TimeAdvanced { dt },
                Event::BugDamaged {
                    bug: bug_id,
                    remaining: Health::ZERO,
                },
                Event::GoldChanged {
                    amount: Gold::new(91),
                },
                Event::BugDied { bug: bug_id },
                Event::ProjectileHit {
                    projectile: projectile_id,
                    target: bug_id,
                    damage,
                },
            ],
        );
        assert!(world.projectiles.is_empty());
        assert!(world.bugs.is_empty());
        assert!(world.bug_positions.is_empty());
        assert!(world.occupancy.can_enter(bug_cell));
    }

    #[test]
    fn tick_expires_projectiles_when_target_dead() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(5, 5);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x40, 0x50, 0x60),
                health: Health::new(2),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        let tower = TowerId::new(0);
        let bug_id = world.bugs[0].id;
        apply(
            &mut world,
            Command::FireProjectile {
                tower,
                target: bug_id,
            },
            &mut events,
        );
        events.clear();

        world.bugs[0].health = Health::ZERO;

        let projectile_id = *world.projectiles.keys().next().expect("projectile stored");
        let projectile = world
            .projectiles
            .get(&projectile_id)
            .expect("projectile present");
        let travel_ms = projectile.travel_time_ms;
        assert!(travel_ms > 0, "projectile travel time should be positive");
        let dt = Duration::from_millis(u64::try_from(travel_ms).expect("travel time fits in u64"));

        apply(&mut world, Command::Tick { dt }, &mut events);

        assert_eq!(
            events,
            vec![
                Event::TimeAdvanced { dt },
                Event::ProjectileExpired {
                    projectile: projectile_id,
                },
            ],
        );
        assert!(world.projectiles.is_empty());
        assert_eq!(world.bugs.len(), 1);
        assert_eq!(world.bugs[0].health, Health::ZERO);
    }

    #[test]
    fn tick_resolves_simultaneous_projectiles_in_id_order() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );
        let first_tower = match events.as_slice() {
            [Event::GoldChanged { amount }, Event::TowerPlaced { tower, .. }]
                if *amount == Gold::new(90) =>
            {
                *tower
            }
            other => panic!("unexpected events when placing first tower: {other:?}"),
        };
        events.clear();

        let second_origin = CellCoord::new(6, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );
        let second_tower = match events.as_slice() {
            [Event::GoldChanged { amount }, Event::TowerPlaced { tower, .. }]
                if *amount == Gold::new(80) =>
            {
                *tower
            }
            other => panic!("unexpected events when placing second tower: {other:?}"),
        };
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected spawner");
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner,
                color: BugColor::from_rgb(0x55, 0x22, 0x11),
                health: Health::new(1),
                step_ms,
            },
            &mut events,
        );
        let bug_id = match events.as_slice() {
            [Event::BugSpawned { bug_id, .. }] => *bug_id,
            other => panic!("unexpected events when spawning bug: {other:?}"),
        };
        events.clear();

        let original_cell = world.bugs[0].cell;
        let target_cell = CellCoord::new(9, 9);
        world.bugs[0].cell = target_cell;
        world.occupancy.vacate(original_cell);
        world.occupancy.occupy(bug_id, target_cell);

        apply(
            &mut world,
            Command::FireProjectile {
                tower: first_tower,
                target: bug_id,
            },
            &mut events,
        );
        let first_projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => panic!("unexpected events when firing projectile from first tower: {other:?}"),
        };
        events.clear();

        apply(
            &mut world,
            Command::FireProjectile {
                tower: second_tower,
                target: bug_id,
            },
            &mut events,
        );
        let second_projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => {
                panic!("unexpected events when firing projectile from second tower: {other:?}")
            }
        };
        events.clear();

        let dt = Duration::from_millis(10_000);
        apply(&mut world, Command::Tick { dt }, &mut events);

        assert_eq!(
            events,
            vec![
                Event::TimeAdvanced { dt },
                Event::BugDamaged {
                    bug: bug_id,
                    remaining: Health::ZERO,
                },
                Event::GoldChanged {
                    amount: Gold::new(81),
                },
                Event::BugDied { bug: bug_id },
                Event::ProjectileHit {
                    projectile: first_projectile,
                    target: bug_id,
                    damage: TowerKind::Basic.projectile_damage(),
                },
                Event::ProjectileExpired {
                    projectile: second_projectile,
                },
            ],
        );
        assert!(world.projectiles.is_empty());
        assert!(world.bugs.is_empty());
        assert!(world.occupancy.can_enter(target_cell));
    }

    #[test]
    fn placing_tower_requires_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();
        let origin = CellCoord::new(1, 1);

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::InvalidMode,
            }]
        );
        assert!(world.towers.is_empty());
        assert!(!world.tower_occupancy.contains(origin));
    }

    #[test]
    fn placing_tower_rejects_misaligned_origin() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: DEFAULT_GRID_COLUMNS,
                rows: DEFAULT_GRID_ROWS,
                tile_length: DEFAULT_TILE_LENGTH,
                cells_per_tile: 4,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(
            SIDE_BORDER_CELL_LAYERS.saturating_add(1),
            TOP_BORDER_CELL_LAYERS,
        );
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::Misaligned,
            }]
        );
        assert!(world.towers.is_empty());
    }

    #[test]
    fn placing_tower_rejects_out_of_bounds_origin() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let (columns, _) = world.tower_occupancy.dimensions();
        assert!(columns > 0);
        let origin = CellCoord::new(columns.saturating_sub(1), 0);

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::OutOfBounds,
            }]
        );
        assert!(world.towers.is_empty());
    }

    #[test]
    fn placing_tower_rejects_when_region_is_occupied() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![
                Event::GoldChanged {
                    amount: Gold::new(90),
                },
                Event::TowerPlaced {
                    tower: TowerId::new(0),
                    kind: TowerKind::Basic,
                    region: CellRect::from_origin_and_size(
                        first_origin,
                        super::footprint_for(TowerKind::Basic),
                    ),
                },
            ]
        );
        events.clear();

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin: first_origin,
                reason: PlacementError::Occupied,
            }]
        );
        events.clear();

        let second_origin = CellCoord::new(6, 6);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![
                Event::GoldChanged {
                    amount: Gold::new(80),
                },
                Event::TowerPlaced {
                    tower: TowerId::new(1),
                    kind: TowerKind::Basic,
                    region: CellRect::from_origin_and_size(
                        second_origin,
                        super::footprint_for(TowerKind::Basic),
                    ),
                },
            ]
        );
    }

    #[test]
    fn placing_tower_rejects_when_exit_path_is_blocked() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let cells_per_tile = world.cells_per_tile;
        let exit_columns = exit_columns_for_tile_grid(world.tile_grid.columns(), cells_per_tile);
        assert!(
            !exit_columns.is_empty(),
            "configured grid should expose at least one exit column"
        );
        let exit_column = exit_columns[0];
        assert!(
            exit_column >= 1,
            "exit column should leave room for tower footprint towards the west"
        );

        let walkway_row = walkway_row_for_tile_grid(world.tile_grid.rows(), cells_per_tile)
            .expect("configured grid defines walkway row");
        assert!(
            walkway_row >= 3,
            "walkway row should allow a four-cell-tall footprint to sit above the wall"
        );

        let origin = CellCoord::new(exit_column.saturating_sub(1), walkway_row.saturating_sub(3));

        let (columns, rows) = world.tower_occupancy.dimensions();
        let footprint = super::footprint_for(TowerKind::Basic);
        assert!(
            origin.column().saturating_add(footprint.width()) <= columns,
            "footprint must remain within grid columns"
        );
        assert!(
            origin.row().saturating_add(footprint.height()) <= rows,
            "footprint must remain within grid rows"
        );

        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerPlacementRejected {
                kind: TowerKind::Basic,
                origin,
                reason: PlacementError::PathBlocked,
            }]
        );
        assert!(world.towers.is_empty());
        assert!(!world.tower_occupancy.contains(origin));
    }

    #[test]
    fn placing_tower_sets_occupancy_bits() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(3, 4);
        let region = CellRect::from_origin_and_size(origin, super::footprint_for(TowerKind::Basic));
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![
                Event::GoldChanged {
                    amount: Gold::new(90),
                },
                Event::TowerPlaced {
                    tower: TowerId::new(0),
                    kind: TowerKind::Basic,
                    region,
                },
            ]
        );

        for column_offset in 0..region.size().width() {
            for row_offset in 0..region.size().height() {
                let cell =
                    CellCoord::new(origin.column() + column_offset, origin.row() + row_offset);
                assert!(world.tower_occupancy.contains(cell));
            }
        }
    }

    #[test]
    fn removing_tower_requires_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(2, 3);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemovalRejected {
                tower: TowerId::new(0),
                reason: RemovalError::InvalidMode,
            }]
        );
        assert!(world.towers.get(TowerId::new(0)).is_some());
    }

    #[test]
    fn removing_missing_tower_reports_error() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(42),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemovalRejected {
                tower: TowerId::new(42),
                reason: RemovalError::MissingTower,
            }]
        );
    }

    #[test]
    fn removing_tower_clears_state_and_occupancy() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let origin = CellCoord::new(4, 2);
        let region = CellRect::from_origin_and_size(origin, super::footprint_for(TowerKind::Basic));
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::TowerRemoved {
                tower: TowerId::new(0),
                region,
            }]
        );
        assert!(world.towers.is_empty());

        for column_offset in 0..region.size().width() {
            for row_offset in 0..region.size().height() {
                let cell =
                    CellCoord::new(origin.column() + column_offset, origin.row() + row_offset);
                assert!(!world.tower_occupancy.contains(cell));
            }
        }
    }

    #[test]
    fn tower_query_reports_snapshots_in_identifier_order() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(6, 4);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );
        events.clear();

        let second_origin = CellCoord::new(2, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        let footprint = super::footprint_for(TowerKind::Basic);
        let first_region = CellRect::from_origin_and_size(first_origin, footprint);
        let second_region = CellRect::from_origin_and_size(second_origin, footprint);

        let snapshots = query::towers(&world).into_vec();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].id, TowerId::new(0));
        assert_eq!(snapshots[0].kind, TowerKind::Basic);
        assert_eq!(snapshots[0].region, first_region);
        assert_eq!(snapshots[1].id, TowerId::new(1));
        assert_eq!(snapshots[1].kind, TowerKind::Basic);
        assert_eq!(snapshots[1].region, second_region);
    }

    #[test]
    fn tower_at_reports_identifier_for_cells_inside_footprints() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let first_origin = CellCoord::new(4, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: first_origin,
            },
            &mut events,
        );
        events.clear();

        let second_origin = CellCoord::new(8, 2);
        apply(
            &mut world,
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: second_origin,
            },
            &mut events,
        );

        let footprint = super::footprint_for(TowerKind::Basic);

        for column_offset in 0..footprint.width() {
            for row_offset in 0..footprint.height() {
                let first_cell = CellCoord::new(
                    first_origin.column() + column_offset,
                    first_origin.row() + row_offset,
                );
                assert_eq!(query::tower_at(&world, first_cell), Some(TowerId::new(0)));

                let second_cell = CellCoord::new(
                    second_origin.column() + column_offset,
                    second_origin.row() + row_offset,
                );
                assert_eq!(query::tower_at(&world, second_cell), Some(TowerId::new(1)));
            }
        }

        let outside_above =
            CellCoord::new(first_origin.column(), first_origin.row().saturating_sub(1));
        assert_eq!(query::tower_at(&world, outside_above), None);

        let outside_between = CellCoord::new(
            second_origin.column().saturating_sub(1),
            first_origin.row().saturating_add(footprint.height()),
        );
        assert_eq!(query::tower_at(&world, outside_between), None);

        assert_eq!(query::tower_at(&world, CellCoord::new(0, 0)), None);
    }

    #[test]
    fn entering_builder_mode_clears_bugs_and_occupancy() {
        let mut world = World::new();
        let mut events = Vec::new();

        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x2f, 0x95, 0x32),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );
        assert!(!query::bug_view(&world).into_vec().is_empty());
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }]
        );
        assert_eq!(query::play_mode(&world), PlayMode::Builder);
        assert!(query::bug_view(&world).into_vec().is_empty());
        assert!(query::occupancy_view(&world)
            .iter()
            .all(|slot| slot.is_none()));
    }

    #[test]
    fn returning_to_attack_mode_preserves_empty_maze() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Attack,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Attack,
            }]
        );
        assert_eq!(query::play_mode(&world), PlayMode::Attack);
        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn setting_same_play_mode_is_idempotent() {
        let mut world = World::new();
        let mut events = Vec::new();

        ensure_attack_mode(&mut world, &mut events);

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }]
        );

        events.clear();
        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn configure_tile_grid_respects_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
        assert_eq!(query::play_mode(&world), PlayMode::Builder);
        assert!(query::bug_view(&world).into_vec().is_empty());
        assert!(query::occupancy_view(&world)
            .iter()
            .all(|slot| slot.is_none()));
    }

    #[test]
    fn tick_is_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        let tick_before = world.tick_index;
        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(500),
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert_eq!(world.tick_index, tick_before);
        assert!(world.reservations.claims.is_empty());
    }

    #[test]
    fn step_bug_is_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::StepBug {
                bug_id: BugId::new(0),
                direction: Direction::North,
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert!(world.reservations.claims.is_empty());
    }

    #[test]
    fn bug_spawner_creates_bug_when_cell_free() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(1),
                rows: TileCoord::new(1),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        events.clear();
        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x12, 0x34, 0x56),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        assert_eq!(
            events,
            vec![Event::BugSpawned {
                bug_id: BugId::new(0),
                cell: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x12, 0x34, 0x56),
                health: Health::new(3),
            }]
        );

        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.id, BugId::new(0));
        assert_eq!(snapshot.cell, CellCoord::new(0, 0));
        assert_eq!(snapshot.color, BugColor::from_rgb(0x12, 0x34, 0x56));
        assert_eq!(snapshot.health, Health::new(3));
        assert_eq!(
            query::occupancy_view(&world).occupant(CellCoord::new(0, 0)),
            Some(BugId::new(0))
        );
    }

    #[test]
    fn slow_cadence_bug_requires_multiple_ticks() {
        let mut world = World::new();
        let mut events = Vec::new();
        let slow_step_ms = 600;

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x12, 0x34, 0x56),
                health: Health::new(5),
                step_ms: slow_step_ms,
            },
            &mut events,
        );

        let bug_id = events
            .iter()
            .find_map(|event| {
                if let Event::BugSpawned { bug_id, .. } = event {
                    Some(*bug_id)
                } else {
                    None
                }
            })
            .expect("spawn should emit bug identifier");
        events.clear();

        let index = world
            .bug_index(bug_id)
            .expect("bug should be tracked after spawning");
        world.bugs[index].accum_ms = 0;

        apply(
            &mut world,
            Command::StepBug {
                bug_id,
                direction: Direction::South,
            },
            &mut events,
        );
        assert!(events
            .iter()
            .all(|event| !matches!(event, Event::BugAdvanced { .. })));
        events.clear();

        for _ in 0..2 {
            apply(
                &mut world,
                Command::Tick {
                    dt: Duration::from_millis(200),
                },
                &mut events,
            );
            assert!(events
                .iter()
                .any(|event| matches!(event, Event::TimeAdvanced { .. })));
            events.clear();

            apply(
                &mut world,
                Command::StepBug {
                    bug_id,
                    direction: Direction::South,
                },
                &mut events,
            );
            assert!(events
                .iter()
                .all(|event| !matches!(event, Event::BugAdvanced { .. })));
            events.clear();
        }

        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(200),
            },
            &mut events,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. })));
        events.clear();

        apply(
            &mut world,
            Command::StepBug {
                bug_id,
                direction: Direction::South,
            },
            &mut events,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::BugAdvanced { .. })));

        let updated_index = world
            .bug_index(bug_id)
            .expect("bug should remain tracked after stepping");
        assert_eq!(world.bugs[updated_index].accum_ms, 0);
    }

    #[test]
    fn large_tick_clamps_accumulator_to_step() {
        let mut world = World::new();
        let mut events = Vec::new();
        let step_ms = 150;

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x01, 0x23, 0x45),
                health: Health::new(4),
                step_ms,
            },
            &mut events,
        );

        let bug_id = events
            .iter()
            .find_map(|event| {
                if let Event::BugSpawned { bug_id, .. } = event {
                    Some(*bug_id)
                } else {
                    None
                }
            })
            .expect("spawn should emit bug identifier");
        events.clear();

        let index = world
            .bug_index(bug_id)
            .expect("bug should be tracked after spawning");
        world.bugs[index].accum_ms = 0;

        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(5_000),
            },
            &mut events,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. })));

        let updated_index = world
            .bug_index(bug_id)
            .expect("bug should remain tracked after tick");
        assert_eq!(world.bugs[updated_index].accum_ms, step_ms);
    }

    #[test]
    fn step_bug_carries_remainder_within_single_tick() {
        let mut world = World::new();
        let mut events = Vec::new();
        let step_ms = 120;

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0xde, 0xad, 0xbe),
                health: Health::new(7),
                step_ms,
            },
            &mut events,
        );

        let bug_id = events
            .iter()
            .find_map(|event| {
                if let Event::BugSpawned { bug_id, .. } = event {
                    Some(*bug_id)
                } else {
                    None
                }
            })
            .expect("spawn should emit bug identifier");
        events.clear();

        let index = world
            .bug_index(bug_id)
            .expect("bug should be tracked after spawning");
        world.bugs[index].accum_ms = step_ms.saturating_mul(2);

        apply(
            &mut world,
            Command::StepBug {
                bug_id,
                direction: Direction::South,
            },
            &mut events,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::BugAdvanced { .. })));
        events.clear();

        let mid_index = world
            .bug_index(bug_id)
            .expect("bug should remain tracked after first step");
        assert_eq!(world.bugs[mid_index].accum_ms, step_ms);

        apply(
            &mut world,
            Command::StepBug {
                bug_id,
                direction: Direction::South,
            },
            &mut events,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::BugAdvanced { .. })));

        let final_index = world
            .bug_index(bug_id)
            .expect("bug should remain tracked after second step");
        assert_eq!(world.bugs[final_index].accum_ms, 0);
    }

    #[test]
    fn bug_spawner_requires_free_cell() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(1),
                rows: TileCoord::new(1),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        events.clear();
        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0xaa, 0xbb, 0xcc),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        assert_eq!(events.len(), 1);

        events.clear();
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x10, 0x20, 0x30),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        assert!(events.is_empty());
        let snapshots = query::bug_view(&world).into_vec();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].color, BugColor::from_rgb(0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn bug_spawner_ignored_without_registration() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(3),
                tile_length: 25.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        world.clear_bugs();

        events.clear();
        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(1, 1),
                color: BugColor::from_rgb(0xaa, 0x00, 0xff),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        assert!(events.is_empty());
        assert!(query::bug_view(&world).into_vec().is_empty());
    }

    #[test]
    fn bug_spawner_ignored_in_builder_mode() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            &mut events,
        );

        events.clear();
        let step_ms = world_step_ms(&world);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0, 0, 0),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        assert!(events.is_empty());
    }

    #[test]
    fn bug_spawners_cover_outer_rim_in_default_world() {
        let world = World::new();
        let (columns, rows) = world.occupancy.dimensions();
        let expected = expected_outer_rim(columns, rows);

        assert_eq!(world.bug_spawners.cells(), &expected);
    }

    #[test]
    fn bug_spawners_rebuilt_after_configuring_grid() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(2),
                rows: TileCoord::new(3),
                tile_length: 50.0,
                cells_per_tile: 2,
            },
            &mut events,
        );

        let (columns, rows) = world.occupancy.dimensions();
        let expected_columns = total_cell_columns(TileCoord::new(2), 2);
        let expected_rows = total_cell_rows(TileCoord::new(3), 2);
        assert_eq!((columns, rows), (expected_columns, expected_rows));
        let expected = expected_outer_rim(columns, rows);

        assert_eq!(world.bug_spawners.cells(), &expected);
    }

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
                cells_per_tile: 1,
            },
            &mut events,
        );

        let tile_grid = query::tile_grid(&world);

        assert_eq!(tile_grid.columns(), expected_columns);
        assert_eq!(tile_grid.rows(), expected_rows);
        assert_eq!(tile_grid.tile_length(), expected_tile_length);
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::PressureConfigChanged { .. })),);
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
    }

    #[test]
    fn bugs_are_generated_within_configured_grid() {
        let mut world = World::new();
        let mut events = Vec::new();
        let columns = TileCoord::new(8);
        let rows = TileCoord::new(6);
        let cells_per_tile = 2;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns,
                rows,
                tile_length: 32.0,
                cells_per_tile,
            },
            &mut events,
        );

        let interior_start_column = 0;
        let interior_end_column =
            interior_start_column + columns.get().saturating_mul(cells_per_tile);
        let interior_start_row = 0;
        let interior_end_row = interior_start_row + rows.get().saturating_mul(cells_per_tile);

        for bug in query::bug_view(&world).iter() {
            assert!(bug.cell.column() >= interior_start_column);
            assert!(bug.cell.column() < interior_end_column);
            assert!(bug.cell.row() >= interior_start_row);
            assert!(bug.cell.row() < interior_end_row);
        }
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
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
                cells_per_tile: 1,
            },
            &mut events,
        );

        let bugs = query::bug_view(&world).into_vec();
        assert!(bugs.is_empty());
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
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
                cells_per_tile: 3,
            },
            &mut first_events,
        );

        apply(
            &mut second_world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(9),
                tile_length: 50.0,
                cells_per_tile: 3,
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
    fn bug_emits_exit_event_after_advancing_to_exit_cell() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(3),
                rows: TileCoord::new(3),
                tile_length: 1.0,
                cells_per_tile: 1,
            },
            &mut events,
        );
        events.clear();

        let exit_cell = query::target_cells(&world)
            .into_iter()
            .next()
            .expect("expected at least one exit cell");
        let spawn_cell = query::bug_spawners(&world)
            .into_iter()
            .find(|cell| cell.column() == exit_cell.column())
            .expect("expected aligned spawner");
        let step_ms = world_step_ms(&world);

        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: spawn_cell,
                color: BugColor::from_rgb(0x2f, 0x95, 0x32),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );

        let bug_id = events
            .iter()
            .find_map(|event| {
                if let Event::BugSpawned { bug_id, .. } = event {
                    Some(*bug_id)
                } else {
                    None
                }
            })
            .expect("spawn should emit bug identifier");
        events.clear();

        let mut snapshot = query::bug_view(&world)
            .iter()
            .find(|bug| bug.id == bug_id)
            .map(|bug| bug.cell)
            .expect("bug should exist after spawning");

        while snapshot.column() < exit_cell.column() {
            let step_events = drive_step(&mut world, bug_id, Direction::East);
            assert!(step_events
                .iter()
                .any(|event| matches!(event, Event::BugAdvanced { .. })));
            snapshot = query::bug_view(&world)
                .iter()
                .find(|bug| bug.id == bug_id)
                .map(|bug| bug.cell)
                .expect("bug should remain before exit step");
        }

        while snapshot.column() > exit_cell.column() {
            let step_events = drive_step(&mut world, bug_id, Direction::West);
            assert!(step_events
                .iter()
                .any(|event| matches!(event, Event::BugAdvanced { .. })));
            snapshot = query::bug_view(&world)
                .iter()
                .find(|bug| bug.id == bug_id)
                .map(|bug| bug.cell)
                .expect("bug should remain before exit step");
        }

        let visible_wall_row = visible_wall_row_for_tile_grid(TileCoord::new(3), 1)
            .expect("configured grid defines wall row");
        let walkway_row = walkway_row_for_tile_grid(TileCoord::new(3), 1)
            .expect("configured grid defines walkway row");

        while snapshot.row() < walkway_row {
            let step_events = drive_step(&mut world, bug_id, Direction::South);
            assert!(step_events
                .iter()
                .any(|event| matches!(event, Event::BugAdvanced { .. })));
            snapshot = query::bug_view(&world)
                .iter()
                .find(|bug| bug.id == bug_id)
                .map(|bug| bug.cell)
                .expect("bug should remain before exit step");
        }

        assert_eq!(
            snapshot.row(),
            walkway_row,
            "bug should pause on walkway row"
        );

        let wall_step_events = drive_step(&mut world, bug_id, Direction::South);
        let wall_advanced_cell = wall_step_events
            .iter()
            .find_map(|event| {
                if let Event::BugAdvanced {
                    bug_id: event_id,
                    to,
                    ..
                } = event
                {
                    if *event_id == bug_id {
                        return Some(*to);
                    }
                }
                None
            })
            .expect("bug should advance through wall gap");
        assert_eq!(
            wall_advanced_cell,
            CellCoord::new(exit_cell.column(), visible_wall_row),
            "bug must enter the visible wall row first",
        );
        assert!(
            wall_step_events
                .iter()
                .all(|event| !matches!(event, Event::BugExited { .. })),
            "bug should not exit from the wall row",
        );

        snapshot = query::bug_view(&world)
            .iter()
            .find(|bug| bug.id == bug_id)
            .map(|bug| bug.cell)
            .expect("bug should remain before exit step");
        assert_eq!(snapshot.row(), visible_wall_row);

        let final_events = drive_step(&mut world, bug_id, Direction::South);
        let advanced_index = final_events
            .iter()
            .position(|event| matches!(event, Event::BugAdvanced { .. }))
            .expect("expected bug advancement event");
        let exit_index = final_events
            .iter()
            .position(|event| matches!(event, Event::BugExited { .. }))
            .expect("expected bug exit event");
        let builder_index = final_events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::PlayModeChanged {
                        mode: PlayMode::Builder,
                    }
                )
            })
            .expect("expected builder play mode event");
        let round_lost_index = final_events
            .iter()
            .position(|event| matches!(event, Event::RoundLost { .. }))
            .expect("expected round lost event");
        assert!(
            advanced_index < exit_index,
            "bug must advance before exiting"
        );
        assert!(exit_index < builder_index, "play mode change follows exit");
        assert!(
            builder_index < round_lost_index,
            "round loss announced last"
        );

        let mut advanced_cell: Option<CellCoord> = None;
        let mut exited_cell: Option<CellCoord> = None;
        let mut lost_bug = None;
        for event in &final_events {
            match event {
                Event::BugAdvanced {
                    bug_id: event_id,
                    to,
                    ..
                } => {
                    assert_eq!(*event_id, bug_id, "unexpected bug advanced");
                    advanced_cell = Some(*to);
                }
                Event::BugExited {
                    bug_id: event_id,
                    cell,
                } => {
                    assert_eq!(*event_id, bug_id, "unexpected bug exit");
                    exited_cell = Some(*cell);
                }
                Event::RoundLost { bug } => {
                    assert!(lost_bug.is_none(), "duplicate round loss event");
                    lost_bug = Some(*bug);
                }
                _ => {}
            }
        }

        assert_eq!(advanced_cell, Some(exit_cell));
        assert_eq!(exited_cell, Some(exit_cell));
        assert_eq!(lost_bug, Some(bug_id));
        assert!(query::bug_view(&world).iter().all(|bug| bug.id != bug_id));
        let occupancy = query::occupancy_view(&world);
        assert!(occupancy.is_free(exit_cell));
        assert_eq!(query::play_mode(&world), PlayMode::Builder);
    }

    #[test]
    fn target_aligns_with_center_for_odd_columns() {
        let mut world = World::new();
        let mut events = Vec::new();
        let cells_per_tile = 3;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(9),
                rows: TileCoord::new(7),
                tile_length: 64.0,
                cells_per_tile,
            },
            &mut events,
        );

        let target_cells = query::target(&world).cells();

        assert_eq!(
            target_cells.len(),
            usize::try_from(cells_per_tile).expect("cells_per_tile fits in usize")
        );
        let expected_row = exit_row_for_tile_grid(TileCoord::new(7), cells_per_tile);
        let expected_start =
            SIDE_BORDER_CELL_LAYERS.saturating_add(4_u32.saturating_mul(cells_per_tile));
        let expected_columns: Vec<u32> = (0..cells_per_tile)
            .map(|offset| expected_start + offset)
            .collect();
        let actual_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column()).collect();
        assert_eq!(actual_columns, expected_columns);
        assert!(target_cells.iter().all(|cell| cell.row() == expected_row));
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
    }

    #[test]
    fn target_spans_single_tile_for_even_columns() {
        let mut world = World::new();
        let mut events = Vec::new();
        let cells_per_tile = 2;

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(12),
                rows: TileCoord::new(6),
                tile_length: 64.0,
                cells_per_tile,
            },
            &mut events,
        );

        let target_cells = query::target(&world).cells();

        assert_eq!(
            target_cells.len(),
            usize::try_from(cells_per_tile).expect("cells_per_tile fits in usize")
        );
        let expected_row = exit_row_for_tile_grid(TileCoord::new(6), cells_per_tile);
        let expected_start =
            SIDE_BORDER_CELL_LAYERS.saturating_add(5_u32.saturating_mul(cells_per_tile));
        let expected_columns: Vec<u32> = (0..cells_per_tile)
            .map(|offset| expected_start + offset)
            .collect();
        let actual_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column()).collect();
        assert_eq!(actual_columns, expected_columns);
        assert!(target_cells.iter().all(|cell| cell.row() == expected_row));
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
    }

    #[test]
    fn target_absent_when_grid_missing() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(0),
                rows: TileCoord::new(0),
                tile_length: 32.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        assert!(query::target(&world).cells().is_empty());
        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());
    }

    #[test]
    fn goal_for_returns_nearest_target_cell() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureTileGrid {
                columns: TileCoord::new(5),
                rows: TileCoord::new(4),
                tile_length: 1.0,
                cells_per_tile: 1,
            },
            &mut events,
        );

        strip_pending_wave_events(&mut events);
        assert!(events.is_empty());

        let goal = query::goal_for(&world, CellCoord::new(0, 0));
        let expected_columns = exit_columns_for_tile_grid(TileCoord::new(5), 1);
        let expected_column = *expected_columns
            .first()
            .expect("expected at least one target column");
        let expected = CellCoord::new(
            expected_column,
            exit_row_for_tile_grid(TileCoord::new(4), 1),
        );
        assert_eq!(goal, Some(Goal::at(expected)));
    }

    #[test]
    fn select_goal_prefers_closest_cell() {
        let origin = CellCoord::new(3, 2);
        let candidates = [
            CellCoord::new(0, 5),
            CellCoord::new(3, 5),
            CellCoord::new(4, 4),
        ];

        let goal = query::select_goal(origin, &candidates);
        assert_eq!(goal, Some(Goal::at(CellCoord::new(3, 5))));
    }

    #[test]
    fn configure_bug_step_adjusts_quantum() {
        let mut world = World::new();
        let mut events = Vec::new();

        apply(
            &mut world,
            Command::ConfigureBugStep {
                step_duration: Duration::from_millis(125),
            },
            &mut events,
        );

        assert!(events.is_empty());

        let step_ms = world_step_ms(&world);
        ensure_attack_mode(&mut world, &mut events);
        apply(
            &mut world,
            Command::SpawnBug {
                spawner: CellCoord::new(0, 0),
                color: BugColor::from_rgb(0x2f, 0x95, 0x32),
                health: Health::new(3),
                step_ms,
            },
            &mut events,
        );
        events.clear();

        apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(125),
            },
            &mut events,
        );

        assert!(events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. })));

        let bug_view = query::bug_view(&world);
        assert!(bug_view.iter().any(|bug| bug.ready_for_step));
    }

    #[test]
    fn projectile_replay_is_deterministic() {
        let script = scripted_combat_commands();
        let first = replay_combat_script(script.clone());
        let second = replay_combat_script(script);

        assert_eq!(first, second, "combat replay diverged between runs");

        let fingerprint = first.fingerprint();
        let expected = 0xec52_f446_c2fb_ff32;
        assert_eq!(
            fingerprint, expected,
            "combat replay fingerprint mismatch: {fingerprint:#x}"
        );
    }

    fn replay_combat_script(commands: Vec<Command>) -> CombatReplayOutcome {
        let mut world = World::new();
        let mut log = Vec::new();

        for command in commands {
            let mut events = Vec::new();
            apply(&mut world, command, &mut events);
            log.extend(events.into_iter().map(CombatEventRecord::from));
        }

        let towers = query::towers(&world)
            .into_vec()
            .into_iter()
            .map(TowerRecord::from)
            .collect();

        let cooldowns = query::tower_cooldowns(&world)
            .into_vec()
            .into_iter()
            .map(TowerCooldownRecord::from)
            .collect();

        let bugs = query::bug_view(&world)
            .into_vec()
            .into_iter()
            .map(BugRecord::from)
            .collect();

        let projectiles = query::projectiles(&world)
            .map(ProjectileRecord::from)
            .collect();

        let navigation = query::navigation_field(&world);
        let navigation_fingerprint = navigation_fingerprint(&navigation);

        CombatReplayOutcome {
            towers,
            cooldowns,
            bugs,
            projectiles,
            next_bug_id: world.next_bug_id,
            next_projectile_id: world.next_projectile_id,
            events: log,
            navigation_fingerprint,
        }
    }

    fn scripted_combat_commands() -> Vec<Command> {
        let mut world = World::new();
        let mut commands = Vec::new();
        let mut events = Vec::new();

        ensure_attack_mode(&mut world, &mut events);
        let enter_builder = Command::SetPlayMode {
            mode: PlayMode::Builder,
        };
        commands.push(enter_builder.clone());
        apply(&mut world, enter_builder, &mut events);
        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Builder,
            }]
        );
        events.clear();

        let near_origin = CellCoord::new(2, 2);
        let place_near = Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: near_origin,
        };
        commands.push(place_near.clone());
        apply(&mut world, place_near, &mut events);
        let near_tower = match events.as_slice() {
            [Event::GoldChanged { amount }, Event::TowerPlaced { tower, .. }]
                if *amount == Gold::new(90) =>
            {
                *tower
            }
            other => panic!("unexpected events when placing near tower: {other:?}"),
        };
        events.clear();

        let far_origin = CellCoord::new(6, 6);
        let place_far = Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: far_origin,
        };
        commands.push(place_far.clone());
        apply(&mut world, place_far, &mut events);
        let far_tower = match events.as_slice() {
            [Event::GoldChanged { amount }, Event::TowerPlaced { tower, .. }]
                if *amount == Gold::new(80) =>
            {
                *tower
            }
            other => panic!("unexpected events when placing far tower: {other:?}"),
        };
        events.clear();

        let enter_attack = Command::SetPlayMode {
            mode: PlayMode::Attack,
        };
        commands.push(enter_attack.clone());
        apply(&mut world, enter_attack, &mut events);
        assert_eq!(
            events,
            vec![Event::PlayModeChanged {
                mode: PlayMode::Attack,
            }]
        );
        events.clear();

        let spawner = query::bug_spawners(&world)
            .into_iter()
            .next()
            .expect("expected at least one spawner");
        let step_ms = world_step_ms(&world);
        let spawn_bug = Command::SpawnBug {
            spawner,
            color: BugColor::from_rgb(0xaa, 0x44, 0x22),
            health: Health::new(1),
            step_ms,
        };
        commands.push(spawn_bug.clone());
        apply(&mut world, spawn_bug, &mut events);
        let bug_id = match events.as_slice() {
            [Event::BugSpawned { bug_id, .. }] => *bug_id,
            other => panic!("unexpected events when spawning bug: {other:?}"),
        };
        events.clear();

        let fire_near = Command::FireProjectile {
            tower: near_tower,
            target: bug_id,
        };
        commands.push(fire_near.clone());
        apply(&mut world, fire_near, &mut events);
        let near_projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => panic!("unexpected events when firing projectile from near tower: {other:?}"),
        };
        events.clear();

        let fire_far = Command::FireProjectile {
            tower: far_tower,
            target: bug_id,
        };
        commands.push(fire_far.clone());
        apply(&mut world, fire_far, &mut events);
        let far_projectile = match events.as_slice() {
            [Event::ProjectileFired { projectile, .. }] => *projectile,
            other => panic!("unexpected events when firing projectile from far tower: {other:?}"),
        };
        events.clear();

        let mut projectile_states: Vec<ProjectileState> =
            world.projectiles.values().cloned().collect();
        assert_eq!(
            projectile_states.len(),
            2,
            "expected two active projectiles"
        );
        projectile_states.sort_by_key(|state| state.distance_half);

        let first = &projectile_states[0];
        let second = &projectile_states[1];
        let first_time = travel_time_millis(first);
        let second_time = travel_time_millis(second);
        assert!(
            first_time < second_time,
            "expected first projectile to arrive before second"
        );
        let first_id = first.id;
        let second_id = second.id;
        let first_damage = first.damage;

        let first_dt = Duration::from_millis(
            u64::try_from(first_time).expect("projectile travel time fits in u64"),
        );
        let resolve_first = Command::Tick { dt: first_dt };
        commands.push(resolve_first.clone());
        apply(&mut world, resolve_first, &mut events);
        match events.as_slice() {
            [Event::TimeAdvanced { dt }, Event::BugDamaged { bug, remaining }, Event::GoldChanged { amount }, Event::BugDied { bug: died_bug }, Event::ProjectileHit {
                projectile,
                target,
                damage,
            }] if *dt == first_dt
                && *bug == bug_id
                && *remaining == Health::ZERO
                && *amount == Gold::new(81)
                && *died_bug == bug_id
                && *projectile == first_id
                && *target == bug_id
                && *damage == first_damage => {}
            other => panic!("unexpected events when resolving first projectile tick: {other:?}"),
        }
        events.clear();

        let remaining_time = second_time - first_time;
        assert!(
            remaining_time > 0,
            "expected second projectile to require extra time"
        );
        let remaining_dt = Duration::from_millis(
            u64::try_from(remaining_time).expect("remaining travel time fits in u64"),
        );
        let resolve_second = Command::Tick { dt: remaining_dt };
        commands.push(resolve_second.clone());
        apply(&mut world, resolve_second, &mut events);
        match events.as_slice() {
            [Event::TimeAdvanced { dt }, Event::ProjectileExpired { projectile }]
                if *dt == remaining_dt && *projectile == second_id => {}
            other => panic!("unexpected events when resolving second projectile tick: {other:?}"),
        }
        events.clear();

        assert_eq!(
            [near_projectile, far_projectile]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            [first_id, second_id]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            "projectile identifiers changed unexpectedly",
        );
        assert!(
            world.projectiles.is_empty(),
            "expected no remaining projectiles"
        );
        assert!(
            world.bugs.is_empty(),
            "expected bug to be removed after death"
        );

        commands
    }

    fn travel_time_millis(projectile: &ProjectileState) -> u128 {
        projectile.travel_time_ms
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct CombatReplayOutcome {
        towers: Vec<TowerRecord>,
        cooldowns: Vec<TowerCooldownRecord>,
        bugs: Vec<BugRecord>,
        projectiles: Vec<ProjectileRecord>,
        next_bug_id: u32,
        next_projectile_id: ProjectileId,
        events: Vec<CombatEventRecord>,
        navigation_fingerprint: u64,
    }

    impl CombatReplayOutcome {
        fn fingerprint(&self) -> u64 {
            let mut hasher = DefaultHasher::new();
            self.hash(&mut hasher);
            hasher.finish()
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct TowerCooldownRecord {
        tower: TowerId,
        kind: TowerKind,
        ready_in_micros: u128,
    }

    impl From<TowerCooldownSnapshot> for TowerCooldownRecord {
        fn from(snapshot: TowerCooldownSnapshot) -> Self {
            Self {
                tower: snapshot.tower,
                kind: snapshot.kind,
                ready_in_micros: snapshot.ready_in.as_micros(),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct BugRecord {
        id: BugId,
        cell: CellCoord,
        color: (u8, u8, u8),
        max_health: Health,
        health: Health,
        step_ms: u32,
        accum_ms: u32,
        ready_for_step: bool,
    }

    impl From<BugSnapshot> for BugRecord {
        fn from(snapshot: BugSnapshot) -> Self {
            Self {
                id: snapshot.id,
                cell: snapshot.cell,
                color: (
                    snapshot.color.red(),
                    snapshot.color.green(),
                    snapshot.color.blue(),
                ),
                max_health: snapshot.max_health,
                health: snapshot.health,
                step_ms: snapshot.step_ms,
                accum_ms: snapshot.accum_ms,
                ready_for_step: snapshot.ready_for_step,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ProjectileRecord {
        projectile: ProjectileId,
        tower: TowerId,
        target: BugId,
        origin_half: CellPointHalf,
        dest_half: CellPointHalf,
        distance_half: u128,
        travelled_half: u128,
    }

    impl From<ProjectileSnapshot> for ProjectileRecord {
        fn from(snapshot: ProjectileSnapshot) -> Self {
            Self {
                projectile: snapshot.projectile,
                tower: snapshot.tower,
                target: snapshot.target,
                origin_half: snapshot.origin_half,
                dest_half: snapshot.dest_half,
                distance_half: snapshot.distance_half,
                travelled_half: snapshot.travelled_half,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    enum CombatEventRecord {
        PlayModeChanged {
            mode: PlayMode,
        },
        TowerPlaced {
            tower: TowerId,
            kind: TowerKind,
            region: CellRect,
        },
        RoundLost {
            bug: BugId,
        },
        BugSpawned {
            bug: BugId,
            cell: CellCoord,
            color: (u8, u8, u8),
            health: Health,
        },
        ProjectileFired {
            projectile: ProjectileId,
            tower: TowerId,
            target: BugId,
        },
        ProjectileHit {
            projectile: ProjectileId,
            target: BugId,
            damage: Damage,
        },
        ProjectileExpired {
            projectile: ProjectileId,
        },
        ProjectileRejected {
            tower: TowerId,
            target: BugId,
            reason: ProjectileRejection,
        },
        BugDamaged {
            bug: BugId,
            remaining: Health,
        },
        BugDied {
            bug: BugId,
        },
        TimeAdvanced {
            dt_micros: u128,
        },
        GoldChanged {
            amount: Gold,
        },
    }

    impl From<Event> for CombatEventRecord {
        fn from(event: Event) -> Self {
            match event {
                Event::PlayModeChanged { mode } => Self::PlayModeChanged { mode },
                Event::TowerPlaced {
                    tower,
                    kind,
                    region,
                } => Self::TowerPlaced {
                    tower,
                    kind,
                    region,
                },
                Event::RoundLost { bug } => Self::RoundLost { bug },
                Event::BugSpawned {
                    bug_id,
                    cell,
                    color,
                    health,
                } => Self::BugSpawned {
                    bug: bug_id,
                    cell,
                    color: (color.red(), color.green(), color.blue()),
                    health,
                },
                Event::ProjectileFired {
                    projectile,
                    tower,
                    target,
                } => Self::ProjectileFired {
                    projectile,
                    tower,
                    target,
                },
                Event::ProjectileHit {
                    projectile,
                    target,
                    damage,
                } => Self::ProjectileHit {
                    projectile,
                    target,
                    damage,
                },
                Event::ProjectileExpired { projectile } => Self::ProjectileExpired { projectile },
                Event::ProjectileRejected {
                    tower,
                    target,
                    reason,
                } => Self::ProjectileRejected {
                    tower,
                    target,
                    reason,
                },
                Event::BugDamaged { bug, remaining } => Self::BugDamaged { bug, remaining },
                Event::BugDied { bug } => Self::BugDied { bug },
                Event::TimeAdvanced { dt } => Self::TimeAdvanced {
                    dt_micros: dt.as_micros(),
                },
                Event::GoldChanged { amount } => Self::GoldChanged { amount },
                other => panic!("unexpected event emitted during combat replay: {other:?}"),
            }
        }
    }

    #[test]
    fn tower_replay_is_deterministic() {
        let first = replay_tower_script(scripted_tower_commands());
        let second = replay_tower_script(scripted_tower_commands());

        assert_eq!(first, second, "tower replay diverged between runs");

        let fingerprint = first.fingerprint();
        let expected = 0x77da_db50_6ee7_9b88;
        assert_eq!(
            fingerprint, expected,
            "tower replay fingerprint mismatch: {fingerprint:#x}"
        );
    }

    fn replay_tower_script(commands: Vec<Command>) -> ReplayOutcome {
        let mut world = World::new();
        let mut log = Vec::new();

        let mut init_events = Vec::new();
        ensure_attack_mode(&mut world, &mut init_events);
        debug_assert!(init_events.is_empty());

        for command in commands {
            let mut events = Vec::new();
            apply(&mut world, command, &mut events);
            log.extend(
                events
                    .into_iter()
                    .filter(|event| !matches!(event, Event::PressureConfigChanged { .. }))
                    .map(EventRecord::from),
            );
        }

        let towers = query::towers(&world)
            .into_vec()
            .into_iter()
            .map(TowerRecord::from)
            .collect();

        let navigation = query::navigation_field(&world);
        let navigation_fingerprint = navigation_fingerprint(&navigation);

        ReplayOutcome {
            towers,
            events: log,
            navigation_fingerprint,
        }
    }

    fn scripted_tower_commands() -> Vec<Command> {
        vec![
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(1, 1),
            },
            Command::SetPlayMode {
                mode: PlayMode::Builder,
            },
            Command::ConfigureTileGrid {
                columns: TileCoord::new(6),
                rows: TileCoord::new(5),
                tile_length: 1.0,
                cells_per_tile: 4,
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(3, 2),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 20),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 4),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(4, 4),
            },
            Command::RemoveTower {
                tower: TowerId::new(1),
            },
            Command::RemoveTower {
                tower: TowerId::new(0),
            },
            Command::PlaceTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(8, 6),
            },
        ]
    }

    fn drive_step(world: &mut World, bug_id: BugId, direction: Direction) -> Vec<Event> {
        let mut tick_events = Vec::new();
        apply(
            world,
            Command::Tick {
                dt: Duration::from_millis(250),
            },
            &mut tick_events,
        );
        assert!(tick_events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. })));

        let mut step_events = Vec::new();
        apply(
            world,
            Command::StepBug { bug_id, direction },
            &mut step_events,
        );
        step_events
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ReplayOutcome {
        towers: Vec<TowerRecord>,
        events: Vec<EventRecord>,
        navigation_fingerprint: u64,
    }

    impl ReplayOutcome {
        fn fingerprint(&self) -> u64 {
            let mut hasher = DefaultHasher::new();
            self.hash(&mut hasher);
            hasher.finish()
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct TowerRecord {
        id: TowerId,
        kind: TowerKind,
        region: CellRect,
    }

    impl From<maze_defence_core::TowerSnapshot> for TowerRecord {
        fn from(snapshot: maze_defence_core::TowerSnapshot) -> Self {
            Self {
                id: snapshot.id,
                kind: snapshot.kind,
                region: snapshot.region,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    enum EventRecord {
        PlayModeChanged {
            mode: PlayMode,
        },
        TowerPlaced {
            tower: TowerId,
            kind: TowerKind,
            region: CellRect,
        },
        TowerRemoved {
            tower: TowerId,
            region: CellRect,
        },
        TowerPlacementRejected {
            kind: TowerKind,
            origin: CellCoord,
            reason: PlacementError,
        },
        TowerRemovalRejected {
            tower: TowerId,
            reason: RemovalError,
        },
        RoundLost {
            bug: BugId,
        },
        PendingWaveDifficultyChanged {
            pending: PendingWaveDifficulty,
        },
    }

    impl From<Event> for EventRecord {
        fn from(event: Event) -> Self {
            match event {
                Event::PlayModeChanged { mode } => Self::PlayModeChanged { mode },
                Event::TowerPlaced {
                    tower,
                    kind,
                    region,
                } => Self::TowerPlaced {
                    tower,
                    kind,
                    region,
                },
                Event::TowerRemoved { tower, region } => Self::TowerRemoved { tower, region },
                Event::TowerPlacementRejected {
                    kind,
                    origin,
                    reason,
                } => Self::TowerPlacementRejected {
                    kind,
                    origin,
                    reason,
                },
                Event::TowerRemovalRejected { tower, reason } => {
                    Self::TowerRemovalRejected { tower, reason }
                }
                Event::RoundLost { bug } => Self::RoundLost { bug },
                Event::PendingWaveDifficultyChanged { pending } => {
                    Self::PendingWaveDifficultyChanged { pending }
                }
                other => panic!("unexpected event emitted during tower replay: {other:?}"),
            }
        }
    }
}
