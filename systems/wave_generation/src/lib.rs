#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic pressure-based wave generation system.

use std::collections::HashSet;
use std::num::NonZeroU32;

use maze_defence_core::{
    AttackPlan, BurstPlan, CadenceRange, Command, Event, Pressure, PressureConfig, PressureWeight,
    SpawnPatchId, SpawnPatchTableView, SpeciesDefinition, SpeciesId, SpeciesTableView,
    WaveDifficulty, WaveId, WaveSeedContext, PRESSURE_FIXED_POINT_SCALE, RNG_STREAM_DIRICHLET,
    RNG_STREAM_PRESSURE, RNG_STREAM_SPECIES_PREFIX,
};
use sha2::{Digest, Sha256};

const TWO_PI: f64 = std::f64::consts::PI * 2.0;

/// Pure system that generates deterministic [`AttackPlan`] values for waves.
#[derive(Debug, Default)]
pub struct WaveGeneration {
    dirichlet_workspace: Vec<f64>,
}

impl WaveGeneration {
    /// Consumes `GenerateAttackPlan` commands and emits [`Event::AttackPlanReady`].
    #[allow(clippy::too_many_arguments)]
    pub fn handle(
        &mut self,
        commands: &[Command],
        species_table: SpeciesTableView<'_>,
        patch_table: SpawnPatchTableView<'_>,
        pressure_config: &PressureConfig,
        seed_context: WaveSeedContext,
        out_events: &mut Vec<Event>,
    ) {
        if species_table.definitions().is_empty() {
            for command in commands {
                if let Command::GenerateAttackPlan { wave, .. } = command {
                    let plan = AttackPlan::empty(species_table.version());
                    out_events.push(Event::AttackPlanReady { wave: *wave, plan });
                }
            }
            return;
        }

        let mut valid_patches = HashSet::new();
        for descriptor in patch_table.iter() {
            let _ = valid_patches.insert(descriptor.id());
        }

        for command in commands {
            if let Command::GenerateAttackPlan { wave, difficulty } = command {
                let plan = self.generate_plan(
                    *wave,
                    *difficulty,
                    &species_table,
                    &valid_patches,
                    pressure_config,
                    seed_context,
                );
                out_events.push(Event::AttackPlanReady { wave: *wave, plan });
            }
        }
    }

    fn generate_plan(
        &mut self,
        wave: WaveId,
        difficulty: WaveDifficulty,
        species_table: &SpeciesTableView<'_>,
        valid_patches: &HashSet<SpawnPatchId>,
        pressure_config: &PressureConfig,
        seed_context: WaveSeedContext,
    ) -> AttackPlan {
        let pressure_curve = pressure_config.curve();
        let mean = f64::from(pressure_curve.mean().get());
        let deviation = f64::from(pressure_curve.deviation().get());
        let effective_tier = effective_tier(seed_context.difficulty_tier(), difficulty);
        let pressure_scalar = effective_tier.saturating_add(1);

        let base_seed = derive_base_seed(seed_context.global_seed(), wave, effective_tier);
        let mut pressure_rng = SplitMix64::new(derive_labeled_seed(base_seed, RNG_STREAM_PRESSURE));
        let mut dirichlet_rng =
            SplitMix64::new(derive_labeled_seed(base_seed, RNG_STREAM_DIRICHLET));

        let sampled_pressure = sample_pressure(mean, deviation, &mut pressure_rng);
        let pressure_value = Pressure::new(sampled_pressure).saturating_mul(pressure_scalar);
        if pressure_value.is_zero() {
            return AttackPlan::empty(species_table.version());
        }

        let mut ordered: Vec<&SpeciesDefinition> = species_table.iter().collect();
        ordered.sort_by_key(|definition| definition.id());
        self.prepare_dirichlet_workspace(ordered.len());
        let proportions =
            sample_dirichlet(&mut dirichlet_rng, &ordered, &mut self.dirichlet_workspace);

        let pressure_budget = pressure_value.get();
        let mut bursts = Vec::new();

        for (definition, proportion) in ordered.into_iter().zip(proportions) {
            if !valid_patches.contains(&definition.patch()) {
                continue;
            }

            let count = resolve_species_count(
                pressure_budget,
                proportion,
                definition.weight(),
                definition.min_burst_spawn(),
                definition.max_population(),
            );

            if count == 0 {
                continue;
            }

            let mut species_rng = SplitMix64::new(derive_species_seed(base_seed, definition.id()));
            let cadence = sample_cadence(definition.cadence_range(), &mut species_rng);
            let start_offsets = sample_burst_starts(
                count,
                definition.gap_range(),
                pressure_config.burst_scheduling(),
                &mut species_rng,
            );

            for (burst_index, burst_size) in start_offsets.burst_sizes.iter().enumerate() {
                let count_nz = NonZeroU32::new(*burst_size).expect("burst size must be non-zero");
                let cadence_nz = NonZeroU32::new(cadence).expect("cadence must be non-zero");
                let start_ms = start_offsets.starts[burst_index];
                bursts.push(BurstPlan::new(
                    definition.id(),
                    definition.patch(),
                    count_nz,
                    cadence_nz,
                    start_ms,
                ));
            }
        }

        AttackPlan::new(pressure_value, species_table.version(), bursts)
    }

