#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Pure system that computes deterministic tower targets from world snapshots.

use maze_defence_core::{
    BugId, BugView, CellPoint, PlayMode, TowerId, TowerKind, TowerTarget, TowerView,
};

/// Tower targeting system that reuses scratch buffers to avoid repeated allocations.
#[derive(Debug, Default)]
pub struct TowerTargeting {
    tower_workspace: Vec<TowerWorkspace>,
    bug_workspace: Vec<BugCandidate>,
}

impl TowerTargeting {
    /// Creates a new tower targeting system with empty scratch buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes tower targets for the provided world snapshot.
    ///
    /// The output buffer is cleared before populating it with the latest
    /// assignments.
    pub fn handle(
        &mut self,
        play_mode: PlayMode,
        towers: &TowerView,
        bugs: &BugView,
        cells_per_tile: u32,
        out: &mut Vec<TowerTarget>,
    ) {
        out.clear();

        if play_mode != PlayMode::Attack {
            return;
        }

        if towers.iter().next().is_none() || bugs.iter().next().is_none() {
            return;
        }

        self.prepare_tower_workspace(towers);
        if self.tower_workspace.is_empty() {
            return;
        }

        self.prepare_bug_workspace(bugs);
        if self.bug_workspace.is_empty() {
            return;
        }

        for tower in &self.tower_workspace {
            let radius_cells = tower.kind.range_in_cells(cells_per_tile);
            let radius_half = i128::from(radius_cells) * 2;
            let max_distance = radius_half * radius_half;

            let mut best: Option<BestCandidate> = None;

            for candidate in &self.bug_workspace {
                let dx = i128::from(candidate.center.column - tower.center.column);
                let dy = i128::from(candidate.center.row - tower.center.row);
                let distance_sq = dx * dx + dy * dy;

                if distance_sq > max_distance {
                    continue;
                }

                let current = BestCandidate {
                    distance_sq,
                    bug: candidate.id,
                    bug_column: candidate.column,
                    bug_row: candidate.row,
                    bug_center: candidate.center,
                };

                match &mut best {
                    Some(existing) => {
                        if current.precedes(existing) {
                            *existing = current;
                        }
                    }
                    None => best = Some(current),
                }
            }

            if let Some(best_candidate) = best {
                out.push(TowerTarget {
                    tower: tower.id,
                    bug: best_candidate.bug,
                    tower_center_cells: tower.center.to_cell_point(),
                    bug_center_cells: best_candidate.bug_center.to_cell_point(),
                });
            }
        }
    }

    fn prepare_tower_workspace(&mut self, towers: &TowerView) {
        self.tower_workspace.clear();
        let (lower, _) = towers.iter().size_hint();
        self.tower_workspace.reserve(lower);

        for snapshot in towers.iter() {
            let region = snapshot.region;
            let size = region.size();
            if size.width() == 0 || size.height() == 0 {
                continue;
            }

            let origin = region.origin();
            let center = HalfCellPoint {
                column: i64::from(origin.column()) * 2 + i64::from(size.width()),
                row: i64::from(origin.row()) * 2 + i64::from(size.height()),
            };

            self.tower_workspace.push(TowerWorkspace {
                id: snapshot.id,
                kind: snapshot.kind,
                center,
            });
        }
    }

