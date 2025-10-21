#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic spawning system responsible for emitting bug spawn commands.

use std::time::Duration;

use maze_defence_core::{BugColor, CellCoord, Command, Event, PlayMode};

const RNG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const RNG_INCREMENT: u64 = 1;
const SPAWN_COLORS: [BugColor; 4] = [
    BugColor::from_rgb(0x2f, 0x95, 0x32),
    BugColor::from_rgb(0xc8, 0x2a, 0x36),
    BugColor::from_rgb(0xff, 0xc1, 0x07),
    BugColor::from_rgb(0x58, 0x47, 0xff),
];

/// Configuration parameters required to construct the spawning system.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    spawn_interval: Duration,
    rng_seed: u64,
}

impl Config {
    /// Creates a new configuration using the provided spawn cadence and seed.
    #[must_use]
    pub const fn new(spawn_interval: Duration, rng_seed: u64) -> Self {
        Self {
            spawn_interval,
            rng_seed,
        }
    }
}

/// Pure system that deterministically emits spawn commands in attack mode.
#[derive(Debug)]
pub struct Spawning {
    spawn_interval: Duration,
    accumulator: Duration,
    rng_state: u64,
    color_index: usize,
}

impl Spawning {
    /// Creates a new spawning system using the supplied configuration.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            spawn_interval: config.spawn_interval,
            accumulator: Duration::ZERO,
            rng_state: config.rng_seed,
            color_index: 0,
        }
    }

    /// Consumes events and immutable views to emit spawn commands.
    pub fn handle(
        &mut self,
        events: &[Event],
        play_mode: PlayMode,
        spawners: &[CellCoord],
        out: &mut Vec<Command>,
    ) {
        if play_mode != PlayMode::Attack {
            self.accumulator = Duration::ZERO;
            return;
        }

        if self.spawn_interval.is_zero() || spawners.is_empty() {
            return;
        }

        let mut accumulated = Duration::ZERO;
        for event in events {
            if let Event::TimeAdvanced { dt } = event {
                accumulated = accumulated.saturating_add(*dt);
            }
        }

        if accumulated.is_zero() {
            return;
        }

        self.accumulator = self.accumulator.saturating_add(accumulated);
        let spawn_attempts = self.resolve_spawn_attempts();

        for _ in 0..spawn_attempts {
            let spawner = self.select_spawner(spawners);
            let color = self.next_color();
            out.push(Command::SpawnBug { spawner, color });
        }
    }

    fn resolve_spawn_attempts(&mut self) -> usize {
        if self.spawn_interval.is_zero() {
            return 0;
        }

        let mut attempts = 0;
        while self.accumulator >= self.spawn_interval {
            self.accumulator -= self.spawn_interval;
            attempts += 1;
        }
        attempts
    }

    fn select_spawner(&mut self, spawners: &[CellCoord]) -> CellCoord {
        debug_assert!(!spawners.is_empty(), "select_spawner requires spawners");
        let value = self.advance_rng();
        let index = (value % spawners.len() as u64) as usize;
        spawners[index]
    }

    fn advance_rng(&mut self) -> u64 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(RNG_MULTIPLIER)
            .wrapping_add(RNG_INCREMENT);
        self.rng_state
    }

    fn next_color(&mut self) -> BugColor {
        let color = SPAWN_COLORS[self.color_index % SPAWN_COLORS.len()];
        self.color_index = (self.color_index + 1) % SPAWN_COLORS.len();
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_spawn_attempts_without_interval() {
        let mut spawning = Spawning::new(Config::new(Duration::ZERO, 1));
        spawning.accumulator = Duration::from_secs(10);
        assert_eq!(spawning.resolve_spawn_attempts(), 0);
    }
}