    fn prepare_dirichlet_workspace(&mut self, capacity: usize) {
        if self.dirichlet_workspace.len() < capacity {
            self.dirichlet_workspace.resize(capacity, 0.0);
        }
    }
}

fn effective_tier(base_tier: u32, difficulty: WaveDifficulty) -> u32 {
    match difficulty {
        WaveDifficulty::Normal => base_tier,
        WaveDifficulty::Hard => base_tier.saturating_add(1),
    }
}

fn sample_pressure(mean: f64, deviation: f64, rng: &mut SplitMix64) -> u32 {
    let sample = if deviation == 0.0 {
        mean
    } else {
        let normal = sample_standard_normal(rng);
        mean + deviation * normal
    };

    if sample <= 0.0 {
        return 0;
    }

    let rounded = sample.round();
    let clamped = rounded.max(0.0).min(f64::from(u32::MAX));
    clamped as u32
}

fn sample_standard_normal(rng: &mut SplitMix64) -> f64 {
    let u1 = rng.next_unit_open();
    let u2 = rng.next_unit();
    let radius = (-2.0 * u1.ln()).sqrt();
    let theta = TWO_PI * u2;
    radius * theta.cos()
}

fn sample_dirichlet(
    rng: &mut SplitMix64,
    species: &[&SpeciesDefinition],
    workspace: &mut [f64],
) -> Vec<f64> {
    let mut total = 0.0;
    for (index, definition) in species.iter().enumerate() {
        let shape = definition.dirichlet_weight().get().get();
        let sample = sample_gamma_integer(rng, shape);
        workspace[index] = sample;
        total += sample;
    }

    if total <= f64::EPSILON {
        let uniform = 1.0 / species.len() as f64;
        return vec![uniform; species.len()];
    }

    workspace.iter().map(|value| value / total).collect()
}

fn sample_gamma_integer(rng: &mut SplitMix64, shape: u32) -> f64 {
    if shape == 0 {
        return 0.0;
    }

    let mut sum = 0.0;
    for _ in 0..shape {
        let u = rng.next_unit_open();
        sum -= u.ln();
    }
    sum
}

fn resolve_species_count(
    pressure_budget: u32,
    proportion: f64,
    weight: PressureWeight,
    min_burst_spawn: u32,
    max_population: NonZeroU32,
) -> u32 {
    if pressure_budget == 0 {
        return 0;
    }

    let target = (f64::from(pressure_budget) * proportion).round();
    if target <= 0.0 {
        return 0;
    }

    let numerator = (target as u128).saturating_mul(u128::from(PRESSURE_FIXED_POINT_SCALE));
    let denominator = u128::from(weight.get().get());
    let mut count = (numerator / denominator) as u32;

    if count > 0 && count < min_burst_spawn {
        count = min_burst_spawn;
    }

    count = count.min(max_population.get());
    count
}

fn sample_cadence(range: CadenceRange, rng: &mut SplitMix64) -> u32 {
    sample_uniform_inclusive(rng, range.min_ms().get(), range.max_ms().get())
}

struct BurstSchedule {
    burst_sizes: Vec<u32>,
    starts: Vec<u32>,
}

fn sample_burst_starts(
    total_count: u32,
    gap_range: maze_defence_core::BurstGapRange,
    scheduling: maze_defence_core::BurstSchedulingConfig,
    rng: &mut SplitMix64,
) -> BurstSchedule {
    let burst_count = resolve_burst_count(total_count, scheduling);
    let base = total_count / burst_count;
    let leftover = total_count % burst_count;

    let mut burst_sizes = Vec::with_capacity(burst_count as usize);
    for index in 0..burst_count {
        let size = base + u32::from(index < leftover);
        burst_sizes.push(size.max(1));
    }

    let mut starts = Vec::with_capacity(burst_count as usize);
    let jitter = sample_uniform_inclusive(rng, 0, gap_range.min_ms().get());
    let mut current_start = jitter;
    for index in 0..burst_count {
        starts.push(current_start);
        if index + 1 < burst_count {
            let gap =
                sample_uniform_inclusive(rng, gap_range.min_ms().get(), gap_range.max_ms().get());
            current_start = current_start.saturating_add(gap);
        }
    }

    BurstSchedule {
        burst_sizes,
        starts,
    }
}

