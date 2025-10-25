#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic pressure v2 wave generation system stub.

use maze_defence_core::{
    DifficultyLevel, LevelId, PressureSpawnRecord, PressureWaveInputs, WaveId,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

const DEFAULT_RNG_SEED: u64 = 0x8955_06d3_3f6b_11d7;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0001_0000_01b3;

/// Aggregated tuning knobs controlling every adjustable aspect of the pressure generator.
#[derive(Clone, Debug)]
pub struct PressureTuning {
    /// Controls the logistic bug-count curve and sampling spread per §3.2 of the spec.
    pub count: CountTuning,
    /// Governs the wave-wide HP multiplier latent described in §3.3.1.
    pub hp: HpTuning,
    /// Governs the wave-wide speed multiplier latent described in §3.3.2.
    pub speed: SpeedTuning,
    /// Configures provisional component sampling, minimum-share enforcement, and Dirichlet allocation.
    pub components: ComponentTuning,
    /// Configures the per-bug pressure weighting used when aligning η in §5.
    pub pressure_weights: PressureWeightTuning,
    /// Controls cadence, start offsets, duration clamps, and compression behaviour from §6.
    pub cadence: CadenceTuning,
}

impl Default for PressureTuning {
    fn default() -> Self {
        Self {
            count: CountTuning::default(),
            hp: HpTuning::default(),
            speed: SpeedTuning::default(),
            components: ComponentTuning::default(),
            pressure_weights: PressureWeightTuning::default(),
            cadence: CadenceTuning::default(),
        }
    }
}

/// Bug-count logistic curve and sampling parameters.
#[derive(Clone, Debug)]
pub struct CountTuning {
    /// Lower-asymptote bug count C_min; raising this inflates how many bugs appear at tutorial difficulty.
    pub minimum: f32,
    /// Logistic plateau C_cap; increasing this raises the long-term soft ceiling for bug quantity.
    pub cap: f32,
    /// Logistic slope a; larger values make the count ramp faster with difficulty.
    pub slope: f32,
    /// Logistic midpoint D_mid; lowering this shifts the steepest growth earlier in the campaign.
    pub midpoint: f32,
    /// Standard deviation expressed as a ratio of the mean when sampling the truncated normal.
    pub deviation_ratio: f32,
    /// Hard minimum bug count allowed after sampling (lower clamp from §3.2).
    pub floor: u32,
}

impl Default for CountTuning {
    fn default() -> Self {
        Self {
            minimum: 20.0,
            cap: 1_000.0,
            slope: 1.2,
            midpoint: 3.0,
            deviation_ratio: 0.08,
            floor: 5,
        }
    }
}

/// HP latent parameters controlling wave durability.
#[derive(Clone, Debug)]
pub struct HpTuning {
    /// Amplitude of the early additive HP boost h_soft; larger values make low-D waves sturdier immediately.
    pub soft_boost_fraction: f32,
    /// Exponential decay rate k_h for the soft boost; higher values make the boost kick in sooner.
    pub soft_boost_rate: f32,
    /// Multiplicative growth g_h applied beyond the pivot; raising this accelerates HP growth at high difficulty.
    pub post_pivot_growth: f32,
    /// Difficulty pivot D_h after which multiplicative scaling applies.
    pub growth_pivot: f32,
    /// Standard deviation of the truncated normal draw around μ_HPmul(D).
    pub deviation: f32,
    /// Minimum allowed HP multiplier clamp.
    pub min_multiplier: f32,
    /// Maximum allowed HP multiplier clamp.
    pub max_multiplier: f32,
}

impl Default for HpTuning {
    fn default() -> Self {
        Self {
            soft_boost_fraction: 0.6,
            soft_boost_rate: 1.0,
            post_pivot_growth: 1.08,
            growth_pivot: 4.0,
            deviation: 0.05,
            min_multiplier: 0.6,
            max_multiplier: 2.2,
        }
    }
}

/// Speed latent parameters controlling wave pacing.
#[derive(Clone, Debug)]
pub struct SpeedTuning {
    /// Amplitude of the early additive speed boost analogous to h_soft; higher values quicken low-D waves.
    pub soft_boost_fraction: f32,
    /// Exponential decay rate for the speed soft boost; higher values front-load the acceleration.
    pub soft_boost_rate: f32,
    /// Multiplicative growth applied beyond the pivot; increasing this speeds up late-game waves.
    pub post_pivot_growth: f32,
    /// Difficulty pivot controlling when multiplicative speed scaling begins.
    pub growth_pivot: f32,
    /// Standard deviation of the truncated normal speed latent draw.
    pub deviation: f32,
    /// Minimum allowed speed multiplier clamp.
    pub min_multiplier: f32,
    /// Maximum allowed speed multiplier clamp.
    pub max_multiplier: f32,
}

impl Default for SpeedTuning {
    fn default() -> Self {
        Self {
            soft_boost_fraction: 0.5,
            soft_boost_rate: 0.9,
            post_pivot_growth: 1.06,
            growth_pivot: 3.5,
            deviation: 0.05,
            min_multiplier: 0.6,
            max_multiplier: 1.7,
        }
    }
}

/// Parameters that control provisional component sampling and merging.
#[derive(Clone, Debug)]
pub struct ComponentTuning {
    /// Baseline κ(D) intercept; increasing this raises the expected component count even at low difficulty.
    pub poisson_intercept: f32,
    /// Linear growth applied to κ(D) per difficulty step above 1; higher slope produces more components late-game.
    pub poisson_slope: f32,
    /// Hard cap K_abs_max restricting provisional component count.
    pub poisson_cap: u32,
    /// Minimum share threshold enforced during merges; raising this forces larger post-merge species.
    pub minimum_share: f32,
    /// Symmetric Dirichlet concentration α_mix; larger values make component allocations more even.
    pub dirichlet_concentration: f32,
    /// Log-space standard deviation for HP when sampling component centres.
    pub log_hp_sigma: f32,
    /// Log-space standard deviation for speed when sampling component centres.
    pub log_speed_sigma: f32,
    /// Correlation coefficient ρ tying HP and speed draws together.
    pub log_correlation: f32,
    /// Minimum HP multiplier allowed for component centres before scaling.
    pub hp_multiplier_min: f32,
    /// Maximum HP multiplier allowed for component centres before scaling.
    pub hp_multiplier_max: f32,
    /// Minimum speed multiplier allowed for component centres before scaling.
    pub speed_multiplier_min: f32,
    /// Maximum speed multiplier allowed for component centres before scaling.
    pub speed_multiplier_max: f32,
}

impl Default for ComponentTuning {
    fn default() -> Self {
        Self {
            poisson_intercept: 1.2,
            poisson_slope: 0.45,
            poisson_cap: 6,
            minimum_share: 0.10,
            dirichlet_concentration: 1.5,
            log_hp_sigma: 0.10,
            log_speed_sigma: 0.10,
            log_correlation: -0.5,
            hp_multiplier_min: 0.6,
            hp_multiplier_max: 2.2,
            speed_multiplier_min: 0.6,
            speed_multiplier_max: 1.7,
        }
    }
}

/// Weighting parameters used by the pressure alignment function.
#[derive(Clone, Debug)]
pub struct PressureWeightTuning {
    /// Linear HP weight α in pressure(hp, v); increasing this makes toughness dominate the pressure budget.
    pub alpha: f32,
    /// Speed weight β in pressure(hp, v); increasing this emphasises fast species when aligning η.
    pub beta: f32,
    /// Exponent γ applied to speed in the pressure equation.
    pub gamma: f32,
}

impl Default for PressureWeightTuning {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            beta: 0.6,
            gamma: 1.0,
        }
    }
}

