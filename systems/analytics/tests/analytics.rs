use std::time::Duration;

use maze_defence_core::{CellCoord, Command, Event, StatsReport};
use maze_defence_system_analytics::{Analytics, AnalyticsScratch};

fn sample_report(seed: u32) -> StatsReport {
    StatsReport::new(seed, seed + 1, seed + 2, seed + 3, seed + 4)
}

#[test]
fn layout_change_recomputes_without_tick() {
    let mut analytics = Analytics::new();
    let mut emitted = Vec::new();
    let mut recompute_calls = 0;

    analytics.handle(
        &[Event::MazeLayoutChanged],
        &[],
        |_scratch: &mut AnalyticsScratch<'_>| {
            recompute_calls += 1;
            Some(sample_report(10))
        },
        &mut emitted,
    );

    assert_eq!(
        recompute_calls, 1,
        "layout change should recompute immediately"
    );
    assert_eq!(emitted.len(), 1, "analytics update must be published");

    let report = match &emitted[0] {
        Event::AnalyticsUpdated { report } => report.clone(),
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(report, sample_report(10));
    assert_eq!(analytics.last_report(), Some(&sample_report(10)));
}

#[test]
fn manual_refresh_coalesces_duplicates() {
    let mut analytics = Analytics::new();
    let mut emitted = Vec::new();
    let mut recompute_calls = 0;

    analytics.handle(
        &[],
        &[
            Command::RequestAnalyticsRefresh,
            Command::RequestAnalyticsRefresh,
        ],
        |_scratch: &mut AnalyticsScratch<'_>| {
            recompute_calls += 1;
            Some(sample_report(40))
        },
        &mut emitted,
    );

    assert_eq!(recompute_calls, 1, "manual refresh should trigger once");
    assert_eq!(emitted.len(), 1);
    assert_eq!(analytics.last_report(), Some(&sample_report(40)));
}

#[test]
fn layout_and_manual_requests_coalesce_per_call() {
    let mut analytics = Analytics::new();
    let mut emitted = Vec::new();
    let mut recompute_calls = 0;

    analytics.handle(
        &[Event::MazeLayoutChanged, Event::MazeLayoutChanged],
        &[Command::RequestAnalyticsRefresh],
        |scratch: &mut AnalyticsScratch<'_>| {
            recompute_calls += 1;
            // Demonstrate scratch reuse by writing to the buffers.
            scratch.path().push(CellCoord::new(0, 0));
            scratch.frontier().clear();
            Some(sample_report(60))
        },
        &mut emitted,
    );

    assert_eq!(
        recompute_calls, 1,
        "multiple triggers must coalesce per call"
    );
    assert_eq!(emitted.len(), 1);
    assert_eq!(analytics.last_report(), Some(&sample_report(60)));

    emitted.clear();

    analytics.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_millis(8),
        }],
        &[],
        |_scratch: &mut AnalyticsScratch<'_>| {
            recompute_calls += 1;
            Some(sample_report(80))
        },
        &mut emitted,
    );

    assert_eq!(recompute_calls, 1, "no recompute when queue is empty");
    assert!(emitted.is_empty());
    assert_eq!(analytics.last_report(), Some(&sample_report(60)));
}