fn resolve_burst_count(
    total_count: u32,
    scheduling: maze_defence_core::BurstSchedulingConfig,
) -> u32 {
    let nominal = scheduling.nominal_burst_size().get();
    let max_bursts = scheduling.burst_count_max().get();
    let mut burst_count = if nominal == 0 {
        1
    } else {
        ((total_count + nominal - 1) / nominal).max(1)
    };
    burst_count = burst_count.min(max_bursts);
    burst_count.max(1)
}

fn derive_base_seed(global_seed: u64, wave: WaveId, tier: u32) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(global_seed.to_le_bytes());
    hasher.update(wave.get().to_le_bytes());
    hasher.update(tier.to_le_bytes());
    finalize_seed(hasher)
}

fn derive_labeled_seed(base: u64, label: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(base.to_le_bytes());
    hasher.update(label.as_bytes());
    finalize_seed(hasher)
}

fn derive_species_seed(base: u64, species: SpeciesId) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(base.to_le_bytes());
    hasher.update(RNG_STREAM_SPECIES_PREFIX.as_bytes());
    hasher.update(species.get().to_le_bytes());
    finalize_seed(hasher)
}

fn finalize_seed(hasher: Sha256) -> u64 {
    let digest = hasher.finalize();
    let bytes: [u8; 8] = digest[0..8].try_into().expect("sha256 digest slice length");
    u64::from_le_bytes(bytes)
}

fn sample_uniform_inclusive(rng: &mut SplitMix64, min: u32, max: u32) -> u32 {
    if min == max {
        return min;
    }

    let range = u64::from(max.saturating_sub(min)) + 1;
    let value = rng.next_u64();
    let offset = value % range;
    min.saturating_add(offset as u32)
}

