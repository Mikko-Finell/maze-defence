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
    stalled: StallCounter,
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

        usize::try_from(u64::from(columns) * u64::from(rows)).unwrap_or(0)
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
        self.stalled.begin_tick(&ordered);

        for (index, bug) in ordered.iter().enumerate() {
            let bug = *bug;
            if !bug.ready_for_step {
                continue;
            }

            let Some(goal) = select_goal(bug.cell, &self.targets) else {
                continue;
            };

            if bug.cell == goal.cell() {
                continue;
            }

            match self.plan_next_hop(bug, navigation_view, occupancy_view, is_cell_blocked) {
                PlanOutcome::Advance(next_cell) => {
                    self.stalled.reset(index);
                    if let Some(direction) = direction_between(bug.cell, next_cell) {
                        out.push(Command::StepBug {
                            bug_id: bug.id,
                            direction,
                        });
                    }
                }
                PlanOutcome::Stalled => {
                    self.stalled.increment(index);
                }
            }
        }
    }

    fn plan_next_hop<F>(
        &mut self,
        bug: &BugSnapshot,
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        is_cell_blocked: &F,
    ) -> PlanOutcome
    where
        F: Fn(CellCoord) -> bool,
    {
        let Some(current_distance) = navigation_view.distance(bug.cell) else {
            return PlanOutcome::Stalled;
        };
        if current_distance == u16::MAX {
            return PlanOutcome::Stalled;
        }

        let mut best: Option<(CellCoord, u16)> = None;
        for neighbor in neighbors(bug.cell) {
            if is_cell_blocked(neighbor) {
                continue;
            }

            if !cell_available_for(neighbor, bug.id, occupancy_view) {
                continue;
            }

            let Some(distance) = navigation_view.distance(neighbor) else {
                continue;
            };
            if distance == u16::MAX {
                continue;
            }
            if distance >= current_distance {
                continue;
            }

            let replace = match best {
                None => true,
                Some((candidate, best_distance)) => {
                    distance < best_distance
                        || (distance == best_distance
                            && (neighbor.column(), neighbor.row())
                                < (candidate.column(), candidate.row()))
                }
            };

            if replace {
                best = Some((neighbor, distance));
            }
        }

        match best {
            Some((cell, _)) => PlanOutcome::Advance(cell),
            None => PlanOutcome::Stalled,
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
            stalled: StallCounter::default(),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlanOutcome {
    Advance(CellCoord),
    Stalled,
}

#[derive(Debug, Default)]
struct StallCounter {
    bug_ids: Vec<Option<BugId>>,
    counts: Vec<u8>,
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

    fn reset(&mut self, bug_index: usize) {
        if let Some(count) = self.counts.get_mut(bug_index) {
            *count = 0;
        }
    }

    fn increment(&mut self, bug_index: usize) {
        if let Some(count) = self.counts.get_mut(bug_index) {
            *count = count.saturating_add(1);
        }
    }
}

fn neighbors(cell: CellCoord) -> impl Iterator<Item = CellCoord> {
    let north = cell
        .row()
        .checked_sub(1)
        .map(|row| CellCoord::new(cell.column(), row));
    let south = cell
        .row()
        .checked_add(1)
        .map(|row| CellCoord::new(cell.column(), row));
    let east = cell
        .column()
        .checked_add(1)
        .map(|column| CellCoord::new(column, cell.row()));
    let west = cell
        .column()
        .checked_sub(1)
        .map(|column| CellCoord::new(column, cell.row()));

    [north, west, east, south].into_iter().flatten()
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
    fn plan_next_hop_follows_static_gradient() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(
            vec![
                4, 3, 2, // row 0
                3, 2, 1, // row 1
                2, 1, 0, // row 2
            ],
            3,
            3,
        );
        let occupancy_cells = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let outcome = movement.planner.plan_next_hop(
            &bug_snapshot_at(CellCoord::new(0, 0)),
            &navigation,
            occupancy,
            &|_| false,
        );

        assert_eq!(outcome, PlanOutcome::Advance(CellCoord::new(0, 1)));
    }

    #[test]
    fn plan_next_hop_skips_blocked_neighbors() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(
            vec![
                4, 3, 2, // row 0
                3, 2, 1, // row 1
                2, 1, 0, // row 2
            ],
            3,
            3,
        );
        let occupancy_cells = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let blocked = CellCoord::new(0, 1);
        let outcome = movement.planner.plan_next_hop(
            &bug_snapshot_at(CellCoord::new(0, 0)),
            &navigation,
            occupancy,
            &|cell| cell == blocked,
        );

        assert_eq!(outcome, PlanOutcome::Advance(CellCoord::new(1, 0)));
    }

    #[test]
    fn plan_next_hop_stalls_when_all_progress_cells_blocked() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(
            vec![
                4, 3, 2, // row 0
                3, 2, 1, // row 1
                2, 1, 0, // row 2
            ],
            3,
            3,
        );
        let occupancy_cells = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let blockers = [CellCoord::new(0, 1), CellCoord::new(1, 0)];
        let outcome = movement.planner.plan_next_hop(
            &bug_snapshot_at(CellCoord::new(0, 0)),
            &navigation,
            occupancy,
            &|cell| blockers.contains(&cell),
        );

        assert_eq!(outcome, PlanOutcome::Stalled);
    }

    #[test]
    fn plan_next_hop_stalls_without_lower_neighbor() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(
            vec![
                2, 2, 2, // row 0
                2, 0, 2, // row 1
                2, 2, 2, // row 2
            ],
            3,
            3,
        );
        let occupancy_cells = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let outcome = movement.planner.plan_next_hop(
            &bug_snapshot_at(CellCoord::new(1, 1)),
            &navigation,
            occupancy,
            &|_| false,
        );

        assert_eq!(outcome, PlanOutcome::Stalled);
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