    fn prepare_bug_workspace(&mut self, bugs: &BugView) {
        self.bug_workspace.clear();
        let (lower, _) = bugs.iter().size_hint();
        self.bug_workspace.reserve(lower);

        for snapshot in bugs.iter() {
            let cell = snapshot.cell;
            let center = HalfCellPoint {
                column: i64::from(cell.column()) * 2 + 1,
                row: i64::from(cell.row()) * 2 + 1,
            };

            self.bug_workspace.push(BugCandidate {
                id: snapshot.id,
                column: cell.column(),
                row: cell.row(),
                center,
            });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TowerWorkspace {
    id: TowerId,
    kind: TowerKind,
    center: HalfCellPoint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BugCandidate {
    id: BugId,
    column: u32,
    row: u32,
    center: HalfCellPoint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HalfCellPoint {
    column: i64,
    row: i64,
}

impl HalfCellPoint {
    fn to_cell_point(self) -> CellPoint {
        CellPoint::new(self.column as f32 / 2.0, self.row as f32 / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BestCandidate {
    distance_sq: i128,
    bug: BugId,
    bug_column: u32,
    bug_row: u32,
    bug_center: HalfCellPoint,
}

impl BestCandidate {
    fn precedes(&self, other: &Self) -> bool {
        if self.distance_sq != other.distance_sq {
            return self.distance_sq < other.distance_sq;
        }

        if self.bug != other.bug {
            return self.bug < other.bug;
        }

        if self.bug_column != other.bug_column {
            return self.bug_column < other.bug_column;
        }

        self.bug_row < other.bug_row
    }
}

#[cfg(test)]
mod tests {
    use super::{CellPoint, TowerTarget, TowerTargeting};
    use maze_defence_core::{
        BugId, BugSnapshot, BugView, CellCoord, CellRect, CellRectSize, Health, PlayMode, TowerId,
        TowerKind, TowerSnapshot, TowerView,
    };
    use std::time::Duration;

    fn tower_view(snapshots: Vec<TowerSnapshot>) -> TowerView {
        TowerView::from_snapshots(snapshots)
    }

    fn bug_view(snapshots: Vec<BugSnapshot>) -> BugView {
        BugView::from_snapshots(snapshots)
    }

    fn tower_snapshot(id: u32, origin: (u32, u32), size: (u32, u32)) -> TowerSnapshot {
        TowerSnapshot {
            id: TowerId::new(id),
            kind: TowerKind::Basic,
            region: CellRect::from_origin_and_size(
                CellCoord::new(origin.0, origin.1),
                CellRectSize::new(size.0, size.1),
            ),
        }
    }

    fn bug_snapshot(id: u32, cell: (u32, u32)) -> BugSnapshot {
        BugSnapshot {
            id: BugId::new(id),
            cell: CellCoord::new(cell.0, cell.1),
            color: maze_defence_core::BugColor::from_rgb(255, 0, 0),
            health: Health::new(3),
            ready_for_step: true,
            accumulated: Duration::ZERO,
        }
    }

    #[test]
    fn targets_bug_within_range() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (4, 4), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(2, (7, 5))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);

        assert_eq!(out.len(), 1);
        let target = out[0];
        assert_eq!(target.tower, TowerId::new(1));
        assert_eq!(target.bug, BugId::new(2));
        assert_eq!(target.tower_center_cells, CellPoint::new(5.0, 5.0));
        assert_eq!(target.bug_center_cells, CellPoint::new(7.5, 5.5));
    }

    #[test]
    fn bug_outside_range_is_ignored() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (0, 0), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(2, (20, 20))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn smaller_bug_id_is_preferred_when_distances_match() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (2, 2), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(20, (4, 3)), bug_snapshot(10, (1, 3))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bug, BugId::new(10));
    }

    #[test]
    fn column_tie_break_prefers_smaller_column() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (4, 4), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(10, (6, 5)), bug_snapshot(10, (4, 5))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 4, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bug_center_cells, CellPoint::new(4.5, 5.5));
    }

    #[test]
    fn row_tie_break_prefers_smaller_row() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (4, 4), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(10, (5, 6)), bug_snapshot(10, (5, 4))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 4, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bug_center_cells, CellPoint::new(5.5, 4.5));
    }

    #[test]
    fn zero_sized_tower_produces_no_target() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (2, 2), (0, 3))]);
        let bugs = bug_view(vec![bug_snapshot(1, (2, 2))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn builder_mode_clears_output() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (0, 0), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(1, (1, 1))]);

        let mut out = vec![TowerTarget {
            tower: TowerId::new(99),
            bug: BugId::new(99),
            tower_center_cells: CellPoint::new(0.0, 0.0),
            bug_center_cells: CellPoint::new(0.0, 0.0),
        }];

        system.handle(PlayMode::Builder, &towers, &bugs, 2, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn empty_collections_produce_no_targets() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(Vec::new());
        let bugs = bug_view(vec![bug_snapshot(1, (1, 1))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);
        assert!(out.is_empty());

        let towers = tower_view(vec![tower_snapshot(1, (0, 0), (2, 2))]);
        let bugs = bug_view(Vec::new());
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn decreasing_cells_per_tile_removes_out_of_range_targets() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (0, 0), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(1, (7, 0))]);

        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);
        assert_eq!(
            out.len(),
            1,
            "bug should be in range with larger cells_per_tile"
        );

        system.handle(PlayMode::Attack, &towers, &bugs, 1, &mut out);
        assert!(
            out.is_empty(),
            "bug should fall out of range with smaller cells_per_tile"
        );
    }

    #[test]
    fn removing_bugs_does_not_select_out_of_range_candidates() {
        let mut system = TowerTargeting::new();
        let towers = tower_view(vec![tower_snapshot(1, (0, 0), (2, 2))]);
        let bugs = bug_view(vec![bug_snapshot(1, (2, 0)), bug_snapshot(2, (20, 0))]);
        let mut out = Vec::new();
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bug, BugId::new(1));

        let bugs = bug_view(vec![bug_snapshot(2, (20, 0))]);
        system.handle(PlayMode::Attack, &towers, &bugs, 2, &mut out);
        assert!(out.is_empty(), "far bug should not be targeted when alone");
    }
}