/// Cadence, start offset, and duration tuning parameters.
#[derive(Clone, Debug)]
pub struct CadenceTuning {
    /// Hard minimum cadence cad_min enforced even after compression.
    pub cadence_min_ms: u32,
    /// Hard maximum cadence cad_max allowed before compression.
    pub cadence_max_ms: u32,
    /// Ratio applied to μ_cad(D) to derive the truncated normal deviation.
    pub cadence_deviation_ratio: f32,
    /// Base cadence at D=1 (μ_cad intercept); lowering this quickens every wave.
    pub cadence_base_ms: f32,
    /// Linear cadence decrease per difficulty step; larger negative slope accelerates high-D waves.
    pub cadence_slope_ms: f32,
    /// Base start delay at D=1 (μ_start intercept).
    pub start_base_ms: f32,
    /// Linear start-delay decrease per difficulty step; more negative slope launches waves sooner.
    pub start_slope_ms: f32,
    /// Ratio applied to μ_start(D) when computing the truncated normal deviation.
    pub start_deviation_ratio: f32,
    /// Hard cap on start offsets start_max.
    pub start_max_ms: u32,
    /// Target wave duration at D=1 used when enforcing compression.
    pub duration_base_ms: f32,
    /// Linear change applied to the duration target per difficulty step.
    pub duration_slope_ms: f32,
}

