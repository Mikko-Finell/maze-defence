//! Immutable analytics snapshots sourced from the authoritative world state.

use crate::World;
use maze_defence_core::{AnalyticsInputs, AnalyticsLayoutSnapshot, CellCoord, TowerAnalyticsView};

#[cfg(any(test, feature = "tower_scaffolding"))]
use maze_defence_core::TowerAnalyticsSnapshot;

/// Captures the full set of analytics inputs required for recomputation.
pub(crate) fn snapshot(world: &World) -> AnalyticsInputs {
    let layout = layout_snapshot(world);
    let towers = tower_view(world);
    AnalyticsInputs::new(layout, towers)
}

/// Snapshots spawner and target coordinates without mutating the world state.
pub(crate) fn layout_snapshot(world: &World) -> AnalyticsLayoutSnapshot {
    let spawners: Vec<CellCoord> = world.bug_spawners.iter().collect();
    let targets = world.targets.clone();
    AnalyticsLayoutSnapshot::new(spawners, targets)
}

/// Captures deterministic tower metrics for analytics consumers.
pub(crate) fn tower_view(world: &World) -> TowerAnalyticsView {
    gather_towers(world)
}

#[cfg(any(test, feature = "tower_scaffolding"))]
fn gather_towers(world: &World) -> TowerAnalyticsView {
    if world.towers.is_empty() {
        return TowerAnalyticsView::default();
    }

    let cells_per_tile = world.cells_per_tile.max(1);
    let snapshots: Vec<TowerAnalyticsSnapshot> = world
        .towers
        .iter()
        .map(|tower| TowerAnalyticsSnapshot {
            tower: tower.id,
            kind: tower.kind,
            region: tower.region,
            range_cells: tower.kind.range_in_cells(cells_per_tile),
            damage_per_second: compute_tower_dps(tower.kind),
        })
        .collect();

    TowerAnalyticsView::from_snapshots(snapshots)
}

#[cfg(not(any(test, feature = "tower_scaffolding")))]
fn gather_towers(_world: &World) -> TowerAnalyticsView {
    TowerAnalyticsView::default()
}

#[cfg(any(test, feature = "tower_scaffolding"))]
fn compute_tower_dps(kind: maze_defence_core::TowerKind) -> u32 {
    let damage = u64::from(kind.projectile_damage().get());
    let cooldown_ms = u64::from(kind.fire_cooldown_ms().max(1));
    let per_second = damage.saturating_mul(1_000).saturating_div(cooldown_ms);
    per_second.min(u64::from(u32::MAX)) as u32
}
