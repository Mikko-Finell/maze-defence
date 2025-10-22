#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic movement system that plans paths and proposes bug steps.

use maze_defence_core::{
    BugId, BugSnapshot, BugView, CellCoord, Command, Direction, Event, NavigationFieldView,
    OccupancyView, PlayMode, ReservationLedgerView,
};
use maze_defence_world::query::select_goal;

/// Pure system that reacts to world events and emits movement commands.
#[derive(Debug)]
pub struct Movement {
    planner: CrowdPlanner,
    play_mode: PlayMode,
}

impl Movement {
    /// Consumes world events and immutable views to emit movement commands.
    pub fn handle<F>(
        &mut self,
        events: &[Event],
        bug_view: &BugView,
        occupancy_view: OccupancyView<'_>,
        navigation_view: NavigationFieldView<'_>,
        reservation_ledger: ReservationLedgerView<'_>,
        targets: &[CellCoord],
        is_cell_blocked: F,
        out: &mut Vec<Command>,
    ) where
        F: Fn(CellCoord) -> bool,
    {
        for event in events {
            if let Event::PlayModeChanged { mode } = event {
                self.play_mode = *mode;
            }
        }

        if self.play_mode == PlayMode::Builder {
            return;
        }

        let (columns, rows) = occupancy_view.dimensions();
        let node_count = self
            .planner
            .prepare_workspace(columns, rows, &navigation_view, targets);
        if node_count == 0 {
            return;
        }

        if !events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. }))
        {
            return;
        }

        self.planner.plan(
            bug_view,
            occupancy_view,
            &navigation_view,
            &reservation_ledger,
            columns,
            rows,
            &is_cell_blocked,
            out,
        );
    }
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            planner: CrowdPlanner::default(),
            play_mode: PlayMode::Attack,
        }
    }
}

#[derive(Debug)]
struct CrowdPlanner {
    targets: Vec<CellCoord>,
    prepared_dimensions: Option<(u32, u32)>,
    congestion: Vec<u8>,
    detour_queue: Vec<CellCoord>,
    last_cell: LastCellRing,
}

impl CrowdPlanner {
    fn prepare_workspace(
        &mut self,
        columns: u32,
        rows: u32,
        navigation_view: &NavigationFieldView<'_>,
        targets: &[CellCoord],
    ) -> usize {
        if targets.is_empty() {
            self.targets.clear();
            self.prepared_dimensions = Some((columns, rows));
            self.congestion.clear();
            self.detour_queue.clear();
            return 0;
        }

        if self.prepared_dimensions != Some((columns, rows)) || self.targets.as_slice() != targets {
            self.targets.clear();
            self.targets.extend_from_slice(targets);
            self.prepared_dimensions = Some((columns, rows));
        }

        let node_count_u64 = u64::from(columns) * u64::from(rows);
        let node_count = usize::try_from(node_count_u64).unwrap_or(0);

        let field_cells = navigation_view.cells().len();
        if self.congestion.len() < field_cells {
            self.congestion.resize(field_cells, 0);
        } else {
            for value in &mut self.congestion {
                *value = 0;
            }
        }
        if self.detour_queue.capacity() < field_cells {
            self.detour_queue
                .reserve(field_cells - self.detour_queue.capacity());
        }

        node_count
    }

    fn plan<F>(
        &mut self,
        bug_view: &BugView,
        occupancy_view: OccupancyView<'_>,
        navigation_view: &NavigationFieldView<'_>,
        reservation_ledger: &ReservationLedgerView<'_>,
        columns: u32,
        rows: u32,
        is_cell_blocked: &F,
        out: &mut Vec<Command>,
    ) where
        F: Fn(CellCoord) -> bool,
    {
        self.prepare_per_tick(navigation_view, reservation_ledger.len());
        self.emit_step_commands(
            bug_view,
            occupancy_view,
            navigation_view,
            columns,
            rows,
            is_cell_blocked,
            out,
        );
    }

    fn prepare_per_tick(
        &mut self,
        navigation_view: &NavigationFieldView<'_>,
        reservation_count: usize,
    ) {
        if self.congestion.len() < navigation_view.cells().len() {
            self.congestion.resize(navigation_view.cells().len(), 0);
        }
        for value in &mut self.congestion {
            *value = 0;
        }

        self.detour_queue.clear();
        self.detour_queue.reserve(reservation_count);
    }