impl Default for CadenceTuning {
    fn default() -> Self {
        Self {
            cadence_min_ms: 120,
            cadence_max_ms: 2_000,
            cadence_deviation_ratio: 0.08,
            cadence_base_ms: 600.0,
            cadence_slope_ms: -40.0,
            start_base_ms: 1_000.0,
            start_slope_ms: -120.0,
            start_deviation_ratio: 0.15,
            start_max_ms: 10_000,
            duration_base_ms: 60_000.0,
            duration_slope_ms: -1_500.0,
        }
    }
}

/// Stub implementation of the pressure v2 generator.
#[derive(Debug)]
pub struct PressureV2 {
    tuning: PressureTuning,
    rng: ChaCha8Rng,
    telemetry: PressureTelemetry,
}

impl Default for PressureV2 {
    fn default() -> Self {
        Self::new(PressureTuning::default())
    }
}

impl PressureV2 {
    /// Creates a new generator with the provided tuning surface.
    #[must_use]
    pub fn new(tuning: PressureTuning) -> Self {
        Self {
            tuning,
            rng: ChaCha8Rng::seed_from_u64(DEFAULT_RNG_SEED),
            telemetry: PressureTelemetry::default(),
        }
    }

    /// Returns a mutable reference to the global tuning knobs so designers can adjust wave behaviour.
    pub fn tuning_mut(&mut self) -> &mut PressureTuning {
        &mut self.tuning
    }

    /// Returns the most recent telemetry snapshot emitted by the generator.
    pub fn telemetry(&self) -> &PressureTelemetry {
        &self.telemetry
    }

    /// Generates v2 pressure spawns according to the provided inputs.
    pub fn generate(&mut self, inputs: &PressureWaveInputs, out: &mut Vec<PressureSpawnRecord>) {
        self.reseed_rng(inputs);
        // RNG draw order (documented here for determinism auditing):
        // 1) Difficulty latent draws (§3) consume the first sequence elements.
        // 2) Species sampling (§4) consumes the subsequent draws in the order they appear in the spec.
        // 3) Cadence sampling (§6) consumes the remaining draws before compression.
        self.telemetry.reset();
        self.telemetry.ensure_placeholders();
        out.clear();
        todo!("pressure v2 generation not implemented");
    }

    fn reseed_rng(&mut self, inputs: &PressureWaveInputs) {
        let seed = wave_seed_hash(
            inputs.game_seed(),
            inputs.level_id(),
            inputs.wave(),
            inputs.difficulty(),
        );
        self.rng = ChaCha8Rng::seed_from_u64(seed);
    }
}

fn wave_seed_hash(
    game_seed: u64,
    level_id: LevelId,
    wave: WaveId,
    difficulty: DifficultyLevel,
) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv1a(hash, &game_seed.to_le_bytes());
    hash = fnv1a(hash, &level_id.get().to_le_bytes());
    hash = fnv1a(hash, &wave.get().to_le_bytes());
    fnv1a(hash, &difficulty.get().to_le_bytes())
}

