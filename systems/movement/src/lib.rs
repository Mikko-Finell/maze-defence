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
    BugId, BugSnapshot, BugView, CellCoord, Command, Direction, Event, Goal, NavigationFieldView,
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
    stall_counters: StallCounter,
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
        self.stall_counters.begin_tick(&ordered);

        for (index, bug) in ordered.into_iter().enumerate() {
            if !bug.ready_for_step {
                continue;
            }

            let Some(goal) = select_goal(bug.cell, &self.targets) else {
                continue;
            };

            if bug.cell == goal.cell() {
                self.stall_counters.reset(index);
                continue;
            }

            let Some(next_cell) = self.plan_next_hop(
                bug,
                goal,
                navigation_view,
                occupancy_view,
                columns,
                rows,
                is_cell_blocked,
            ) else {
                self.stall_counters.increment(index);
                continue;
            };

            if !cell_available_for(next_cell, bug.id, occupancy_view) {
                self.stall_counters.increment(index);
                continue;
            }

            let Some(direction) = direction_between(bug.cell, next_cell) else {
                self.stall_counters.increment(index);
                continue;
            };

            out.push(Command::StepBug {
                bug_id: bug.id,
                direction,
            });
            self.stall_counters.reset(index);
        }
    }

    fn plan_next_hop<F>(
        &mut self,
        bug: &BugSnapshot,
        goal: Goal,
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        columns: u32,
        rows: u32,
        is_cell_blocked: &F,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let Some(current_distance) = navigation_view.distance(bug.cell) else {
            return None;
        };

        if current_distance == u16::MAX {
            return None;
        }

        let mut best: Option<NeighborCandidate> = None;

        for (order, neighbor) in
            enumerate_neighbors(bug.cell, columns, rows, goal.cell(), is_cell_blocked).enumerate()
        {
            if !cell_available_for(neighbor, bug.id, occupancy_view) {
                continue;
            }

            let Some(distance) = navigation_view.distance(neighbor) else {
                continue;
            };

            if distance == u16::MAX {
                continue;
            }

            let delta = i32::from(distance) - i32::from(current_distance);
            if delta >= 0 {
                continue;
            }

            let candidate = NeighborCandidate {
                cell: neighbor,
                distance,
                order,
            };

            best = match best {
                None => Some(candidate),
                Some(existing) => {
                    if candidate.distance < existing.distance
                        || (candidate.distance == existing.distance
                            && candidate.order < existing.order)
                    {
                        Some(candidate)
                    } else {
                        Some(existing)
                    }
                }
            };
        }

        best.map(|candidate| candidate.cell)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NeighborCandidate {
    cell: CellCoord,
    distance: u16,
    order: usize,
}

impl Default for CrowdPlanner {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            prepared_dimensions: None,
            congestion: Vec::new(),
            detour_queue: Vec::new(),
            last_cell: LastCellRing::default(),
            stall_counters: StallCounter::default(),
        }
    }
}

#[derive(Debug, Default)]
struct LastCellRing {
    history: Vec<[Option<CellCoord>; 2]>,
    bug_ids: Vec<Option<BugId>>,
}

impl LastCellRing {
    fn begin_tick(&mut self, ordered: &[&BugSnapshot]) {
        let bug_count = ordered.len();
        self.history.resize(bug_count, [None, None]);
        self.bug_ids.resize(bug_count, None);

        for (index, bug) in ordered.iter().enumerate() {
            if self.bug_ids[index] != Some(bug.id) {
                self.history[index] = [None, None];
                self.bug_ids[index] = Some(bug.id);
            }
        }
    }
}

#[derive(Debug, Default)]
struct StallCounter {
    counts: Vec<u32>,
    bug_ids: Vec<Option<BugId>>,
}

impl StallCounter {
    fn begin_tick(&mut self, ordered: &[&BugSnapshot]) {
        let bug_count = ordered.len();
        self.counts.resize(bug_count, 0);
        self.bug_ids.resize(bug_count, None);

        for (index, bug) in ordered.iter().enumerate() {
            if self.bug_ids[index] != Some(bug.id) {
                self.counts[index] = 0;
                self.bug_ids[index] = Some(bug.id);
            }
        }
    }

    fn reset(&mut self, index: usize) {
        if let Some(value) = self.counts.get_mut(index) {
            *value = 0;
        }
    }

    fn increment(&mut self, index: usize) {
        if let Some(value) = self.counts.get_mut(index) {
            *value = value.saturating_add(1);
        }
    }
}

