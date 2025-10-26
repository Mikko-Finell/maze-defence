#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic pressure v2 wave generation system stub.

use std::cmp::Ordering;

use macroquad::color::Color as MacroquadColor;
use maze_defence_core::{
    DifficultyLevel, LevelId, PressureSpawnRecord, PressureWaveInputs, WaveId,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Gamma, Poisson, StandardNormal};

const DEFAULT_RNG_SEED: u64 = 0x8955_06d3_3f6b_11d7;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0001_0000_01b3;
const ETA_MIN: f32 = 0.75;
const ETA_MAX: f32 = 1.5;
const ETA_BISECTION_STEPS: u32 = 24;

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
    work: WaveWork,
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
            work: WaveWork::default(),
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
        // RNG draw order (documented for determinism auditing):
        //   1: `draw_bug_count` pulls a truncated normal using
        //      `PressureTuning::count.{deviation_ratio,floor,cap}`.
        //   2: `draw_hp_multiplier` pulls a truncated normal using
        //      `PressureTuning::hp.{deviation,min_multiplier,max_multiplier}`.
        //   3: `draw_speed_multiplier` pulls a truncated normal using
        //      `PressureTuning::speed.{deviation,min_multiplier,max_multiplier}`.
        //   4: `draw_raw_component_count` samples the Poisson proposal with
        //      `PressureTuning::components.{poisson_intercept,poisson_slope}`.
        //   5-6 per provisional component: `populate_component_centres` consumes
        //      two `StandardNormal` draws to build log-space HP/speed using the
        //      `components.log_*` sigmas, correlation, and multiplier clamps.
        //   7+ per provisional component: `allocate_dirichlet_counts` draws
        //      Gammas parameterised by `components.dirichlet_concentration`.
        //   Cadence realisation: for each surviving component,
        //      `sample_cadence_and_start_offsets` pulls a cadence draw bounded
        //      by `cadence_min_ms`/`cadence_max_ms` and a start-offset draw
        //      capped by `start_max_ms` with deviations derived from
        //      `cadence_deviation_ratio`/`start_deviation_ratio`.
        //   Tint assignment: `draw_unique_tint` consumes hue, saturation, then
        //      value for each component before falling back to deterministic
        //      hues when the random attempts collide.
        self.telemetry.reset();
        self.telemetry.ensure_placeholders();
        self.work.reset();
        self.compute_difficulty_latents(inputs);
        self.sample_provisional_species(inputs);
        self.align_pressure_with_eta();
        self.sample_cadence_and_start_offsets(inputs);
        self.enforce_duration_caps(inputs);
        self.write_final_spawn_records(out);
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

    fn compute_difficulty_latents(&mut self, inputs: &PressureWaveInputs) {
        let difficulty = inputs.difficulty().get() as f32;
        let count_latent = self.draw_bug_count(difficulty);
        let hp_latent = self.draw_hp_multiplier(difficulty);
        let speed_latent = self.draw_speed_multiplier(difficulty);

        let hp_wave = BASE_HP * hp_latent.multiplier;
        let speed_wave = speed_latent.multiplier;
        let per_bug_pressure = self.tuning.pressure_weights.alpha * hp_wave
            + self.tuning.pressure_weights.beta
                * speed_wave.powf(self.tuning.pressure_weights.gamma);
        let pressure_target = (count_latent.sampled as f32 * per_bug_pressure).round() as u32;

        let difficulty_work = &mut self.work.difficulty;
        *difficulty_work = WaveDifficultyLatents {
            count_mean: count_latent.mean,
            bug_count: count_latent.sampled,
            hp_multiplier: hp_latent.multiplier,
            speed_multiplier: speed_latent.multiplier,
        };

        let telemetry = self.telemetry.difficulty_latents_mut();
        telemetry.bug_count_mean = difficulty_work.count_mean;
        telemetry.bug_count_sampled = difficulty_work.bug_count;
        telemetry.hp_multiplier = difficulty_work.hp_multiplier;
        telemetry.speed_multiplier = difficulty_work.speed_multiplier;
        telemetry.hp_mean_multiplier = hp_latent.mean_multiplier;
        telemetry.speed_mean_multiplier = speed_latent.mean_multiplier;
        telemetry.hp_absolute = hp_wave;
        telemetry.speed_absolute = speed_wave;
        telemetry.per_bug_pressure = per_bug_pressure;
        telemetry.pressure_target = pressure_target;

        self.work.pressure_target = pressure_target;
        self.work.hp_wave = hp_wave;
        self.work.speed_wave = speed_wave;
        self.work.per_bug_pressure = per_bug_pressure;
        debug_assert!(self.work.hp_wave >= BASE_HP * self.tuning.hp.min_multiplier);
        debug_assert!(self.work.speed_wave >= self.tuning.speed.min_multiplier);
        debug_assert!(self.work.per_bug_pressure >= 0.0);
    }

    fn draw_bug_count(&mut self, difficulty: f32) -> CountLatent {
        let logistic = self.count_mean(difficulty);
        let deviation = logistic * self.tuning.count.deviation_ratio;
        let floor = self.tuning.count.floor as f32;
        // RNG draw #1: bug count latent truncated normal sample using
        // `count.deviation_ratio` for spread and clamped to `count.floor`/`count.cap`.
        let sample = draw_truncated_normal(
            &mut self.rng,
            logistic,
            deviation,
            floor,
            self.tuning.count.cap,
        );
        let rounded = sample.round();
        let clamped = rounded.clamp(floor, self.tuning.count.cap);
        CountLatent {
            mean: logistic,
            sampled: clamped as u32,
        }
    }

    fn draw_hp_multiplier(&mut self, difficulty: f32) -> HpLatent {
        let mean_multiplier = self.hp_mean_multiplier(difficulty);
        // RNG draw #2: HP multiplier truncated normal sample controlled by
        // `hp.deviation` and clamped to `hp.min_multiplier`/`hp.max_multiplier`.
        let sampled_multiplier = draw_truncated_normal(
            &mut self.rng,
            mean_multiplier,
            self.tuning.hp.deviation,
            self.tuning.hp.min_multiplier,
            self.tuning.hp.max_multiplier,
        );

        HpLatent {
            mean_multiplier,
            multiplier: sampled_multiplier,
        }
    }

    fn draw_speed_multiplier(&mut self, difficulty: f32) -> SpeedLatent {
        let mean_multiplier = self.speed_mean_multiplier(difficulty);
        // RNG draw #3: speed multiplier truncated normal sample controlled by
        // `speed.deviation` and clamped to `speed.min_multiplier`/`speed.max_multiplier`.
        let sampled_multiplier = draw_truncated_normal(
            &mut self.rng,
            mean_multiplier,
            self.tuning.speed.deviation,
            self.tuning.speed.min_multiplier,
            self.tuning.speed.max_multiplier,
        );

        SpeedLatent {
            mean_multiplier,
            multiplier: sampled_multiplier,
        }
    }

    fn sample_provisional_species(&mut self, inputs: &PressureWaveInputs) {
        let difficulty = inputs.difficulty().get() as f32;
        let bug_count = self.work.difficulty.bug_count;
        let minimum_species_size =
            ((self.tuning.components.minimum_share * bug_count as f32).ceil() as u32).max(1);
        self.work.minimum_species_size = minimum_species_size;

        let raw_count = self.draw_raw_component_count(difficulty);
        let soft_capped = raw_count.min(self.tuning.components.poisson_cap);
        let count_cap = (bug_count / minimum_species_size).max(1);
        let final_count = soft_capped.min(count_cap).max(1);
        self.work.provisional_species_count = final_count;

        self.populate_component_centres(difficulty, final_count as usize);
        self.allocate_dirichlet_counts(bug_count);
        self.enforce_minimum_share();
    }

    // §5.2 fixed-step bisection
    fn align_pressure_with_eta(&mut self) {
        if self.work.provisional_species.is_empty() {
            self.work.eta = 1.0;
            self.work.eta_clamped = false;
            self.work.pressure_after_eta = 0.0;
            let telemetry = self.telemetry.eta_scaling_mut();
            telemetry.eta_final = 1.0;
            telemetry.eta_clamped = false;
            telemetry.pressure_target = self.work.pressure_target as f32;
            telemetry.pressure_after_eta = 0.0;
            return;
        }

        let target_pressure = self.work.pressure_target as f32;
        let pressure_at_min = self.total_pressure_for_eta(ETA_MIN);
        let pressure_at_max = self.total_pressure_for_eta(ETA_MAX);

        let mut lower = ETA_MIN;
        let mut upper = ETA_MAX;
        for _ in 0..ETA_BISECTION_STEPS {
            let midpoint = 0.5 * (lower + upper);
            let pressure = self.total_pressure_for_eta(midpoint);
            if pressure > target_pressure {
                upper = midpoint;
            } else {
                lower = midpoint;
            }
        }

        let mut eta = 0.5 * (lower + upper);
        let eta_clamped = if target_pressure <= pressure_at_min {
            eta = ETA_MIN;
            true
        } else if target_pressure >= pressure_at_max {
            eta = ETA_MAX;
            true
        } else {
            let clamped = eta.clamp(ETA_MIN, ETA_MAX);
            let was_clamped = clamped != eta;
            eta = clamped;
            was_clamped
        };

        let weights = &self.tuning.pressure_weights;
        let mut realised_pressure = 0.0;
        for component in self.work.provisional_species.iter_mut() {
            let hp_post = eta * component.hp_pre;
            let speed_post = eta * component.speed_pre;
            let pressure_weight_post =
                weights.alpha * hp_post + weights.beta * speed_post.powf(weights.gamma);
            component.hp_post = hp_post;
            component.speed_post = speed_post;
            component.pressure_weight_post = pressure_weight_post;
            realised_pressure += component.bug_count as f32 * pressure_weight_post;
        }

        self.work.eta = eta;
        self.work.eta_clamped = eta_clamped;
        self.work.pressure_after_eta = realised_pressure;

        let telemetry = self.telemetry.eta_scaling_mut();
        telemetry.eta_final = eta;
        telemetry.eta_clamped = eta_clamped;
        telemetry.pressure_target = target_pressure;
        telemetry.pressure_after_eta = realised_pressure;
    }

    fn sample_cadence_and_start_offsets(&mut self, inputs: &PressureWaveInputs) {
        if self.work.provisional_species.is_empty() {
            return;
        }

        let difficulty = inputs.difficulty().get() as f32;
        let cadence_mean = self.cadence_mean_ms(difficulty);
        let start_mean = self.start_offset_mean_ms(difficulty);
        let tuning = &self.tuning.cadence;
        let cadence_min = tuning.cadence_min_ms as f32;
        let cadence_max = tuning.cadence_max_ms as f32;
        let start_max = tuning.start_max_ms as f32;

        for component in self.work.provisional_species.iter_mut() {
            // RNG draw: per-species cadence sample; `cadence_base_ms` +
            // `cadence_slope_ms` set the mean while `cadence_deviation_ratio`
            // expands/shrinks the spread before the min/max clamps.
            let cadence_sample = draw_truncated_normal(
                &mut self.rng,
                cadence_mean,
                cadence_mean * tuning.cadence_deviation_ratio,
                cadence_min,
                cadence_max,
            );
            let cadence_ms = cadence_sample.round().clamp(cadence_min, cadence_max) as u32;
            component.cadence_ms = cadence_ms.max(tuning.cadence_min_ms);

            // RNG draw: per-species start offset sample centred on
            // `start_base_ms` + `start_slope_ms` * (D-1) with spread controlled
            // by `start_deviation_ratio` and clamped to `start_max_ms`.
            let start_sample = draw_truncated_normal(
                &mut self.rng,
                start_mean,
                start_mean * tuning.start_deviation_ratio,
                0.0,
                start_max,
            );
            let start_offset = start_sample.round().clamp(0.0, start_max) as u32;
            component.start_offset_ms = start_offset;

            component.spawn_times.clear();
            component.spawn_times.reserve(component.bug_count as usize);
            let cadence = component.cadence_ms as u64;
            let start = component.start_offset_ms as u64;
            for index in 0..component.bug_count {
                let time = start.saturating_add(cadence.saturating_mul(index as u64));
                component.spawn_times.push(time.min(u32::MAX as u64) as u32);
            }
        }
    }

    fn enforce_duration_caps(&mut self, inputs: &PressureWaveInputs) {
        let difficulty = inputs.difficulty().get() as f32;
        let target_duration = self.duration_target_ms(difficulty);

        let mut t_end_before = 0u32;
        for component in &self.work.provisional_species {
            if let Some(&time) = component.spawn_times.last() {
                t_end_before = t_end_before.max(time);
            }
        }

        let mut compression_factor = 1.0f32;
        let mut t_end_after = t_end_before;

        if !self.work.provisional_species.is_empty() && t_end_before > target_duration {
            let factor = f64::from(t_end_before) / f64::from(target_duration);
            compression_factor = factor as f32;
            t_end_after = 0;
            let cadence_min = self.tuning.cadence.cadence_min_ms;
            for component in self.work.provisional_species.iter_mut() {
                let divided = (f64::from(component.cadence_ms) / factor).floor();
                let mut cadence = if divided.is_finite() {
                    divided.max(1.0).min(f64::from(u32::MAX)) as u32
                } else {
                    component.cadence_ms
                };
                if cadence < cadence_min {
                    cadence = cadence_min;
                }
                component.cadence_ms = cadence;
                component.spawn_times.clear();
                component.spawn_times.reserve(component.bug_count as usize);
                let cadence_u64 = cadence as u64;
                let start_u64 = component.start_offset_ms as u64;
                for index in 0..component.bug_count {
                    let time = start_u64.saturating_add(cadence_u64.saturating_mul(index as u64));
                    component.spawn_times.push(time.min(u32::MAX as u64) as u32);
                }
                if let Some(&last) = component.spawn_times.last() {
                    t_end_after = t_end_after.max(last);
                }
            }
        }

        let cadence_min = self.tuning.cadence.cadence_min_ms;
        let hit_cadence_min = self
            .work
            .provisional_species
            .iter()
            .any(|component| component.cadence_ms == cadence_min);

        let telemetry = self.telemetry.cadence_compression_mut();
        telemetry.t_end_before = t_end_before;
        telemetry.t_target = target_duration;
        telemetry.compression_factor = compression_factor;
        telemetry.hit_cadence_min = hit_cadence_min;
        telemetry.t_end_after = t_end_after;
    }

    fn write_final_spawn_records(&self, out: &mut Vec<PressureSpawnRecord>) {
        out.clear();
        let total_spawns: usize = self
            .work
            .provisional_species
            .iter()
            .map(|component| component.spawn_times.len())
            .sum();

        if total_spawns == 0 {
            return;
        }

        let mut scratch: Vec<(u32, u32, u32, f32, f32)> = Vec::with_capacity(total_spawns);
        for (species_id, component) in self.work.provisional_species.iter().enumerate() {
            for (index, &time) in component.spawn_times.iter().enumerate() {
                scratch.push((
                    time,
                    species_id as u32,
                    index as u32,
                    component.hp_post,
                    component.speed_post,
                ));
            }
        }

        scratch.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        out.reserve(scratch.len());
        for (time, species_id, _, hp, speed) in scratch {
            let hp_value = hp.round().clamp(1.0, u32::MAX as f32) as u32;
            out.push(PressureSpawnRecord::new(time, hp_value, speed, species_id));
        }
    }

    fn total_pressure_for_eta(&self, eta: f32) -> f32 {
        let weights = &self.tuning.pressure_weights;
        self.work
            .provisional_species
            .iter()
            .fold(0.0, |acc, component| {
                let hp = eta * component.hp_pre;
                let speed = eta * component.speed_pre;
                let per_bug = weights.alpha * hp + weights.beta * speed.powf(weights.gamma);
                acc + component.bug_count as f32 * per_bug
            })
    }

    fn draw_raw_component_count(&mut self, difficulty: f32) -> u32 {
        let mean = self.component_poisson_mean(difficulty);
        // RNG draw #4: provisional component Poisson proposal using the
        // `components.poisson_intercept` + `poisson_slope` growth curve.
        let distribution = Poisson::new(f64::from(mean)).expect("positive Poisson mean");
        distribution.sample(&mut self.rng) as u32
    }

    fn component_poisson_mean(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.components;
        let delta = (difficulty - 1.0).max(0.0);
        let mean = tuning.poisson_intercept + tuning.poisson_slope * delta;
        mean.max(0.1)
    }

    fn populate_component_centres(&mut self, difficulty: f32, count: usize) {
        self.work.provisional_species.clear();
        self.work.provisional_species.reserve(count);

        let mean_hp_multiplier = self.hp_mean_multiplier(difficulty);
        let mean_speed_multiplier = self.speed_mean_multiplier(difficulty);
        let tuning = &self.tuning.components;
        let rho = tuning.log_correlation.clamp(-0.999, 0.999);
        let orthogonal_scale = (1.0 - rho * rho).max(0.0).sqrt();

        for _ in 0..count {
            // RNG draws #5-6: bivariate log-space component centre using
            // `components.log_hp_sigma`, `log_speed_sigma`, and `log_correlation`
            // before clamping to the multiplier bounds.
            let z_hp: f32 = self.rng.sample(StandardNormal);
            let z_speed: f32 = self.rng.sample(StandardNormal);

            let log_hp = mean_hp_multiplier.ln() + tuning.log_hp_sigma * z_hp;
            let log_speed = mean_speed_multiplier.ln()
                + tuning.log_speed_sigma * (rho * z_hp + orthogonal_scale * z_speed);

            let hp_multiplier = log_hp
                .exp()
                .clamp(tuning.hp_multiplier_min, tuning.hp_multiplier_max);
            let speed_multiplier = log_speed
                .exp()
                .clamp(tuning.speed_multiplier_min, tuning.speed_multiplier_max);

            let log_hp_clamped = hp_multiplier.ln();
            let log_speed_clamped = speed_multiplier.ln();

            let hp_pre = BASE_HP * hp_multiplier;
            let speed_pre = speed_multiplier;
            let pressure_weight = self.tuning.pressure_weights.alpha * hp_pre
                + self.tuning.pressure_weights.beta
                    * speed_pre.powf(self.tuning.pressure_weights.gamma);

            self.work.provisional_species.push(ComponentWork::new(
                hp_pre,
                speed_pre,
                pressure_weight,
                log_hp_clamped,
                log_speed_clamped,
            ));
        }
    }

    fn allocate_dirichlet_counts(&mut self, bug_count: u32) {
        let component_count = self.work.provisional_species.len();
        debug_assert!(component_count > 0);

        let alpha = self.tuning.components.dirichlet_concentration;
        let dirichlet =
            Gamma::new(f64::from(alpha), 1.0).expect("positive Dirichlet concentration");
        let mut draws = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            // RNG draw #7+: Dirichlet gamma sample per component governed by
            // `components.dirichlet_concentration`.
            draws.push(dirichlet.sample(&mut self.rng) as f32);
        }

        let sum: f32 = draws.iter().sum();
        let normaliser = if sum.is_finite() && sum > f32::EPSILON {
            sum
        } else {
            component_count as f32
        };

        let mut floor_sum = 0u32;
        let mut remainders = Vec::with_capacity(component_count);
        for (component, draw) in self.work.provisional_species.iter_mut().zip(draws.iter()) {
            let share = (*draw / normaliser).max(0.0);
            component.dirichlet_share = share;
            let fractional = share * bug_count as f32;
            component.fractional_count = fractional;
            let floor = fractional.floor() as u32;
            component.bug_count = floor;
            floor_sum = floor_sum.saturating_add(floor);
            remainders.push(fractional - floor as f32);
        }

        let bug_count_i32 = bug_count as i32;
        let floor_sum_i32 = floor_sum as i32;
        match bug_count_i32 - floor_sum_i32 {
            diff if diff > 0 => {
                let mut indices: Vec<usize> = (0..component_count).collect();
                indices.sort_by(|a, b| {
                    remainders[*b]
                        .partial_cmp(&remainders[*a])
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.cmp(b))
                });
                for index in indices.into_iter().take(diff as usize) {
                    self.work.provisional_species[index].bug_count = self.work.provisional_species
                        [index]
                        .bug_count
                        .saturating_add(1);
                }
            }
            diff if diff < 0 => {
                let mut indices: Vec<usize> = (0..component_count).collect();
                indices.sort_by(|a, b| {
                    remainders[*a]
                        .partial_cmp(&remainders[*b])
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.cmp(b))
                });
                for index in indices.into_iter().take(diff.abs() as usize) {
                    self.work.provisional_species[index].bug_count = self.work.provisional_species
                        [index]
                        .bug_count
                        .saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    fn enforce_minimum_share(&mut self) {
        let minimum_share = self.work.minimum_species_size;
        let total_bugs = self.work.difficulty.bug_count;
        let sigma_hp = self.tuning.components.log_hp_sigma.max(f32::EPSILON);
        let sigma_speed = self.tuning.components.log_speed_sigma.max(f32::EPSILON);

        let components = &mut self.work.provisional_species;
        if components.is_empty() {
            return;
        }

        self.telemetry.clear_species_merge();

        let mut merge_count = 0u32;

        loop {
            if components.len() <= 1 {
                break;
            }

            let mut candidate: Option<(usize, u32)> = None;
            for (index, component) in components.iter().enumerate() {
                if component.bug_count >= minimum_share {
                    continue;
                }

                candidate = match candidate {
                    Some((best_index, best_count)) => {
                        if component.bug_count < best_count
                            || (component.bug_count == best_count && index < best_index)
                        {
                            Some((index, component.bug_count))
                        } else {
                            Some((best_index, best_count))
                        }
                    }
                    None => Some((index, component.bug_count)),
                };
            }

            let (from_index, from_count) = match candidate {
                Some(entry) => entry,
                None => break,
            };

            let from_component = &components[from_index];
            let mut nearest: Option<(usize, f32)> = None;

            for (index, component) in components.iter().enumerate() {
                if index == from_index {
                    continue;
                }

                let delta_hp =
                    (from_component.log_hp_multiplier - component.log_hp_multiplier) / sigma_hp;
                let delta_speed = (from_component.log_speed_multiplier
                    - component.log_speed_multiplier)
                    / sigma_speed;
                let mut distance_sq = delta_hp * delta_hp + delta_speed * delta_speed;
                if !distance_sq.is_finite() {
                    distance_sq = f32::INFINITY;
                }

                nearest = match nearest {
                    Some((best_index, best_distance)) => {
                        if distance_sq < best_distance
                            || (distance_sq == best_distance && index < best_index)
                        {
                            Some((index, distance_sq))
                        } else {
                            Some((best_index, best_distance))
                        }
                    }
                    None => Some((index, distance_sq)),
                };
            }

            let (to_index, distance_sq) =
                nearest.expect("at least one neighbour must be available for merging");
            let distance = distance_sq.sqrt();

            let to_count_before = components[to_index].bug_count;
            let new_count = to_count_before.saturating_add(from_count);
            components[to_index].bug_count = new_count;
            if total_bugs > 0 {
                let total = total_bugs as f32;
                components[to_index].dirichlet_share = new_count as f32 / total;
                components[to_index].fractional_count =
                    components[to_index].dirichlet_share * total;
            }

            let record = self.telemetry.push_species_merge();
            record.no_merge = false;
            record.from_component = from_index as u32;
            record.to_component = to_index as u32;
            record.from_count = from_count;
            record.to_count_before = to_count_before;
            record.to_count_after = new_count;
            record.log_distance = distance;

            let _ = components.remove(from_index);
            merge_count += 1;
        }

        self.work.provisional_species_count = components.len() as u32;
        debug_assert_eq!(
            components
                .iter()
                .map(|component| component.bug_count)
                .sum::<u32>(),
            total_bugs
        );

        if total_bugs > 0 {
            let total = total_bugs as f32;
            for component in components.iter_mut() {
                component.dirichlet_share = component.bug_count as f32 / total;
                component.fractional_count = component.dirichlet_share * total;
            }
        }

        if merge_count == 0 {
            self.telemetry.record_no_species_merge();
        }

        self.assign_species_tints();
    }

    fn assign_species_tints(&mut self) {
        if self.work.provisional_species.is_empty() {
            return;
        }

        let mut used = Vec::with_capacity(self.work.provisional_species.len());
        for index in 0..self.work.provisional_species.len() {
            let tint = self.draw_unique_tint(&mut used);
            self.work.provisional_species[index].tint = tint;
        }
    }

    fn draw_unique_tint(&mut self, used: &mut Vec<(u8, u8, u8)>) -> MacroquadColor {
        const MAX_ATTEMPTS: usize = 24;
        for _ in 0..MAX_ATTEMPTS {
            // RNG draws: species tint hue, saturation, and value in that order;
            // saturation/value ranges ensure readable contrast without ever
            // dipping below 0.55/0.85.
            let hue: f32 = self.rng.gen();
            let saturation: f32 = self.rng.gen_range(0.55..0.85);
            let value: f32 = self.rng.gen_range(0.85..0.98);
            let tint = hsv_to_color(hue, saturation, value);
            let quantized = quantize_color(tint);
            if !used.contains(&quantized) {
                used.push(quantized);
                return tint;
            }
        }

        let mut offset = 0usize;
        loop {
            let tint = fallback_tint(used.len() + offset);
            let quantized = quantize_color(tint);
            if !used.contains(&quantized) {
                used.push(quantized);
                return tint;
            }
            offset += 1;
        }
    }

    fn count_mean(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.count;
        let exponent = -tuning.slope * (difficulty - tuning.midpoint);
        tuning.minimum + (tuning.cap - tuning.minimum) / (1.0 + exponent.exp())
    }

    fn hp_mean_multiplier(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.hp;
        let delta = (difficulty - 1.0).max(0.0);
        // `soft_boost_fraction`/`soft_boost_rate` deliver the early additive
        // HP padding, while `post_pivot_growth` + `growth_pivot` set the
        // multiplicative ramp beyond the pivot.
        let soft_boost =
            tuning.soft_boost_fraction * (1.0 - (-tuning.soft_boost_rate * delta).exp());
        let multiplicative = tuning
            .post_pivot_growth
            .powf((difficulty - tuning.growth_pivot).max(0.0));
        (1.0 + soft_boost) * multiplicative
    }

    fn speed_mean_multiplier(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.speed;
        let delta = (difficulty - 1.0).max(0.0);
        // Mirrors the HP knobs: `soft_boost_fraction`/`soft_boost_rate` govern
        // the additive low-D bump while `post_pivot_growth` + `growth_pivot`
        // dictate the late-game exponential acceleration.
        let soft_boost =
            tuning.soft_boost_fraction * (1.0 - (-tuning.soft_boost_rate * delta).exp());
        let multiplicative = tuning
            .post_pivot_growth
            .powf((difficulty - tuning.growth_pivot).max(0.0));
        (1.0 + soft_boost) * multiplicative
    }

    fn cadence_mean_ms(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.cadence;
        // `cadence_base_ms` establishes the D=1 cadence and `cadence_slope_ms`
        // shifts it per difficulty before the min/max clamps apply.
        let raw = tuning.cadence_base_ms + tuning.cadence_slope_ms * (difficulty - 1.0);
        raw.clamp(tuning.cadence_min_ms as f32, tuning.cadence_max_ms as f32)
    }

    fn start_offset_mean_ms(&self, difficulty: f32) -> f32 {
        let tuning = &self.tuning.cadence;
        // `start_base_ms` + `start_slope_ms` set the linear offset trend and
        // `start_max_ms` enforces the hard cap.
        let raw = tuning.start_base_ms + tuning.start_slope_ms * (difficulty - 1.0);
        raw.clamp(0.0, tuning.start_max_ms as f32)
    }

    fn duration_target_ms(&self, difficulty: f32) -> u32 {
        let tuning = &self.tuning.cadence;
        // Compression triggers when the realised end time exceeds this linear
        // target derived from `duration_base_ms` + `duration_slope_ms` * (D-1).
        let raw = tuning.duration_base_ms + tuning.duration_slope_ms * (difficulty - 1.0);
        raw.max(1.0).round() as u32
    }
}