fn fnv1a(mut state: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        state ^= u64::from(*byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

/// Telemetry accumulator for the pressure generator.
#[derive(Clone, Debug, Default)]
pub struct PressureTelemetry {
    difficulty_latents: DifficultyLatentsTelemetry,
    species_merge: Vec<SpeciesMergeTelemetry>,
    eta_scaling: EtaScalingTelemetry,
    cadence_compression: CadenceCompressionTelemetry,
}

impl PressureTelemetry {
    /// Clears any accumulated telemetry back to placeholder defaults.
    pub fn reset(&mut self) {
        self.difficulty_latents = DifficultyLatentsTelemetry::default();
        self.species_merge.clear();
        self.eta_scaling = EtaScalingTelemetry::default();
        self.cadence_compression = CadenceCompressionTelemetry::default();
    }

    /// Ensures that every telemetry stream has at least a placeholder record available.
    pub fn ensure_placeholders(&mut self) {
        self.difficulty_latents.recorded = false;
        self.eta_scaling.recorded = false;
        self.cadence_compression.recorded = false;
        if self.species_merge.is_empty() {
            self.species_merge.push(SpeciesMergeTelemetry::default());
        }
    }

    /// Begins recording the difficulty latent telemetry entry.
    pub fn difficulty_latents_mut(&mut self) -> &mut DifficultyLatentsTelemetry {
        self.difficulty_latents.recorded = true;
        &mut self.difficulty_latents
    }

    /// Appends a species merge record and marks it as an actual merge event.
    pub fn push_species_merge(&mut self) -> &mut SpeciesMergeTelemetry {
        self.species_merge
            .push(SpeciesMergeTelemetry::merge_placeholder());
        self.species_merge
            .last_mut()
            .expect("merge record should exist")
    }

    /// Accesses the currently accumulated species merge telemetry records.
    pub fn species_merge(&self) -> &[SpeciesMergeTelemetry] {
        &self.species_merge
    }

    /// Accesses the difficulty latent telemetry entry.
    pub fn difficulty_latents(&self) -> &DifficultyLatentsTelemetry {
        &self.difficulty_latents
    }

    /// Accesses the η scaling telemetry entry.
    pub fn eta_scaling_mut(&mut self) -> &mut EtaScalingTelemetry {
        self.eta_scaling.recorded = true;
        &mut self.eta_scaling
    }

    /// Accesses the cadence compression telemetry entry.
    pub fn cadence_compression_mut(&mut self) -> &mut CadenceCompressionTelemetry {
        self.cadence_compression.recorded = true;
        &mut self.cadence_compression
    }

    /// Returns the η scaling telemetry entry.
    pub fn eta_scaling(&self) -> &EtaScalingTelemetry {
        &self.eta_scaling
    }

    /// Returns the cadence compression telemetry entry.
    pub fn cadence_compression(&self) -> &CadenceCompressionTelemetry {
        &self.cadence_compression
    }
}

/// Difficulty latent telemetry entry carrying placeholder values until the latent implementation lands.
#[derive(Clone, Debug, Default)]
pub struct DifficultyLatentsTelemetry {
    recorded: bool,
    /// Placeholder bug count mean stored for upcoming implementations.
    pub bug_count_mean: f32,
    /// Placeholder sampled bug count stored for upcoming implementations.
    pub bug_count_sampled: u32,
    /// Placeholder HP multiplier latent stored for upcoming implementations.
    pub hp_multiplier: f32,
    /// Placeholder speed multiplier latent stored for upcoming implementations.
    pub speed_multiplier: f32,
}

impl DifficultyLatentsTelemetry {
    /// Indicates whether real difficulty latent data has been populated.
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }
}

/// Species merge telemetry entry which records each merge that occurs during §4.4.
#[derive(Clone, Debug, Default)]
pub struct SpeciesMergeTelemetry {
    recorded: bool,
    /// Placeholder index of the component that was merged away.
    pub from_component: u32,
    /// Placeholder index of the component that absorbed the merge prior to the operation.
    pub to_component: u32,
    /// Placeholder bug count of the merged component.
    pub from_count: u32,
    /// Placeholder bug count of the receiving component before the merge.
    pub to_count_before: u32,
    /// Placeholder bug count of the receiving component after the merge.
    pub to_count_after: u32,
    /// Placeholder log-distance used for merge selection.
    pub log_distance: f32,
}

impl SpeciesMergeTelemetry {
    /// Creates a placeholder merge record flagged as an actual merge.
    #[must_use]
    fn merge_placeholder() -> Self {
        let mut record = Self::default();
        record.recorded = true;
        record
    }

    /// Indicates whether the record represents an actual merge (as opposed to a placeholder).
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }
}

