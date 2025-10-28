use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use maze_defence_core::{CellCoord, Command, Event, PlayMode, StatsReport, TileCoord, TowerKind};
use maze_defence_system_analytics::{
    select_shortest_navigation_path, total_tower_dps, tower_count, tower_coverage_mean_bps,
    tower_firing_completion_percent_bps, Analytics, AnalyticsScratch,
};
use maze_defence_world::{self as world, query, World};

#[test]
fn analytics_events_are_deterministic_for_build_sequence() {
    let script = build_sequence();
    let first = replay(script.clone());
    let second = replay(script);

    assert_eq!(first, second, "analytics replay diverged");
    assert_eq!(
        first.reports.len(),
        1,
        "expected exactly one analytics report after recompute",
    );

    let fingerprint = first.fingerprint();
    assert_eq!(
        fingerprint, 0x2a1d_e273_2c62_36da,
        "fingerprint mismatch: {fingerprint:#x}",
    );
}

fn replay(commands: Vec<Command>) -> ReplayOutcome {
    let mut world = World::new();
    let mut analytics = Analytics::new();
    let mut reports = Vec::new();

    for command in commands {
        let mut generated = Vec::new();
        world::apply(&mut world, command.clone(), &mut generated);

        let mut analytics_events = Vec::new();
        analytics.handle(
            &generated,
            std::slice::from_ref(&command),
            |scratch: &mut AnalyticsScratch<'_>| recompute_report(&world, scratch),
            &mut analytics_events,
        );

        for event in analytics_events {
            if let Event::AnalyticsUpdated { report } = event {
                reports.push(report);
            }
        }
    }

    ReplayOutcome { reports }
}

fn recompute_report(world: &World, scratch: &mut AnalyticsScratch<'_>) -> Option<StatsReport> {
    let navigation = query::navigation_field(world);
    let inputs = query::analytics_inputs(world);
    let (layout, towers) = inputs.into_parts();

    let path = select_shortest_navigation_path(&navigation, &layout, scratch);
    let (coverage_bps, firing_bps, path_length) = if let Some(path) = path {
        (
            tower_coverage_mean_bps(path, &towers),
            tower_firing_completion_percent_bps(path, &towers),
            u32::try_from(path.len()).unwrap_or(u32::MAX),
        )
    } else {
        (0, 0, 0)
    };

    let tower_count = tower_count(&towers);
    let total_dps = total_tower_dps(&towers);

    Some(StatsReport::new(
        coverage_bps,
        firing_bps,
        path_length,
        tower_count,
        total_dps,
    ))
}

fn build_sequence() -> Vec<Command> {
    vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(6),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 2,
        },
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(1, 1),
        },
        Command::RequestAnalyticsRefresh,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        Command::Tick {
            dt: Duration::from_millis(16),
        },
        Command::Tick {
            dt: Duration::from_millis(16),
        },
    ]
}

#[derive(Debug, PartialEq, Eq)]
struct ReplayOutcome {
    reports: Vec<StatsReport>,
}

impl ReplayOutcome {
    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.reports.len().hash(&mut hasher);
        for report in &self.reports {
            report.tower_coverage_mean_bps().hash(&mut hasher);
            report.firing_complete_percent_bps().hash(&mut hasher);
            report.shortest_path_length_cells().hash(&mut hasher);
            report.tower_count().hash(&mut hasher);
            report.total_tower_dps().hash(&mut hasher);
        }
        hasher.finish()
    }
}
