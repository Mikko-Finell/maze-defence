#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic pressure v2 wave generation system stub.

use maze_defence_core::{PressureSpawnRecord, PressureWaveInputs};

/// Stub implementation of the pressure v2 generator.
#[derive(Debug, Default)]
pub struct PressureV2;

impl PressureV2 {
    /// Generates v2 pressure spawns according to the provided inputs.
    pub fn generate(&mut self, _inputs: &PressureWaveInputs, out: &mut Vec<PressureSpawnRecord>) {
        out.clear();
        todo!("pressure v2 generation not implemented");
    }
}