/// Telemetry entry describing the η scaling decision made in §5.
#[derive(Clone, Debug, Default)]
pub struct EtaScalingTelemetry {
    recorded: bool,
    /// Placeholder resolved η value.
    pub eta_final: f32,
    /// Placeholder clamp indicator describing whether η hit its bounds.
    pub eta_clamped: bool,
    /// Placeholder target pressure value `P_wave`.
    pub pressure_target: f32,
    /// Placeholder measured pressure after applying η.
    pub pressure_after_eta: f32,
}

impl EtaScalingTelemetry {
    /// Indicates whether real η scaling data has been populated.
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }
}

/// Telemetry entry describing cadence compression results from §6.
#[derive(Clone, Debug, Default)]
pub struct CadenceCompressionTelemetry {
    recorded: bool,
    /// Placeholder maximum spawn time before compression.
    pub t_end_before: u32,
    /// Placeholder target deploy duration `T_target(D)`.
    pub t_target: u32,
    /// Placeholder compression factor applied to cadences.
    pub compression_factor: f32,
    /// Placeholder indicator describing whether any cadence hit the `cad_min` floor.
    pub hit_cadence_min: bool,
    /// Placeholder maximum spawn time after compression.
    pub t_end_after: u32,
}

impl CadenceCompressionTelemetry {
    /// Indicates whether real cadence compression data has been populated.
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    #[test]
    fn wave_seed_hash_uses_all_inputs() {
        let base_inputs =
            PressureWaveInputs::new(7, LevelId::new(3), WaveId::new(1), DifficultyLevel::new(2));
        let mut generator = PressureV2::default();
        let base_seed = wave_seed_hash(
            base_inputs.game_seed(),
            base_inputs.level_id(),
            base_inputs.wave(),
            base_inputs.difficulty(),
        );
        let seeds = [
            wave_seed_hash(
                8,
                base_inputs.level_id(),
                base_inputs.wave(),
                base_inputs.difficulty(),
            ),
            wave_seed_hash(
                base_inputs.game_seed(),
                LevelId::new(4),
                base_inputs.wave(),
                base_inputs.difficulty(),
            ),
            wave_seed_hash(
                base_inputs.game_seed(),
                base_inputs.level_id(),
                WaveId::new(2),
                base_inputs.difficulty(),
            ),
            wave_seed_hash(
                base_inputs.game_seed(),
                base_inputs.level_id(),
                base_inputs.wave(),
                DifficultyLevel::new(3),
            ),
        ];

        for seed in seeds {
            assert_ne!(base_seed, seed);
        }

        generator.reseed_rng(&base_inputs);
        let first_draw = generator.rng.next_u32();
        generator.reseed_rng(&base_inputs);
        let second_draw = generator.rng.next_u32();
        assert_eq!(first_draw, second_draw);
    }

    #[test]
    fn rng_sequence_is_stable_across_instances() {
        let inputs = PressureWaveInputs::new(
            42,
            LevelId::new(11),
            WaveId::new(5),
            DifficultyLevel::new(9),
        );
        let mut generator_a = PressureV2::default();
        let mut generator_b = PressureV2::default();
        generator_a.reseed_rng(&inputs);
        generator_b.reseed_rng(&inputs);

        let draws_a = [
            generator_a.rng.next_u32(),
            generator_a.rng.next_u32(),
            generator_a.rng.next_u32(),
        ];
        let draws_b = [
            generator_b.rng.next_u32(),
            generator_b.rng.next_u32(),
            generator_b.rng.next_u32(),
        ];

        assert_eq!(draws_a, draws_b);
    }

    #[test]
    fn telemetry_placeholders_cover_all_streams() {
        let mut telemetry = PressureTelemetry::default();
        telemetry.ensure_placeholders();
        assert!(!telemetry.species_merge().is_empty());
        assert!(!telemetry.difficulty_latents().is_recorded());
        assert!(!telemetry.eta_scaling().is_recorded());
        assert!(!telemetry.cadence_compression().is_recorded());

        let merge = telemetry.push_species_merge();
        assert!(merge.is_recorded());
        assert_eq!(telemetry.species_merge().len(), 2);
    }
}