#[cfg(test)]
impl PressureV2 {
    fn difficulty_work(&self) -> &WaveDifficultyLatents {
        &self.work.difficulty
    }

    fn tuning(&self) -> &PressureTuning {
        &self.tuning
    }

    fn work_state(&self) -> &WaveWork {
        &self.work
    }

    fn provisional_components(&self) -> &[ComponentWork] {
        &self.work.provisional_species
    }

    fn provisional_species_count(&self) -> u32 {
        self.work.provisional_species_count
    }

    fn minimum_species_size(&self) -> u32 {
        self.work.minimum_species_size
    }

    fn align_pressure_for_test(&mut self) {
        self.align_pressure_with_eta();
    }

    fn wave_eta(&self) -> f32 {
        self.work.eta
    }

    fn eta_was_clamped(&self) -> bool {
        self.work.eta_clamped
    }

    fn pressure_after_eta(&self) -> f32 {
        self.work.pressure_after_eta
    }

    fn pressure_for_eta(&self, eta: f32) -> f32 {
        self.total_pressure_for_eta(eta)
    }

    fn enforce_minimum_share_for_test(&mut self) {
        self.enforce_minimum_share();
    }

    fn assign_species_tints_for_test(&mut self) {
        self.assign_species_tints();
    }

    fn sample_cadence_for_test(&mut self, inputs: &PressureWaveInputs) {
        self.sample_cadence_and_start_offsets(inputs);
    }

