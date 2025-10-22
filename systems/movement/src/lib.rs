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
    stalled_for: StallCounter,
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
        is_cell_blocked: &F,
        out: &mut Vec<Command>,
    ) where
        F: Fn(CellCoord) -> bool,
    {
        let mut ordered: Vec<_> = bug_view.iter().collect();
        ordered.sort_by_key(|bug| bug.id);
        self.last_cell.begin_tick(&ordered);
        self.stalled_for.begin_tick(&ordered);

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

            let Some(next_cell) =
                self.plan_next_hop(index, bug, navigation_view, occupancy_view, is_cell_blocked)
            else {
                continue;
            };

            if next_cell != goal.cell() && is_cell_blocked(next_cell) {
                continue;
            }

            if !cell_available_for(next_cell, bug.id, occupancy_view) {
                continue;
            }

            if let Some(direction) = direction_between(bug.cell, next_cell) {
                out.push(Command::StepBug {
                    bug_id: bug.id,
                    direction,
                });
            }
        }
    }

    fn plan_next_hop<F>(
        &mut self,
        bug_index: usize,
        bug: &BugSnapshot,
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        is_cell_blocked: &F,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let current_distance = navigation_view.distance(bug.cell).unwrap_or(u16::MAX);
        let width = navigation_view.width();
        let height = navigation_view.height();

        let mut best: Option<(CellCoord, u16)> = None;

        for neighbor in neighbors_within_field(bug.cell, width, height) {
            if is_cell_blocked(neighbor) {
                continue;
            }

            if !cell_available_for(neighbor, bug.id, occupancy_view) {
                continue;
            }

            let Some(distance) = navigation_view.distance(neighbor) else {
                continue;
            };

            if distance >= current_distance {
                continue;
            }

            match &mut best {
                None => best = Some((neighbor, distance)),
                Some((best_cell, best_distance)) => {
                    if distance < *best_distance
                        || (distance == *best_distance
                            && lexicographically_less(neighbor, *best_cell))
                    {
                        *best_cell = neighbor;
                        *best_distance = distance;
                    }
                }
            }
        }

        if let Some((cell, _)) = best {
            self.stalled_for.reset(bug_index);
            Some(cell)
        } else {
            self.stalled_for.increment(bug_index);
            None
        }
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
            stalled_for: StallCounter::default(),
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
    values: Vec<u32>,
    bug_ids: Vec<Option<BugId>>,
}

impl StallCounter {
    fn begin_tick(&mut self, ordered: &[&BugSnapshot]) {
        let bug_count = ordered.len();
        self.values.resize(bug_count, 0);
        self.bug_ids.resize(bug_count, None);

        for (index, bug) in ordered.iter().enumerate() {
            if self.bug_ids[index] != Some(bug.id) {
                self.values[index] = 0;
                self.bug_ids[index] = Some(bug.id);
            }
        }
    }

    fn reset(&mut self, index: usize) {
        if let Some(value) = self.values.get_mut(index) {
            *value = 0;
        }
    }

    fn increment(&mut self, index: usize) {
        if let Some(value) = self.values.get_mut(index) {
            *value = value.saturating_add(1);
        }
    }

    #[cfg(test)]
    fn value(&self, index: usize) -> u32 {
        self.values.get(index).copied().unwrap_or(0)
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

fn neighbors_within_field(
    cell: CellCoord,
    width: u32,
    height: u32,
) -> impl Iterator<Item = CellCoord> {
    let mut buffer = [None; 4];
    let mut count = 0;

    if cell.column() >= width || cell.row() >= height {
        return buffer.into_iter().take(0).flatten();
    }

    if let Some(row) = cell.row().checked_sub(1) {
        buffer[count] = Some(CellCoord::new(cell.column(), row));
        count += 1;
    }

    if let Some(column) = cell.column().checked_add(1) {
        if column < width {
            buffer[count] = Some(CellCoord::new(column, cell.row()));
            count += 1;
        }
    }

    if let Some(row) = cell.row().checked_add(1) {
        if row < height {
            buffer[count] = Some(CellCoord::new(cell.column(), row));
            count += 1;
        }
    }

    if let Some(column) = cell.column().checked_sub(1) {
        buffer[count] = Some(CellCoord::new(column, cell.row()));
        count += 1;
    }

    buffer.into_iter().take(count).flatten()
}

fn lexicographically_less(left: CellCoord, right: CellCoord) -> bool {
    left.column() < right.column() || (left.column() == right.column() && left.row() < right.row())
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
    fn plan_next_hop_prefers_lower_distance_neighbor() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![4, 3, 2, 3, 2, 1, 2, 1, 0], 3, 3);
        let target = CellCoord::new(2, 2);
        let occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(0, 0));
        let ordered = vec![&bug];
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let next = movement
            .planner
            .plan_next_hop(0, &bug, &navigation, occupancy, &|_| false);

        assert_eq!(next, Some(CellCoord::new(0, 1)));
        assert_eq!(movement.planner.stalled_for.value(0), 0);
    }

    #[test]
    fn plan_next_hop_respects_occupancy() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![4, 3, 2, 3, 2, 1, 2, 1, 0], 3, 3);
        let target = CellCoord::new(2, 2);
        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        occupancy_cells[3] = Some(BugId::new(2));
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let bug = bug_snapshot_at(CellCoord::new(0, 0));
        let ordered = vec![&bug];
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let next = movement
            .planner
            .plan_next_hop(0, &bug, &navigation, occupancy, &|_| false);

        assert_eq!(next, Some(CellCoord::new(1, 0)));
        assert_eq!(movement.planner.stalled_for.value(0), 0);
    }

    #[test]
    fn plan_next_hop_increments_stall_counter_when_blocked() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![0, 0, 0, 0], 2, 2);
        let target = CellCoord::new(1, 1);
        let _ = movement
            .planner
            .prepare_workspace(2, 2, &navigation, &[target]);

        let occupancy_cells: Vec<Option<BugId>> = vec![None; 4];
        let occupancy = OccupancyView::new(&occupancy_cells, 2, 2);

        let bug = bug_snapshot_at(CellCoord::new(0, 0));
        let ordered = vec![&bug];
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let next = movement
            .planner
            .plan_next_hop(0, &bug, &navigation, occupancy, &|_| false);

        assert_eq!(next, None);
        assert_eq!(movement.planner.stalled_for.value(0), 1);
    }

    fn navigation_stub(width: u32, height: u32) -> NavigationFieldView<'static> {
        let cells = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        NavigationFieldView::from_owned(vec![0; cells], width, height)
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