    fn emit_step_commands<F>(
        &mut self,
        bug_view: &BugView,
        occupancy_view: OccupancyView<'_>,
        navigation_view: &NavigationFieldView<'_>,
        columns: u32,
        rows: u32,
        is_cell_blocked: &F,
        out: &mut Vec<Command>,
    ) where
        F: Fn(CellCoord) -> bool,
    {
        let mut ordered: Vec<_> = bug_view.iter().collect();
        ordered.sort_by_key(|bug| bug.id);
        self.last_cell.begin_tick(&ordered);

        for (index, bug) in ordered.into_iter().enumerate() {
            if !bug.ready_for_step {
                continue;
            }

            let Some(goal) = select_goal(bug.cell, &self.targets) else {
                continue;
            };

            if bug.cell == goal.cell() {
                continue;
            }

            let Some(next_cell) = self.plan_gradient_step(
                bug,
                navigation_view,
                occupancy_view,
                is_cell_blocked,
                columns,
                rows,
            ) else {
                self.last_cell.mark_stall(index);
                continue;
            };

            if !cell_available_for(next_cell, bug.id, occupancy_view) {
                self.last_cell.mark_stall(index);
                continue;
            }

            if let Some(direction) = direction_between(bug.cell, next_cell) {
                self.last_cell.record_progress(index, bug.cell);
                out.push(Command::StepBug {
                    bug_id: bug.id,
                    direction,
                });
            } else {
                self.last_cell.mark_stall(index);
            }
        }
    }

    fn plan_gradient_step<F>(
        &self,
        bug: &BugSnapshot,
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        is_cell_blocked: &F,
        columns: u32,
        rows: u32,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let current_distance = navigation_view.distance(bug.cell)?;
        let mut best: Option<Candidate> = None;

        for neighbor in cardinal_neighbors(bug.cell, columns, rows) {
            if is_cell_blocked(neighbor) {
                continue;
            }
            if !cell_available_for(neighbor, bug.id, occupancy_view) {
                continue;
            }

            let Some(distance) = navigation_view.distance(neighbor) else {
                continue;
            };

            let delta = i32::from(distance) - i32::from(current_distance);
            if delta >= 0 {
                continue;
            }

            let candidate = Candidate {
                cell: neighbor,
                distance,
            };
            best = Some(match best {
                None => candidate,
                Some(existing) => {
                    if candidate.is_better_than(existing) {
                        candidate
                    } else {
                        existing
                    }
                }
            });
        }

        best.map(|candidate| candidate.cell)
    }
}

impl Default for CrowdPlanner {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            prepared_dimensions: None,
            congestion: Vec::new(),
            detour_queue: Vec::new(),
            last_cell: LastCellRing::default(),
        }
    }
}

#[derive(Debug, Default)]
struct LastCellRing {
    history: Vec<[Option<CellCoord>; 2]>,
    bug_ids: Vec<Option<BugId>>,
    stalled_for: Vec<u32>,
}

impl LastCellRing {
    fn begin_tick(&mut self, ordered: &[&BugSnapshot]) {
        let bug_count = ordered.len();
        self.history.resize(bug_count, [None, None]);
        self.bug_ids.resize(bug_count, None);
        self.stalled_for.resize(bug_count, 0);

        for (index, bug) in ordered.iter().enumerate() {
            if self.bug_ids[index] != Some(bug.id) {
                self.history[index] = [None, None];
                self.bug_ids[index] = Some(bug.id);
                self.stalled_for[index] = 0;
            }
        }
    }

    fn record_progress(&mut self, index: usize, from: CellCoord) {
        if let Some(history) = self.history.get_mut(index) {
            history[1] = history[0];
            history[0] = Some(from);
        }
        if let Some(counter) = self.stalled_for.get_mut(index) {
            *counter = 0;
        }
    }

