use std::collections::VecDeque;

use maze_defence_core::{
    AnalyticsLayoutSnapshot, CellCoord, NavigationFieldView, TowerAnalyticsSnapshot,
    TowerAnalyticsView,
};

use crate::AnalyticsScratch;

fn cell_center(cell: &CellCoord) -> (i64, i64) {
    let column = i64::from(cell.column()) * 2 + 1;
    let row = i64::from(cell.row()) * 2 + 1;
    (column, row)
}

/// Selects the canonical shortest path from the provided spawners to the maze exits.
///
/// The navigation field already stores monotonically decreasing distances seeded
/// from the targets. By following this gradient from each spawner we can recover
/// the concrete route without re-running breadth-first search. The shortest path
/// is cached inside the scratch path buffer so subsequent metric passes can reuse
/// the coordinates without recomputing them.
pub fn select_shortest_navigation_path<'a>(
    navigation: &NavigationFieldView<'_>,
    layout: &AnalyticsLayoutSnapshot,
    scratch: &'a mut AnalyticsScratch<'_>,
) -> Option<&'a [CellCoord]> {
    let (path_buffer, working) = scratch.buffers();
    path_buffer.clear();
    working.clear();

    let mut best_len: Option<usize> = None;

    for &spawner in layout.spawners() {
        working.clear();

        if trace_path(spawner, navigation, working).is_none() {
            continue;
        }

        let candidate_len = working.len();
        let replace = match best_len {
            None => true,
            Some(best) => candidate_len < best,
        };

        if replace {
            path_buffer.clear();
            path_buffer.extend(working.iter().copied());
            best_len = Some(candidate_len);
        }
    }

    if best_len.is_some() {
        Some(path_buffer.as_slice())
    } else {
        path_buffer.clear();
        None
    }
}

/// Computes the mean tower coverage along the provided path expressed in basis points.
///
/// Each cell contributes the ratio of towers whose range covers that cell divided by the
/// total number of towers. The accumulated ratios are averaged across the entire path and
/// scaled to basis points (`1/100` of a percent). An empty tower list or path returns
/// zero coverage.
#[must_use]
pub fn tower_coverage_mean_bps(path: &[CellCoord], towers: &TowerAnalyticsView) -> u32 {
    let total_towers = towers.iter().count() as u32;

    if total_towers == 0 || path.is_empty() {
        return 0;
    }

    let cached: Vec<_> = towers
        .iter()
        .filter_map(TowerRangeCache::from_snapshot)
        .collect();

    let mut covered_sum: u128 = 0;

    for cell in path {
        let (cell_center_column, cell_center_row) = cell_center(cell);
        let mut towers_in_range = 0_u32;

        for tower in &cached {
            if tower.contains_cell(cell_center_column, cell_center_row) {
                towers_in_range = towers_in_range.saturating_add(1);
            }
        }

        covered_sum = covered_sum.saturating_add(u128::from(towers_in_range));
    }

    let denominator = u128::from(path.len() as u64) * u128::from(total_towers);

    if denominator == 0 {
        return 0;
    }

    let numerator = covered_sum.saturating_mul(10_000);
    (numerator / denominator) as u32
}

/// Returns the path percentage (in basis points) completed when every tower has a firing
/// opportunity.
///
/// The path is traversed in order, recording the first cell index each tower can reach. The
/// furthest first opportunity determines when the entire defence is online. If any tower never
/// gains line-of-sight this saturates at 100% (`10_000` basis points).
#[must_use]
pub fn tower_firing_completion_percent_bps(path: &[CellCoord], towers: &TowerAnalyticsView) -> u32 {
    if towers.iter().count() == 0 || path.is_empty() {
        return 0;
    }

    let mut furthest_index = 0_usize;
    let mut unreachable_tower = false;

    for snapshot in towers.iter() {
        let Some(cache) = TowerRangeCache::from_snapshot(snapshot) else {
            unreachable_tower = true;
            continue;
        };

        match cache.first_path_index(path) {
            Some(index) => {
                if index > furthest_index {
                    furthest_index = index;
                }
            }
            None => unreachable_tower = true,
        }
    }

    if unreachable_tower {
        return 10_000;
    }

    let steps = path.len().saturating_sub(1) as u64;

    if steps == 0 {
        return 0;
    }

    let numerator = u128::from(furthest_index as u64).saturating_mul(10_000);
    let denominator = u128::from(steps);
    (numerator / denominator) as u32
}

#[derive(Clone, Copy, Debug)]
struct TowerRangeCache {
    center_column: i64,
    center_row: i64,
    range_squared_half: i128,
}

impl TowerRangeCache {
    fn from_snapshot(snapshot: &TowerAnalyticsSnapshot) -> Option<Self> {
        let size = snapshot.region.size();
        if size.width() == 0 || size.height() == 0 {
            return None;
        }

        let origin = snapshot.region.origin();
        let center_column = i64::from(origin.column()) * 2 + i64::from(size.width());
        let center_row = i64::from(origin.row()) * 2 + i64::from(size.height());
        let radius_half = i128::from(snapshot.range_cells) * 2;
        let range_squared_half = radius_half * radius_half;

        Some(Self {
            center_column,
            center_row,
            range_squared_half,
        })
    }