    fn enforce_duration_caps_for_test(&mut self, inputs: &PressureWaveInputs) {
        self.enforce_duration_caps(inputs);
    }

    fn write_spawn_records_for_test(&self, out: &mut Vec<PressureSpawnRecord>) {
        self.write_final_spawn_records(out);
    }

    fn component_cadence_ms(&self, index: usize) -> u32 {
        self.work.provisional_species[index].cadence_ms
    }

    fn component_start_offset_ms(&self, index: usize) -> u32 {
        self.work.provisional_species[index].start_offset_ms
    }

    fn component_spawn_times(&self, index: usize) -> &[u32] {
        &self.work.provisional_species[index].spawn_times
    }
}

const BASE_HP: f32 = 10.0;

fn draw_truncated_normal(
    rng: &mut ChaCha8Rng,
    mean: f32,
    deviation: f32,
    min: f32,
    max: f32,
) -> f32 {
    let z: f32 = rng.sample(StandardNormal);
    let value = mean + deviation * z;
    value.clamp(min, max)
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

fn quantize_color(color: MacroquadColor) -> (u8, u8, u8) {
    (
        quantize_channel(color.r),
        quantize_channel(color.g),
        quantize_channel(color.b),
    )
}

fn quantize_channel(value: f32) -> u8 {
    let clamped = value.clamp(0.0, 1.0);
    (clamped * 255.0).round() as u8
}

fn fallback_tint(index: usize) -> MacroquadColor {
    const GOLDEN_RATIO_CONJUGATE: f32 = 0.618_033_988_75;
    let base_hue = (index as f32 * GOLDEN_RATIO_CONJUGATE).fract();
    let saturation_cycle = match index % 5 {
        0 => 0.70,
        1 => 0.80,
        2 => 0.65,
        3 => 0.85,
        _ => 0.75,
    };
    let value_cycle = match index % 3 {
        0 => 0.92,
        1 => 0.88,
        _ => 0.95,
    };
    hsv_to_color(base_hue, saturation_cycle, value_cycle)
}

fn hsv_to_color(hue: f32, saturation: f32, value: f32) -> MacroquadColor {
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let h = hue.rem_euclid(1.0) * 6.0;

    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    MacroquadColor::new(r1 + m, g1 + m, b1 + m, 1.0)
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

    /// Drops any accumulated species merge telemetry.
    pub fn clear_species_merge(&mut self) {
        self.species_merge.clear();
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

    /// Records a no-merge outcome explicitly in the telemetry stream.
    pub fn record_no_species_merge(&mut self) {
        self.species_merge
            .push(SpeciesMergeTelemetry::no_merge_record());
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

#[derive(Clone, Debug, Default)]
struct WaveWork {
    difficulty: WaveDifficultyLatents,
    pressure_target: u32,
    hp_wave: f32,
    speed_wave: f32,
    per_bug_pressure: f32,
    provisional_species: Vec<ComponentWork>,
    provisional_species_count: u32,
    minimum_species_size: u32,
    eta: f32,
    eta_clamped: bool,
    pressure_after_eta: f32,
}

impl WaveWork {
    fn reset(&mut self) {
        self.difficulty = WaveDifficultyLatents::default();
        self.pressure_target = 0;
        self.hp_wave = 0.0;
        self.speed_wave = 0.0;
        self.per_bug_pressure = 0.0;
        self.provisional_species.clear();
        self.provisional_species_count = 0;
        self.minimum_species_size = 0;
        self.eta = 1.0;
        self.eta_clamped = false;
        self.pressure_after_eta = 0.0;
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct ComponentWork {
    hp_pre: f32,
    speed_pre: f32,
    pressure_weight_pre: f32,
    hp_post: f32,
    speed_post: f32,
    pressure_weight_post: f32,
    dirichlet_share: f32,
    fractional_count: f32,
    bug_count: u32,
    log_hp_multiplier: f32,
    log_speed_multiplier: f32,
    tint: MacroquadColor,
    cadence_ms: u32,
    start_offset_ms: u32,
    spawn_times: Vec<u32>,
}

impl ComponentWork {
    fn new(
        hp_pre: f32,
        speed_pre: f32,
        pressure_weight_pre: f32,
        log_hp_multiplier: f32,
        log_speed_multiplier: f32,
    ) -> Self {
        Self {
            hp_pre,
            speed_pre,
            pressure_weight_pre,
            hp_post: hp_pre,
            speed_post: speed_pre,
            pressure_weight_post: pressure_weight_pre,
            dirichlet_share: 0.0,
            fractional_count: 0.0,
            bug_count: 0,
            log_hp_multiplier,
            log_speed_multiplier,
            tint: MacroquadColor::new(1.0, 1.0, 1.0, 1.0),
            cadence_ms: 0,
            start_offset_ms: 0,
            spawn_times: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct WaveDifficultyLatents {
    count_mean: f32,
    bug_count: u32,
    hp_multiplier: f32,
    speed_multiplier: f32,
}

struct HpLatent {
    mean_multiplier: f32,
    multiplier: f32,
}

struct SpeedLatent {
    mean_multiplier: f32,
    multiplier: f32,
}

struct CountLatent {
    mean: f32,
    sampled: u32,
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
    /// Placeholder HP multiplier mean prior to sampling.
    pub hp_mean_multiplier: f32,
    /// Placeholder speed multiplier latent stored for upcoming implementations.
    pub speed_multiplier: f32,
    /// Placeholder speed multiplier mean prior to sampling.
    pub speed_mean_multiplier: f32,
    /// Placeholder absolute HP after applying the sampled multiplier.
    pub hp_absolute: f32,
    /// Placeholder speed multiplier after sampling (mirrors `speed_multiplier`).
    pub speed_absolute: f32,
    /// Placeholder per-bug pressure contribution derived from the latents.
    pub per_bug_pressure: f32,
    /// Placeholder total wave pressure target computed from the latents.
    pub pressure_target: u32,
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
    /// Flag indicating that the record represents an explicit no-merge outcome.
    no_merge: bool,
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
        record.no_merge = false;
        record
    }

    /// Creates an explicit no-merge telemetry record.
    #[must_use]
    fn no_merge_record() -> Self {
        let mut record = Self::merge_placeholder();
        record.no_merge = true;
        record.from_component = u32::MAX;
        record.to_component = u32::MAX;
        record.from_count = 0;
        record.to_count_before = 0;
        record.to_count_after = 0;
        record.log_distance = f32::INFINITY;
        record
    }

    /// Indicates whether the record represents an actual merge (as opposed to a placeholder).
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }

    /// Indicates whether the record represents an explicit no-merge outcome.
    #[must_use]
    pub fn is_no_merge(&self) -> bool {
        self.no_merge
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
    /// Maximum spawn time encountered prior to enforcing the duration cap.
    pub t_end_before: u32,
    /// Target deploy duration `T_target(D)` for the current difficulty.
    pub t_target: u32,
    /// Compression factor applied to cadences when the deploy duration exceeds the target.
    pub compression_factor: f32,
    /// Indicates whether any cadence was clamped to the `cad_min` floor during compression.
    pub hit_cadence_min: bool,
    /// Maximum spawn time after compression (or the original times when no compression occurs).
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
    use std::collections::HashSet;

    use rand::RngCore;

    fn build_component(
        weights: &PressureWeightTuning,
        hp_multiplier: f32,
        speed_multiplier: f32,
        bug_count: u32,
        total_bugs: u32,
    ) -> ComponentWork {
        let hp_pre = BASE_HP * hp_multiplier;
        let speed_pre = speed_multiplier;
        let pressure_weight = weights.alpha * hp_pre + weights.beta * speed_pre.powf(weights.gamma);
        let share = if total_bugs > 0 {
            bug_count as f32 / total_bugs as f32
        } else {
            0.0
        };

        ComponentWork {
            hp_pre,
            speed_pre,
            pressure_weight_pre: pressure_weight,
            hp_post: hp_pre,
            speed_post: speed_pre,
            pressure_weight_post: pressure_weight,
            dirichlet_share: share,
            fractional_count: share * total_bugs as f32,
            bug_count,
            log_hp_multiplier: hp_multiplier.ln(),
            log_speed_multiplier: speed_multiplier.ln(),
            tint: MacroquadColor::new(1.0, 1.0, 1.0, 1.0),
            cadence_ms: 0,
            start_offset_ms: 0,
            spawn_times: Vec::new(),
        }
    }

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
        assert!(!merge.is_no_merge());
        assert_eq!(telemetry.species_merge().len(), 2);
    }

    #[test]
    fn difficulty_latents_are_monotonic_with_difficulty() {
        let mut generator = PressureV2::default();
        let base_inputs = |difficulty: u32| {
            PressureWaveInputs::new(
                99,
                LevelId::new(1),
                WaveId::new(3),
                DifficultyLevel::new(difficulty),
            )
        };

        let low_inputs = base_inputs(1);
        generator.reseed_rng(&low_inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&low_inputs);
        let low = generator.difficulty_work().clone();
        let low_mean_count = generator.count_mean(low_inputs.difficulty().get() as f32);
        let low_mean_hp = generator.hp_mean_multiplier(low_inputs.difficulty().get() as f32);
        let low_mean_speed = generator.speed_mean_multiplier(low_inputs.difficulty().get() as f32);

        let high_inputs = base_inputs(9);
        generator.reseed_rng(&high_inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&high_inputs);
        let high = generator.difficulty_work().clone();
        let high_mean_count = generator.count_mean(high_inputs.difficulty().get() as f32);
        let high_mean_hp = generator.hp_mean_multiplier(high_inputs.difficulty().get() as f32);
        let high_mean_speed =
            generator.speed_mean_multiplier(high_inputs.difficulty().get() as f32);

        assert!(high_mean_count > low_mean_count);
        assert!(high_mean_hp > low_mean_hp);
        assert!(high_mean_speed > low_mean_speed);
        assert!(high.count_mean > low.count_mean);
    }

    #[test]
    fn difficulty_latents_populate_telemetry_and_respect_bounds() {
        let mut generator = PressureV2::default();
        let inputs =
            PressureWaveInputs::new(7, LevelId::new(2), WaveId::new(1), DifficultyLevel::new(4));

        generator.reseed_rng(&inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&inputs);

        let work = generator.difficulty_work();
        let work_state = generator.work_state();
        let telemetry = generator.telemetry.difficulty_latents();

        assert!(telemetry.is_recorded());
        assert_eq!(telemetry.bug_count_sampled, work.bug_count);
        assert_eq!(telemetry.bug_count_mean, work.count_mean);
        assert_eq!(telemetry.hp_multiplier, work.hp_multiplier);
        assert_eq!(telemetry.speed_multiplier, work.speed_multiplier);
        assert_eq!(
            telemetry.hp_mean_multiplier,
            generator.hp_mean_multiplier(inputs.difficulty().get() as f32)
        );
        assert_eq!(
            telemetry.speed_mean_multiplier,
            generator.speed_mean_multiplier(inputs.difficulty().get() as f32)
        );
        assert_eq!(telemetry.hp_absolute, work_state.hp_wave);
        assert_eq!(telemetry.speed_absolute, work_state.speed_wave);
        assert_eq!(telemetry.per_bug_pressure, work_state.per_bug_pressure);
        assert_eq!(telemetry.pressure_target, work_state.pressure_target);

        let tuning = generator.tuning();
        assert!(work.bug_count >= tuning.count.floor);
        assert!(work.hp_multiplier >= tuning.hp.min_multiplier);
        assert!(work.hp_multiplier <= tuning.hp.max_multiplier);
        assert!(work.speed_multiplier >= tuning.speed.min_multiplier);
        assert!(work.speed_multiplier <= tuning.speed.max_multiplier);
        assert!((work_state.hp_wave - work.hp_multiplier * BASE_HP).abs() < f32::EPSILON);
        assert!(work_state.pressure_target >= work.bug_count);
    }

    #[test]
    fn pressure_target_matches_expected_when_variance_zero() {
        let mut generator = PressureV2::default();
        generator.tuning_mut().count.deviation_ratio = 0.0;
        generator.tuning_mut().hp.deviation = 0.0;
        generator.tuning_mut().speed.deviation = 0.0;

        let inputs =
            PressureWaveInputs::new(13, LevelId::new(5), WaveId::new(2), DifficultyLevel::new(6));

        generator.reseed_rng(&inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&inputs);

        let work = generator.difficulty_work();
        let work_state = generator.work_state();
        let tuning = generator.tuning();
        let per_bug_pressure = tuning.pressure_weights.alpha * work_state.hp_wave
            + tuning.pressure_weights.beta
                * work_state.speed_wave.powf(tuning.pressure_weights.gamma);
        let expected = (work.bug_count as f32 * per_bug_pressure).round() as u32;
        assert_eq!(expected, work_state.pressure_target);
        assert_eq!(
            expected,
            generator.telemetry.difficulty_latents().pressure_target
        );
    }

    #[test]
    fn provisional_species_sampling_populates_work_state() {
        let mut generator = PressureV2::default();
        let inputs =
            PressureWaveInputs::new(17, LevelId::new(4), WaveId::new(2), DifficultyLevel::new(5));

        generator.reseed_rng(&inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&inputs);
        generator.sample_provisional_species(&inputs);

        let components = generator.provisional_components();
        assert!(!components.is_empty());

        let tuning = &generator.tuning().components;
        let hp_min = tuning.hp_multiplier_min * BASE_HP;
        let hp_max = tuning.hp_multiplier_max * BASE_HP;
        let speed_min = tuning.speed_multiplier_min;
        let speed_max = tuning.speed_multiplier_max;
        let bug_count = generator.difficulty_work().bug_count;
        let allocated: u32 = components.iter().map(|component| component.bug_count).sum();
        assert_eq!(allocated, bug_count);

        let share_sum: f32 = components
            .iter()
            .map(|component| component.dirichlet_share)
            .sum();
        assert!((share_sum - 1.0).abs() < 1e-3);

        for component in components {
            assert!(component.hp_pre >= hp_min);
            assert!(component.hp_pre <= hp_max);
            assert!(component.speed_pre >= speed_min);
            assert!(component.speed_pre <= speed_max);
            assert!(component.dirichlet_share.is_finite());
            assert!(component.fractional_count.is_finite());
            assert!(component.log_hp_multiplier.is_finite());
            assert!(component.log_speed_multiplier.is_finite());
            let hp_from_log = component.log_hp_multiplier.exp() * BASE_HP;
            let speed_from_log = component.log_speed_multiplier.exp();
            assert!((hp_from_log - component.hp_pre).abs() < 1e-3);
            assert!((speed_from_log - component.speed_pre).abs() < 1e-3);
        }
    }

    #[test]
    fn provisional_species_count_respects_caps() {
        let mut generator = PressureV2::default();
        {
            let tuning = generator.tuning_mut();
            tuning.components.poisson_intercept = 6.0;
            tuning.components.poisson_slope = 0.0;
            tuning.components.minimum_share = 0.4;
        }
        let poisson_cap = generator.tuning().components.poisson_cap;

        let inputs =
            PressureWaveInputs::new(23, LevelId::new(8), WaveId::new(3), DifficultyLevel::new(2));

        generator.reseed_rng(&inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&inputs);
        generator.sample_provisional_species(&inputs);

        let bug_count = generator.difficulty_work().bug_count;
        let min_share = generator.minimum_species_size();
        let count_cap = (bug_count / min_share).max(1);
        let provisional = generator.provisional_species_count();

        assert!(provisional >= 1);
        assert!(provisional <= poisson_cap);
        assert!(provisional <= count_cap);
    }

    #[test]
    fn provisional_species_sampling_is_deterministic() {
        let mut generator_a = PressureV2::default();
        let mut generator_b = PressureV2::default();
        let inputs =
            PressureWaveInputs::new(31, LevelId::new(5), WaveId::new(4), DifficultyLevel::new(7));

        generator_a.reseed_rng(&inputs);
        generator_a.work.reset();
        generator_a.compute_difficulty_latents(&inputs);
        generator_a.sample_provisional_species(&inputs);

        generator_b.reseed_rng(&inputs);
        generator_b.work.reset();
        generator_b.compute_difficulty_latents(&inputs);
        generator_b.sample_provisional_species(&inputs);

        let comps_a = generator_a.provisional_components();
        let comps_b = generator_b.provisional_components();

        assert_eq!(comps_a.len(), comps_b.len());
        for (a, b) in comps_a.iter().zip(comps_b.iter()) {
            assert!((a.hp_pre - b.hp_pre).abs() < f32::EPSILON);
            assert!((a.speed_pre - b.speed_pre).abs() < f32::EPSILON);
            assert!((a.pressure_weight_pre - b.pressure_weight_pre).abs() < f32::EPSILON);
            assert_eq!(a.bug_count, b.bug_count);
            assert!((a.dirichlet_share - b.dirichlet_share).abs() < f32::EPSILON);
            assert!((a.fractional_count - b.fractional_count).abs() < f32::EPSILON);
            assert!((a.log_hp_multiplier - b.log_hp_multiplier).abs() < f32::EPSILON);
            assert!((a.log_speed_multiplier - b.log_speed_multiplier).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn cadence_sampling_populates_spawn_times_within_bounds() {
        let mut generator = PressureV2::default();
        let inputs =
            PressureWaveInputs::new(37, LevelId::new(6), WaveId::new(5), DifficultyLevel::new(6));

        generator.reseed_rng(&inputs);
        generator.work.reset();
        generator.compute_difficulty_latents(&inputs);
        generator.sample_provisional_species(&inputs);
        generator.align_pressure_with_eta();
        generator.sample_cadence_for_test(&inputs);

        let tuning = &generator.tuning().cadence;
        for (index, component) in generator.provisional_components().iter().enumerate() {
            let cadence = generator.component_cadence_ms(index);
            let start = generator.component_start_offset_ms(index);
            let times = generator.component_spawn_times(index);

            assert!(cadence >= tuning.cadence_min_ms);
            assert!(cadence <= tuning.cadence_max_ms);
            assert!(start <= tuning.start_max_ms);
            assert_eq!(times.len(), component.bug_count as usize);
            if let Some((&first, rest)) = times.split_first() {
                assert_eq!(first, start);
                for (step, window) in rest.iter().enumerate() {
                    let expected = start.saturating_add((step as u32 + 1).saturating_mul(cadence));
                    assert_eq!(*window, expected);
                }
            }
        }
    }

    #[test]
    fn cadence_sampling_is_deterministic() {
        let mut generator_a = PressureV2::default();
        let mut generator_b = PressureV2::default();
        let inputs =
            PressureWaveInputs::new(41, LevelId::new(3), WaveId::new(2), DifficultyLevel::new(4));

        for generator in [&mut generator_a, &mut generator_b] {
            generator.reseed_rng(&inputs);
            generator.work.reset();
            generator.compute_difficulty_latents(&inputs);
            generator.sample_provisional_species(&inputs);
            generator.align_pressure_with_eta();
            generator.sample_cadence_for_test(&inputs);
        }

        let comps_a = generator_a.provisional_components();
        let comps_b = generator_b.provisional_components();
        assert_eq!(comps_a.len(), comps_b.len());
        for index in 0..comps_a.len() {
            assert_eq!(
                generator_a.component_cadence_ms(index),
                generator_b.component_cadence_ms(index)
            );
            assert_eq!(
                generator_a.component_start_offset_ms(index),
                generator_b.component_start_offset_ms(index)
            );
            assert_eq!(
                generator_a.component_spawn_times(index),
                generator_b.component_spawn_times(index)
            );
        }
    }

    #[test]
    fn enforce_minimum_share_merges_until_threshold() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 10;
        generator.work.difficulty.bug_count = total_bugs;
        generator.work.minimum_species_size = 4;

        let weights = &generator.tuning().pressure_weights;
        let components = vec![
            build_component(weights, 1.0, 1.0, 2, total_bugs),
            build_component(weights, 1.5, 0.8, 4, total_bugs),
            build_component(weights, 0.8, 1.2, 4, total_bugs),
        ];
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 3;

        generator.enforce_minimum_share_for_test();

        let merged = generator.provisional_components();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.iter().map(|c| c.bug_count).sum::<u32>(), total_bugs);
        let minimum = generator.minimum_species_size();
        assert!(
            merged.len() == 1
                || merged
                    .iter()
                    .all(|component| component.bug_count >= minimum)
        );

        let telemetry = generator.telemetry().species_merge();
        assert_eq!(telemetry.len(), 1);
        let record = &telemetry[0];
        assert!(record.is_recorded());
        assert!(!record.is_no_merge());
        assert_eq!(record.from_count, 2);
        assert_eq!(record.to_count_before, 4);
        assert_eq!(record.to_count_after, 6);
    }

    #[test]
    fn enforce_minimum_share_records_no_merge_event() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 12;
        generator.work.difficulty.bug_count = total_bugs;
        generator.work.minimum_species_size = 3;

        let weights = &generator.tuning().pressure_weights;
        let components = vec![
            build_component(weights, 1.0, 1.0, 4, total_bugs),
            build_component(weights, 1.3, 0.9, 4, total_bugs),
            build_component(weights, 0.9, 1.1, 4, total_bugs),
        ];
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 3;

        generator.enforce_minimum_share_for_test();

        let merged = generator.provisional_components();
        assert_eq!(merged.len(), 3);
        assert!(merged
            .iter()
            .all(|component| component.bug_count >= generator.minimum_species_size()));

        let telemetry = generator.telemetry().species_merge();
        assert_eq!(telemetry.len(), 1);
        let record = &telemetry[0];
        assert!(record.is_recorded());
        assert!(record.is_no_merge());
        assert_eq!(record.from_component, u32::MAX);
        assert_eq!(record.to_component, u32::MAX);
        assert_eq!(record.from_count, 0);
        assert_eq!(record.to_count_after, 0);
    }

    #[test]
    fn species_tints_are_unique_after_assignment() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 32;
        generator.work.difficulty.bug_count = total_bugs;
        generator.work.minimum_species_size = 4;

        let weights = generator.tuning().pressure_weights.clone();
        let components = vec![
            build_component(&weights, 1.0, 1.0, 8, total_bugs),
            build_component(&weights, 1.2, 0.9, 8, total_bugs),
            build_component(&weights, 0.9, 1.1, 8, total_bugs),
            build_component(&weights, 1.3, 1.2, 8, total_bugs),
        ];
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 4;

        generator.assign_species_tints_for_test();

        let mut seen = HashSet::new();
        for component in generator.provisional_components() {
            let quantized = quantize_color(component.tint);
            assert!(seen.insert(quantized));
        }
    }

    #[test]
    fn species_tints_are_deterministic() {
        let mut generator_a = PressureV2::default();
        let mut generator_b = PressureV2::default();

        generator_a.telemetry.reset();
        generator_b.telemetry.reset();
        generator_a.work.reset();
        generator_b.work.reset();

        let total_bugs = 24;
        generator_a.work.difficulty.bug_count = total_bugs;
        generator_b.work.difficulty.bug_count = total_bugs;
        generator_a.work.minimum_species_size = 3;
        generator_b.work.minimum_species_size = 3;

        let weights = generator_a.tuning().pressure_weights.clone();
        let components = vec![
            build_component(&weights, 1.0, 1.0, 8, total_bugs),
            build_component(&weights, 1.2, 0.9, 8, total_bugs),
            build_component(&weights, 0.9, 1.1, 8, total_bugs),
        ];

        generator_a.work.provisional_species = components.clone();
        generator_b.work.provisional_species = components;
        generator_a.work.provisional_species_count = 3;
        generator_b.work.provisional_species_count = 3;

        generator_a.assign_species_tints_for_test();
        generator_b.assign_species_tints_for_test();

        let tints_a: Vec<_> = generator_a
            .provisional_components()
            .iter()
            .map(|component| quantize_color(component.tint))
            .collect();
        let tints_b: Vec<_> = generator_b
            .provisional_components()
            .iter()
            .map(|component| quantize_color(component.tint))
            .collect();

        assert_eq!(tints_a, tints_b);
    }

    #[test]
    fn eta_scaling_aligns_pressure_when_target_inside_bounds() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 30;
        generator.work.difficulty.bug_count = total_bugs;
        let weights = generator.tuning().pressure_weights.clone();
        let components = vec![
            build_component(&weights, 1.0, 1.0, 10, total_bugs),
            build_component(&weights, 1.2, 0.9, 8, total_bugs),
            build_component(&weights, 0.9, 1.4, 12, total_bugs),
        ];
        let mut pressure_sum = 0.0;
        for component in &components {
            let per_bug = weights.alpha * component.hp_pre
                + weights.beta * component.speed_pre.powf(weights.gamma);
            pressure_sum += component.bug_count as f32 * per_bug;
        }
        generator.work.pressure_target = pressure_sum.round() as u32;
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 3;

        generator.align_pressure_for_test();

        let eta = generator.wave_eta();
        assert!(eta > ETA_MIN);
        assert!(eta < ETA_MAX);
        let realised = generator.pressure_after_eta();
        let target = generator.work_state().pressure_target as f32;
        assert!((realised - target).abs() <= 1.0);

        let telemetry = generator.telemetry().eta_scaling();
        assert!(telemetry.is_recorded());
        assert!(!telemetry.eta_clamped);
        assert!((telemetry.eta_final - eta).abs() < f32::EPSILON);
        assert!((telemetry.pressure_after_eta - realised).abs() < f32::EPSILON);
    }

    #[test]
    fn eta_scaling_clamps_and_records_when_target_too_high() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 18;
        generator.work.difficulty.bug_count = total_bugs;
        let weights = generator.tuning().pressure_weights.clone();
        let components = vec![
            build_component(&weights, 1.0, 1.0, 9, total_bugs),
            build_component(&weights, 1.1, 1.1, 9, total_bugs),
        ];
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 2;
        let max_pressure = generator.pressure_for_eta(ETA_MAX);
        generator.work.pressure_target = (max_pressure * 2.0).round() as u32;

        generator.align_pressure_for_test();

        assert!(generator.eta_was_clamped());
        assert!((generator.wave_eta() - ETA_MAX).abs() < f32::EPSILON);
        assert!(
            generator.pressure_after_eta() <= generator.work_state().pressure_target as f32 + 1.0
        );

        let telemetry = generator.telemetry().eta_scaling();
        assert!(telemetry.is_recorded());
        assert!(telemetry.eta_clamped);
        assert!((telemetry.eta_final - ETA_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn eta_scaling_clamps_and_records_when_target_too_low() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let total_bugs = 20;
        generator.work.difficulty.bug_count = total_bugs;
        let weights = generator.tuning().pressure_weights.clone();
        let components = vec![
            build_component(&weights, 1.3, 1.0, 10, total_bugs),
            build_component(&weights, 1.1, 0.9, 10, total_bugs),
        ];
        generator.work.provisional_species = components;
        generator.work.provisional_species_count = 2;
        let min_pressure = generator.pressure_for_eta(ETA_MIN);
        generator.work.pressure_target = (min_pressure * 0.25).round() as u32;

        generator.align_pressure_for_test();

        assert!(generator.eta_was_clamped());
        assert!((generator.wave_eta() - ETA_MIN).abs() < f32::EPSILON);
        assert!(
            generator.pressure_after_eta() >= generator.work_state().pressure_target as f32 - 1.0
        );

        let telemetry = generator.telemetry().eta_scaling();
        assert!(telemetry.is_recorded());
        assert!(telemetry.eta_clamped);
        assert!((telemetry.eta_final - ETA_MIN).abs() < f32::EPSILON);
    }

    #[test]
    fn duration_caps_skip_compression_when_wave_within_target() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let weights = generator.tuning().pressure_weights.clone();
        let total_bugs = 4;
        let mut component = build_component(&weights, 1.0, 1.0, total_bugs, total_bugs);
        component.cadence_ms = 500;
        component.start_offset_ms = 100;
        component.spawn_times = (0..component.bug_count)
            .map(|idx| {
                component
                    .start_offset_ms
                    .saturating_add(component.cadence_ms.saturating_mul(idx))
            })
            .collect();

        generator.work.provisional_species = vec![component];
        generator.work.provisional_species_count = 1;
        generator.work.difficulty.bug_count = total_bugs;

        let inputs =
            PressureWaveInputs::new(11, LevelId::new(1), WaveId::new(1), DifficultyLevel::new(1));
        generator.enforce_duration_caps_for_test(&inputs);

        let component = &generator.work.provisional_species[0];
        let expected_times = vec![100, 600, 1_100, 1_600];
        assert_eq!(component.spawn_times, expected_times);
        assert_eq!(component.cadence_ms, 500);

        let telemetry = generator.telemetry().cadence_compression();
        assert!(telemetry.is_recorded());
        assert_eq!(telemetry.t_end_before, 1_600);
        assert_eq!(telemetry.t_end_after, 1_600);
        assert_eq!(telemetry.t_target, 60_000);
        assert!((telemetry.compression_factor - 1.0).abs() < f32::EPSILON);
        assert!(!telemetry.hit_cadence_min);
    }

    #[test]
    fn duration_caps_compresses_cadence_when_over_target() {
        let mut generator = PressureV2::default();
        {
            let tuning = generator.tuning_mut();
            tuning.cadence.duration_base_ms = 1_000.0;
            tuning.cadence.duration_slope_ms = 0.0;
        }
        generator.telemetry.reset();
        generator.work.reset();

        let weights = generator.tuning().pressure_weights.clone();
        let total_bugs = 5;
        let mut component = build_component(&weights, 1.0, 1.0, total_bugs, total_bugs);
        component.cadence_ms = 300;
        component.start_offset_ms = 0;
        component.spawn_times = (0..component.bug_count)
            .map(|idx| component.cadence_ms.saturating_mul(idx))
            .collect();

        generator.work.provisional_species = vec![component];
        generator.work.provisional_species_count = 1;
        generator.work.difficulty.bug_count = total_bugs;

        let inputs =
            PressureWaveInputs::new(7, LevelId::new(2), WaveId::new(3), DifficultyLevel::new(1));
        generator.enforce_duration_caps_for_test(&inputs);

        let component = &generator.work.provisional_species[0];
        let expected_times = vec![0, 250, 500, 750, 1_000];
        assert_eq!(component.spawn_times, expected_times);
        assert_eq!(component.cadence_ms, 250);

        let telemetry = generator.telemetry().cadence_compression();
        assert!(telemetry.is_recorded());
        assert_eq!(telemetry.t_end_before, 1_200);
        assert_eq!(telemetry.t_end_after, 1_000);
        assert_eq!(telemetry.t_target, 1_000);
        assert!((telemetry.compression_factor - 1.2).abs() < 1e-3);
        assert!(!telemetry.hit_cadence_min);
    }

    #[test]
    fn duration_caps_hits_cadence_min_when_target_too_low() {
        let mut generator = PressureV2::default();
        {
            let tuning = generator.tuning_mut();
            tuning.cadence.duration_base_ms = 100.0;
            tuning.cadence.duration_slope_ms = 0.0;
        }
        generator.telemetry.reset();
        generator.work.reset();

        let weights = generator.tuning().pressure_weights.clone();
        let total_bugs = 3;
        let mut component = build_component(&weights, 1.0, 1.0, total_bugs, total_bugs);
        component.cadence_ms = 150;
        component.start_offset_ms = 0;
        component.spawn_times = (0..component.bug_count)
            .map(|idx| component.cadence_ms.saturating_mul(idx))
            .collect();

        generator.work.provisional_species = vec![component];
        generator.work.provisional_species_count = 1;
        generator.work.difficulty.bug_count = total_bugs;

        let inputs =
            PressureWaveInputs::new(9, LevelId::new(5), WaveId::new(1), DifficultyLevel::new(1));
        generator.enforce_duration_caps_for_test(&inputs);

        let component = &generator.work.provisional_species[0];
        let expected_times = vec![0, 120, 240];
        assert_eq!(component.spawn_times, expected_times);
        assert_eq!(
            component.cadence_ms,
            generator.tuning().cadence.cadence_min_ms
        );

        let telemetry = generator.telemetry().cadence_compression();
        assert!(telemetry.is_recorded());
        assert_eq!(telemetry.t_end_before, 300);
        assert_eq!(telemetry.t_target, 100);
        assert_eq!(telemetry.t_end_after, 240);
        assert!((telemetry.compression_factor - 3.0).abs() < 1e-3);
        assert!(telemetry.hit_cadence_min);
        assert!(telemetry.t_end_after > telemetry.t_target);
    }

    #[test]
    fn spawn_records_are_sorted_and_hp_rounded() {
        let mut generator = PressureV2::default();
        generator.telemetry.reset();
        generator.work.reset();

        let weights = generator.tuning().pressure_weights.clone();
        let mut component_a = build_component(&weights, 1.0, 1.0, 2, 4);
        component_a.hp_post = 17.6;
        component_a.speed_post = 1.25;
        component_a.spawn_times = vec![400, 800];
        component_a.cadence_ms = 400;
        component_a.start_offset_ms = 0;

        let mut component_b = build_component(&weights, 1.0, 1.0, 2, 4);
        component_b.hp_post = 19.2;
        component_b.speed_post = 0.9;
        component_b.spawn_times = vec![400, 600];
        component_b.cadence_ms = 200;
        component_b.start_offset_ms = 0;

        generator.work.provisional_species = vec![component_a, component_b];
        generator.work.provisional_species_count = 2;

        let mut spawns = Vec::new();
        generator.write_spawn_records_for_test(&mut spawns);

        let times: Vec<u32> = spawns.iter().map(|spawn| spawn.time_ms()).collect();
        assert_eq!(times, vec![400, 400, 600, 800]);

        let species: Vec<u32> = spawns.iter().map(|spawn| spawn.species_id()).collect();
        assert_eq!(species, vec![0, 1, 1, 0]);

        let hp: Vec<u32> = spawns.iter().map(|spawn| spawn.hp()).collect();
        assert_eq!(hp, vec![18, 19, 19, 18]);

        let speeds: Vec<f32> = spawns.iter().map(|spawn| spawn.speed_mult()).collect();
        assert!((speeds[0] - 1.25).abs() < f32::EPSILON);
        assert!((speeds[1] - 0.9).abs() < f32::EPSILON);
        assert!((speeds[2] - 0.9).abs() < f32::EPSILON);
        assert!((speeds[3] - 1.25).abs() < f32::EPSILON);
    }
}