fn enumerate_neighbors<F>(
    cell: CellCoord,
    columns: u32,
    rows: u32,
    goal: CellCoord,
    is_cell_blocked: &F,
) -> NeighborIter
where
    F: Fn(CellCoord) -> bool,
{
    let mut neighbors = NeighborIter::default();
    let mut consider = |candidate: CellCoord| {
        if candidate == goal || !is_cell_blocked(candidate) {
            neighbors.push(candidate);
        }
    };

    if cell.row() > 0 {
        consider(CellCoord::new(cell.column(), cell.row() - 1));
    }
    if cell.column() > 0 {
        consider(CellCoord::new(cell.column() - 1, cell.row()));
    }
    if cell.column() + 1 < columns {
        consider(CellCoord::new(cell.column() + 1, cell.row()));
    }
    if cell.row() + 1 < rows {
        consider(CellCoord::new(cell.column(), cell.row() + 1));
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
        let mut movement = Movement::default();
        let width = 3;
        let height = 3;
        let navigation = navigation_field(width, height, &[4, 3, 2, 3, 2, 1, 4, 3, 2]);
        let cell_count = usize::try_from(width).unwrap() * usize::try_from(height).unwrap();
        let occupancy_cells = vec![None; cell_count];
        let occupancy_view = OccupancyView::new(&occupancy_cells, width, height);

        let target = CellCoord::new(2, 1);
        let _ = movement
            .planner
            .prepare_workspace(width, height, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(0, 1));
        let next = movement
            .planner
            .plan_next_hop(
                &bug,
                Goal::at(target),
                &navigation,
                occupancy_view,
                width,
                height,
                &|_| false,
            )
            .expect("gradient should select a neighbour");

        assert_eq!(next, CellCoord::new(1, 1));
    }

    #[test]
    fn gradient_skips_occupied_neighbor() {
        let mut movement = Movement::default();
        let width = 3;
        let height = 3;
        let navigation = navigation_field(width, height, &[4, 3, 2, 3, 2, 1, 2, 1, 0]);
        let cell_count = usize::try_from(width).unwrap() * usize::try_from(height).unwrap();
        let mut occupancy_cells = vec![None; cell_count];
        set_occupant(
            &mut occupancy_cells,
            width,
            CellCoord::new(2, 1),
            BugId::new(99),
        );
        let occupancy_view = OccupancyView::new(&occupancy_cells, width, height);

        let target = CellCoord::new(2, 2);
        let _ = movement
            .planner
            .prepare_workspace(width, height, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(1, 1));
        let next = movement
            .planner
            .plan_next_hop(
                &bug,
                Goal::at(target),
                &navigation,
                occupancy_view,
                width,
                height,
                &|_| false,
            )
            .expect("planner should find an alternate neighbour");

        assert_eq!(next, CellCoord::new(1, 2));
    }

    #[test]
    fn gradient_prefers_neighbour_order_on_tie() {
        let mut movement = Movement::default();
        let width = 3;
        let height = 3;
        let navigation = navigation_field(width, height, &[4, 3, 2, 3, 2, 1, 2, 1, 0]);
        let cell_count = usize::try_from(width).unwrap() * usize::try_from(height).unwrap();
        let occupancy_cells = vec![None; cell_count];
        let occupancy_view = OccupancyView::new(&occupancy_cells, width, height);

        let target = CellCoord::new(2, 2);
        let _ = movement
            .planner
            .prepare_workspace(width, height, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(1, 1));
        let next = movement
            .planner
            .plan_next_hop(
                &bug,
                Goal::at(target),
                &navigation,
                occupancy_view,
                width,
                height,
                &|_| false,
            )
            .expect("planner should pick a neighbour");

        assert_eq!(next, CellCoord::new(2, 1));
    }

    fn navigation_stub(width: u32, height: u32) -> NavigationFieldView<'static> {
        let cells = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        NavigationFieldView::from_owned(vec![0; cells], width, height)
    }

    fn navigation_field(
        width: u32,
        height: u32,
        distances: &[u16],
    ) -> NavigationFieldView<'static> {
        let cell_count = usize::try_from(width).unwrap() * usize::try_from(height).unwrap();
        assert_eq!(distances.len(), cell_count);
        NavigationFieldView::from_owned(distances.to_vec(), width, height)
    }

    fn set_occupant(cells: &mut [Option<BugId>], width: u32, cell: CellCoord, bug: BugId) {
        let width = usize::try_from(width).expect("width fits usize");
        let row = usize::try_from(cell.row()).expect("row fits usize");
        let column = usize::try_from(cell.column()).expect("column fits usize");
        let index = row * width + column;
        cells[index] = Some(bug);
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