    fn contains_cell(&self, cell_center_column: i64, cell_center_row: i64) -> bool {
        let dx = i128::from(cell_center_column).saturating_sub(i128::from(self.center_column));
        let dy = i128::from(cell_center_row).saturating_sub(i128::from(self.center_row));
        let distance_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
        distance_sq <= self.range_squared_half
    }

    fn first_path_index(&self, path: &[CellCoord]) -> Option<usize> {
        path.iter().enumerate().find_map(|(index, cell)| {
            let (column, row) = cell_center(cell);
            if self.contains_cell(column, row) {
                Some(index)
            } else {
                None
            }
        })
    }
}

fn trace_path(
    start: CellCoord,
    navigation: &NavigationFieldView<'_>,
    out: &mut VecDeque<CellCoord>,
) -> Option<()> {
    let mut current = start;
    let mut current_distance = navigation.distance(current)?;

    if current_distance == u16::MAX {
        return None;
    }

    loop {
        out.push_back(current);

        if current_distance == 0 {
            return Some(());
        }

        let mut next_cell = None;
        let mut best_distance = current_distance;

        for neighbor in neighbors(current, navigation.width(), navigation.height()) {
            let Some(distance) = navigation.distance(neighbor) else {
                continue;
            };

            if distance >= best_distance {
                continue;
            }

            best_distance = distance;
            next_cell = Some(neighbor);
        }

        let next = next_cell?;
        current = next;
        current_distance = best_distance;
    }
}

fn neighbors(cell: CellCoord, width: u32, height: u32) -> impl Iterator<Item = CellCoord> {
    let mut candidates = [None; 4];
    let mut count = 0;

    if let Some(row) = cell.row().checked_sub(1) {
        candidates[count] = Some(CellCoord::new(cell.column(), row));
        count += 1;
    }

    if let Some(column) = cell.column().checked_add(1) {
        if column < width {
            candidates[count] = Some(CellCoord::new(column, cell.row()));
            count += 1;
        }
    }

    if let Some(row) = cell.row().checked_add(1) {
        if row < height {
            candidates[count] = Some(CellCoord::new(cell.column(), row));
            count += 1;
        }
    }

    if let Some(column) = cell.column().checked_sub(1) {
        candidates[count] = Some(CellCoord::new(column, cell.row()));
        count += 1;
    }

    candidates.into_iter().take(count).flatten()
}

#[cfg(test)]
mod tests {
    use super::{
        select_shortest_navigation_path, tower_coverage_mean_bps,
        tower_firing_completion_percent_bps,
    };
    use crate::AnalyticsScratch;
    use maze_defence_core::{
        AnalyticsLayoutSnapshot, CellCoord, CellRect, CellRectSize, NavigationFieldView,
        TowerAnalyticsSnapshot, TowerAnalyticsView, TowerId, TowerKind,
    };
    use std::collections::VecDeque;

    #[test]
    fn chooses_shortest_path_across_spawners() {
        let navigation = NavigationFieldView::from_owned(vec![4, 3, 2, 3, 2, 1, 2, 1, 0], 3, 3);
        let layout = AnalyticsLayoutSnapshot::new(
            vec![CellCoord::new(0, 0), CellCoord::new(0, 1)],
            vec![CellCoord::new(2, 2)],
        );

        let mut path = Vec::new();
        let mut working = VecDeque::new();
        let mut scratch = AnalyticsScratch::new(&mut path, &mut working);

        let selected = select_shortest_navigation_path(&navigation, &layout, &mut scratch)
            .expect("expected reachable path");

        assert_eq!(
            selected,
            &[
                CellCoord::new(0, 1),
                CellCoord::new(1, 1),
                CellCoord::new(2, 1),
                CellCoord::new(2, 2)
            ]
        );
    }

    #[test]
    fn ignores_unreachable_spawners() {
        let navigation = NavigationFieldView::from_owned(
            vec![u16::MAX, u16::MAX, 2, u16::MAX, 2, 1, 2, 1, 0],
            3,
            3,
        );
        let layout = AnalyticsLayoutSnapshot::new(
            vec![CellCoord::new(0, 0), CellCoord::new(1, 2)],
            vec![CellCoord::new(2, 2)],
        );

        let mut path = Vec::new();
        let mut working = VecDeque::new();
        let mut scratch = AnalyticsScratch::new(&mut path, &mut working);

        let selected = select_shortest_navigation_path(&navigation, &layout, &mut scratch)
            .expect("reachable spawner should yield a path");

        assert_eq!(selected, &[CellCoord::new(1, 2), CellCoord::new(2, 2)]);
    }