    fn mark_stall(&mut self, index: usize) {
        if let Some(counter) = self.stalled_for.get_mut(index) {
            *counter = counter.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    cell: CellCoord,
    distance: u16,
}

impl Candidate {
    fn is_better_than(self, other: Candidate) -> bool {
        let rank = (
            u32::from(self.distance),
            self.cell.column(),
            self.cell.row(),
        );
        let other_rank = (
            u32::from(other.distance),
            other.cell.column(),
            other.cell.row(),
        );
        rank < other_rank
    }
}

fn cardinal_neighbors(cell: CellCoord, columns: u32, rows: u32) -> NeighborIter {
    let mut neighbors = NeighborIter::default();

    if cell.row() > 0 {
        neighbors.push(CellCoord::new(cell.column(), cell.row() - 1));
    }
    if cell.column() > 0 {
        neighbors.push(CellCoord::new(cell.column() - 1, cell.row()));
    }
    if cell.column() + 1 < columns {
        neighbors.push(CellCoord::new(cell.column() + 1, cell.row()));
    }
    if cell.row() + 1 < rows {
        neighbors.push(CellCoord::new(cell.column(), cell.row() + 1));
    }

    neighbors
}

#[derive(Clone, Debug, Default)]
struct NeighborIter {
    buffer: [Option<CellCoord>; 4],
    len: usize,
    cursor: usize,
}

impl NeighborIter {
    fn push(&mut self, cell: CellCoord) {
        if self.len < self.buffer.len() {
            self.buffer[self.len] = Some(cell);
            self.len += 1;
        }
    }
}

impl Iterator for NeighborIter {
    type Item = CellCoord;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.len {
            return None;
        }

        let value = self.buffer[self.cursor];
        self.cursor += 1;
        value
    }
}

fn cell_available_for(cell: CellCoord, bug_id: BugId, occupancy_view: OccupancyView<'_>) -> bool {
    match occupancy_view.occupant(cell) {
        None => true,
        Some(occupant) => occupant == bug_id,
    }
}

fn direction_between(from: CellCoord, to: CellCoord) -> Option<Direction> {
    let column_diff = from.column().abs_diff(to.column());
    let row_diff = from.row().abs_diff(to.row());
    if column_diff + row_diff != 1 {
        return None;
    }

    if column_diff == 1 {
        if to.column() > from.column() {
            Some(Direction::East)
        } else {
            Some(Direction::West)
        }
    } else if to.row() > from.row() {
        Some(Direction::South)
    } else {
        Some(Direction::North)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{BugColor, Health};
    use std::time::Duration;

    #[test]
    fn direction_between_neighbors() {
        let origin = CellCoord::new(3, 3);
        assert_eq!(
            direction_between(origin, CellCoord::new(3, 2)),
            Some(Direction::North)
        );
        assert_eq!(
            direction_between(origin, CellCoord::new(4, 3)),
            Some(Direction::East)
        );
        assert_eq!(
            direction_between(origin, CellCoord::new(3, 4)),
            Some(Direction::South)
        );
        assert_eq!(
            direction_between(origin, CellCoord::new(2, 3)),
            Some(Direction::West)
        );
        assert_eq!(direction_between(origin, origin), None);
    }

    #[test]
    fn provided_targets_are_cached() {
        let mut movement = Movement::default();

        assert_eq!(
            movement
                .planner
                .prepare_workspace(0, 0, &navigation_stub(0, 0), &[]),
            0
        );
        assert!(movement.planner.targets.is_empty());

        let targets = vec![CellCoord::new(1, 4)];
        assert_eq!(
            movement
                .planner
                .prepare_workspace(3, 4, &navigation_stub(3, 4), &targets),
            12
        );
        assert_eq!(movement.planner.targets, targets);

        let alternate_targets = vec![CellCoord::new(2, 2), CellCoord::new(2, 3)];
        assert_eq!(
            movement
                .planner
                .prepare_workspace(4, 3, &navigation_stub(4, 3), &alternate_targets),
            12
        );
        assert_eq!(movement.planner.targets, alternate_targets);
    }

    #[test]
    fn gradient_prefers_lower_distance_neighbor() {
        let planner = CrowdPlanner::default();
        let columns = 3;
        let rows = 3;
        let navigation = navigation_with_distances(
            columns,
            rows,
            vec![
                4, 3, 2, // row 0
                3, 2, 1, // row 1
                2, 1, 0, // row 2
            ],
        );
        let occupancy_cells = occupancy_stub(columns, rows);
        let occupancy = OccupancyView::new(&occupancy_cells, columns, rows);
        let bug = bug_snapshot_at(CellCoord::new(0, 0));

        let next =
            planner.plan_gradient_step(&bug, &navigation, occupancy, &|_| false, columns, rows);

        assert_eq!(next, Some(CellCoord::new(0, 1)));
    }

    #[test]
    fn gradient_returns_none_when_all_progress_cells_blocked() {
        let planner = CrowdPlanner::default();
        let columns = 2;
        let rows = 2;
        let navigation = navigation_with_distances(columns, rows, vec![3, 2, 2, 1]);
        let mut occupancy_cells = occupancy_stub(columns, rows);
        set_occupant(
            &mut occupancy_cells,
            columns,
            CellCoord::new(0, 1),
            BugId::new(99),
        );
        let occupancy = OccupancyView::new(&occupancy_cells, columns, rows);
        let bug = bug_snapshot_at(CellCoord::new(0, 0));
        let block_east = |cell: CellCoord| cell == CellCoord::new(1, 0);

        let next =
            planner.plan_gradient_step(&bug, &navigation, occupancy, &block_east, columns, rows);

        assert!(next.is_none());
    }

    fn navigation_stub(width: u32, height: u32) -> NavigationFieldView<'static> {
        let cells = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        NavigationFieldView::from_owned(vec![0; cells], width, height)
    }

    fn navigation_with_distances(
        width: u32,
        height: u32,
        distances: Vec<u16>,
    ) -> NavigationFieldView<'static> {
        NavigationFieldView::from_owned(distances, width, height)
    }

    fn occupancy_stub(width: u32, height: u32) -> Vec<Option<BugId>> {
        let cells = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        vec![None; cells]
    }

    fn set_occupant(cells: &mut [Option<BugId>], width: u32, cell: CellCoord, bug_id: BugId) {
        let width = usize::try_from(width).expect("width fits usize");
        let row = usize::try_from(cell.row()).expect("row fits usize");
        let column = usize::try_from(cell.column()).expect("column fits usize");
        let index = row * width + column;
        if let Some(slot) = cells.get_mut(index) {
            *slot = Some(bug_id);
        }
    }

    fn bug_snapshot_at(cell: CellCoord) -> BugSnapshot {
        BugSnapshot {
            id: BugId::new(1),
            cell,
            color: BugColor::from_rgb(0, 0, 0),
            max_health: Health::new(3),
            health: Health::new(3),
            ready_for_step: true,
            accumulated: Duration::default(),
        }
    }
}