#[derive(Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        let seed = if seed == 0 { 0x9e3779b97f4a7c15 } else { seed };
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn next_unit(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / ((1u64 << 53) as f64);
        let value = self.next_u64() >> 11;
        (value as f64) * SCALE
    }

    fn next_unit_open(&mut self) -> f64 {
        loop {
            let value = self.next_unit();
            if value > 0.0 {
                return value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{
        BugColor, BurstGapRange, CadenceRange, CellCoord, CellRect, CellRectSize, DirichletWeight,
        Health, SpeciesPrototype, SpeciesTableVersion,
    };
    use std::num::NonZeroU32;

    fn make_species(
        id: u32,
        patch: u32,
        weight: u32,
        dirichlet: u32,
        max_population: u32,
        cadence: (u32, u32),
        gap: (u32, u32),
    ) -> SpeciesDefinition {
        SpeciesDefinition::new(
            SpeciesId::new(id),
            SpawnPatchId::new(patch),
            SpeciesPrototype::new(
                BugColor::from_rgb(0, 0, 0),
                Health::new(3),
                NonZeroU32::new(300).expect("non-zero step"),
            ),
            PressureWeight::new(NonZeroU32::new(weight).expect("non-zero weight")),
            DirichletWeight::new(NonZeroU32::new(dirichlet).expect("non-zero dirichlet")),
            0,
            NonZeroU32::new(max_population).expect("non-zero population"),
            CadenceRange::new(
                NonZeroU32::new(cadence.0).expect("cadence min"),
                NonZeroU32::new(cadence.1).expect("cadence max"),
            ),
            BurstGapRange::new(
                NonZeroU32::new(gap.0).expect("gap min"),
                NonZeroU32::new(gap.1).expect("gap max"),
            ),
        )
    }

    fn patch_descriptors() -> Vec<maze_defence_core::SpawnPatchDescriptor> {
        vec![maze_defence_core::SpawnPatchDescriptor::new(
            SpawnPatchId::new(0),
            CellCoord::new(0, 0),
            CellRect::from_origin_and_size(CellCoord::new(0, 0), CellRectSize::new(1, 1)),
        )]
    }

    fn default_pressure_config() -> PressureConfig {
        PressureConfig::new(
            maze_defence_core::PressureCurve::new(Pressure::new(1_200), Pressure::new(250)),
            DirichletWeight::new(NonZeroU32::new(2).expect("dirichlet")),
            maze_defence_core::BurstSchedulingConfig::new(
                NonZeroU32::new(10).expect("burst size"),
                NonZeroU32::new(8).expect("burst max"),
            ),
            NonZeroU32::new(2_000).expect("spawn cap"),
        )
    }

    fn sample_plan(difficulty: WaveDifficulty) -> AttackPlan {
        let species = vec![
            make_species(0, 0, 900, 3, 200, (250, 350), (2_000, 4_000)),
            make_species(1, 0, 1_500, 2, 120, (300, 400), (2_500, 5_000)),
        ];
        let table = SpeciesTableView::new(SpeciesTableVersion::new(1), &species);
        let patches = patch_descriptors();
        let patch_view = SpawnPatchTableView::new(&patches);
        let config = default_pressure_config();
        let context = WaveSeedContext::new(7_654_321, WaveId::new(12), 2);
        let command = Command::GenerateAttackPlan {
            wave: WaveId::new(12),
            difficulty,
        };
        let mut system = WaveGeneration::default();
        let mut events = Vec::new();
        system.handle(&[command], table, patch_view, &config, context, &mut events);
        match events.as_slice() {
            [Event::AttackPlanReady { plan, .. }] => plan.clone(),
            _ => panic!("expected AttackPlanReady event"),
        }
    }

    #[test]
    fn deterministic_generation_replays() {
        let plan_a = sample_plan(WaveDifficulty::Normal);
        let plan_b = sample_plan(WaveDifficulty::Normal);
        assert_eq!(plan_a, plan_b);
    }

    #[test]
    fn budget_respects_pressure() {
        let plan = sample_plan(WaveDifficulty::Normal);
        let species = vec![
            make_species(0, 0, 900, 3, 200, (250, 350), (2_000, 4_000)),
            make_species(1, 0, 1_500, 2, 120, (300, 400), (2_500, 5_000)),
        ];
        let pressure = plan.pressure().get();
        let mut total_cost = 0u128;
        for burst in plan.bursts() {
            let definition = species
                .iter()
                .find(|definition| definition.id() == burst.species())
                .expect("definition");
            let count = u128::from(burst.count().get());
            total_cost =
                total_cost.saturating_add(count * u128::from(definition.weight().get().get()));
        }
        let scaled_pressure = u128::from(pressure) * u128::from(PRESSURE_FIXED_POINT_SCALE);
        assert!(total_cost <= scaled_pressure);
    }

    #[test]
    fn bursts_cover_species_totals() {
        let plan = sample_plan(WaveDifficulty::Normal);
        let mut counts = std::collections::HashMap::new();
        for burst in plan.bursts() {
            *counts.entry(burst.species()).or_insert(0u32) += burst.count().get();
        }
        for (&species, &count) in &counts {
            assert!(count > 0, "species {species:?} should have positive count");
        }
    }

    #[test]
    fn hard_difficulty_adjusts_pressure() {
        let normal = sample_plan(WaveDifficulty::Normal);
        let hard = sample_plan(WaveDifficulty::Hard);
        assert!(hard.pressure().get() >= normal.pressure().get());
    }

    #[test]
    fn zero_pressure_emits_empty_plan() {
        let species = vec![make_species(
            0,
            0,
            1_000,
            2,
            200,
            (300, 300),
            (2_000, 2_000),
        )];
        let table = SpeciesTableView::new(SpeciesTableVersion::new(1), &species);
        let patches = patch_descriptors();
        let patch_view = SpawnPatchTableView::new(&patches);
        let config = PressureConfig::new(
            maze_defence_core::PressureCurve::new(Pressure::new(0), Pressure::new(0)),
            DirichletWeight::new(NonZeroU32::new(2).expect("dirichlet")),
            maze_defence_core::BurstSchedulingConfig::new(
                NonZeroU32::new(5).expect("burst"),
                NonZeroU32::new(4).expect("max"),
            ),
            NonZeroU32::new(2_000).expect("spawn cap"),
        );
        let context = WaveSeedContext::new(1, WaveId::new(0), 0);
        let command = Command::GenerateAttackPlan {
            wave: WaveId::new(0),
            difficulty: WaveDifficulty::Normal,
        };
        let mut system = WaveGeneration::default();
        let mut events = Vec::new();
        system.handle(&[command], table, patch_view, &config, context, &mut events);
        match events.as_slice() {
            [Event::AttackPlanReady { plan, .. }] => {
                assert!(plan.is_empty());
                assert_eq!(plan.pressure().get(), 0);
            }
            _ => panic!("expected AttackPlanReady"),
        }
    }
}