    #[test]
    fn returns_none_when_no_path_exists() {
        let navigation = NavigationFieldView::from_owned(vec![u16::MAX; 4], 2, 2);
        let layout =
            AnalyticsLayoutSnapshot::new(vec![CellCoord::new(0, 0)], vec![CellCoord::new(1, 1)]);

        let mut path = Vec::new();
        let mut working = VecDeque::new();
        let mut scratch = AnalyticsScratch::new(&mut path, &mut working);

        assert!(select_shortest_navigation_path(&navigation, &layout, &mut scratch).is_none());
    }

    #[test]
    fn tower_coverage_returns_mean_basis_points() {
        let path = vec![
            CellCoord::new(0, 0),
            CellCoord::new(1, 0),
            CellCoord::new(2, 0),
        ];

        let towers = TowerAnalyticsView::from_snapshots(vec![
            TowerAnalyticsSnapshot {
                tower: TowerId::new(1),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    CellCoord::new(0, 0),
                    CellRectSize::new(1, 1),
                ),
                range_cells: 2,
                damage_per_second: 10,
            },
            TowerAnalyticsSnapshot {
                tower: TowerId::new(2),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    CellCoord::new(2, 0),
                    CellRectSize::new(1, 1),
                ),
                range_cells: 1,
                damage_per_second: 10,
            },
        ]);

        let coverage = tower_coverage_mean_bps(&path, &towers);
        assert_eq!(coverage, 8_333);
    }

    #[test]
    fn tower_coverage_zero_when_no_towers_or_path() {
        let empty_path: Vec<CellCoord> = Vec::new();
        let towers = TowerAnalyticsView::default();

        assert_eq!(tower_coverage_mean_bps(&empty_path, &towers), 0);

        let path = vec![CellCoord::new(0, 0)];
        assert_eq!(tower_coverage_mean_bps(&path, &towers), 0);
    }

    #[test]
    fn firing_completion_tracks_furthest_first_opportunity() {
        let path = vec![
            CellCoord::new(0, 0),
            CellCoord::new(1, 0),
            CellCoord::new(2, 0),
            CellCoord::new(3, 0),
        ];

        let towers = TowerAnalyticsView::from_snapshots(vec![
            TowerAnalyticsSnapshot {
                tower: TowerId::new(1),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    CellCoord::new(0, 0),
                    CellRectSize::new(1, 1),
                ),
                range_cells: 1,
                damage_per_second: 10,
            },
            TowerAnalyticsSnapshot {
                tower: TowerId::new(2),
                kind: TowerKind::Basic,
                region: CellRect::from_origin_and_size(
                    CellCoord::new(3, 0),
                    CellRectSize::new(1, 1),
                ),
                range_cells: 1,
                damage_per_second: 10,
            },
        ]);

        let completion = tower_firing_completion_percent_bps(&path, &towers);
        assert_eq!(completion, 6_666);
    }

    #[test]
    fn firing_completion_saturates_when_tower_never_sees_path() {
        let path = vec![CellCoord::new(0, 0), CellCoord::new(1, 0)];

        let towers = TowerAnalyticsView::from_snapshots(vec![TowerAnalyticsSnapshot {
            tower: TowerId::new(1),
            kind: TowerKind::Basic,
            region: CellRect::from_origin_and_size(CellCoord::new(5, 5), CellRectSize::new(1, 1)),
            range_cells: 1,
            damage_per_second: 10,
        }]);

        let completion = tower_firing_completion_percent_bps(&path, &towers);
        assert_eq!(completion, 10_000);
    }

    #[test]
    fn firing_completion_handles_single_cell_path() {
        let path = vec![CellCoord::new(0, 0)];

        let towers = TowerAnalyticsView::from_snapshots(vec![TowerAnalyticsSnapshot {
            tower: TowerId::new(1),
            kind: TowerKind::Basic,
            region: CellRect::from_origin_and_size(CellCoord::new(0, 0), CellRectSize::new(1, 1)),
            range_cells: 1,
            damage_per_second: 10,
        }]);

        let completion = tower_firing_completion_percent_bps(&path, &towers);
        assert_eq!(completion, 0);

        let unreachable_tower = TowerAnalyticsView::from_snapshots(vec![TowerAnalyticsSnapshot {
            tower: TowerId::new(2),
            kind: TowerKind::Basic,
            region: CellRect::from_origin_and_size(CellCoord::new(4, 4), CellRectSize::new(1, 1)),
            range_cells: 1,
            damage_per_second: 10,
        }]);

        let completion_unreachable = tower_firing_completion_percent_bps(&path, &unreachable_tower);
        assert_eq!(completion_unreachable, 10_000);
    }

    #[test]
    fn firing_completion_zero_when_no_towers() {
        let path = vec![CellCoord::new(0, 0), CellCoord::new(1, 0)];
        let towers = TowerAnalyticsView::default();

        assert_eq!(tower_firing_completion_percent_bps(&path, &towers), 0);
    }
}
