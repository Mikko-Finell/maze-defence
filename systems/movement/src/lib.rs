#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic movement system that plans paths and proposes bug steps.

use std::{cmp::Ordering, collections::BinaryHeap};

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
    frontier: BinaryHeap<NodeState>,
    came_from: Vec<Option<CellCoord>>,
    g_score: Vec<u32>,
    generation: Vec<u32>,
    targets: Vec<CellCoord>,
    prepared_dimensions: Option<(u32, u32)>,
    workspace_nodes: usize,
    current_generation: u32,
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
        if node_count > self.workspace_nodes {
            self.g_score.resize(node_count, u32::MAX);
            self.came_from.resize(node_count, None);
            self.generation.resize(node_count, 0);
            self.workspace_nodes = node_count;
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

        for bug in ordered {
            if !bug.ready_for_step {
                continue;
            }

            let Some(goal) = select_goal(bug.cell, &self.targets) else {
                continue;
            };

            if bug.cell == goal.cell() {
                continue;
            }

            let Some(next_cell) = self.plan_next_hop(bug, goal, columns, rows, is_cell_blocked)
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
        bug: &BugSnapshot,
        goal: Goal,
        columns: u32,
        rows: u32,
        is_cell_blocked: &F,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let start_index = index(columns, rows, bug.cell)?;

        self.reset_workspace();
        self.prepare_node(start_index);
        self.g_score[start_index] = 0;
        self.frontier.push(NodeState::new(
            bug.cell,
            0,
            heuristic_to_goal(bug.cell, goal.cell()),
        ));

        while let Some(current) = self.frontier.pop() {
            if current.cell == goal.cell() {
                return self.reconstruct_first_hop(bug.cell, goal.cell(), columns, rows);
            }

            let neighbors =
                enumerate_neighbors(current.cell, columns, rows, goal.cell(), is_cell_blocked);
            for neighbor in neighbors {
                let Some(neighbor_index) = index(columns, rows, neighbor) else {
                    continue;
                };

                let tentative = current.g_cost + 1;
                self.prepare_node(neighbor_index);
                if tentative >= self.g_score[neighbor_index] {
                    continue;
                }

                self.came_from[neighbor_index] = Some(current.cell);
                self.g_score[neighbor_index] = tentative;
                self.frontier.push(NodeState::new(
                    neighbor,
                    tentative,
                    heuristic_to_goal(neighbor, goal.cell()),
                ));
            }
        }

        None
    }

    fn prepare_node(&mut self, index: usize) {
        if self.generation[index] != self.current_generation {
            self.generation[index] = self.current_generation;
            self.g_score[index] = u32::MAX;
            self.came_from[index] = None;
        }
    }

    fn reconstruct_first_hop(
        &self,
        start: CellCoord,
        goal: CellCoord,
        columns: u32,
        rows: u32,
    ) -> Option<CellCoord> {
        let mut current = goal;

        loop {
            let index = index(columns, rows, current)?;
            let previous = self.came_from_for_current_generation(index)?;

            if previous == start {
                return Some(current);
            }

            current = previous;
        }
    }

    fn came_from_for_current_generation(&self, index: usize) -> Option<CellCoord> {
        if self.generation.get(index) == Some(&self.current_generation) {
            self.came_from[index]
        } else {
            None
        }
    }

    fn reset_workspace(&mut self) {
        self.frontier.clear();
        if self.current_generation == u32::MAX {
            self.current_generation = 1;
            for stamp in &mut self.generation {
                *stamp = 0;
            }
        } else {
            self.current_generation = self.current_generation.saturating_add(1);
        }
    }
}

impl Default for CrowdPlanner {
    fn default() -> Self {
        Self {
            frontier: BinaryHeap::new(),
            came_from: Vec::new(),
            g_score: Vec::new(),
            generation: Vec::new(),
            targets: Vec::new(),
            prepared_dimensions: None,
            workspace_nodes: 0,
            current_generation: 0,
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
struct NodeState {
    cell: CellCoord,
    g_cost: u32,
    f_cost: u32,
}

impl NodeState {
    fn new(cell: CellCoord, g_cost: u32, heuristic: u32) -> Self {
        Self {
            cell,
            g_cost,
            f_cost: g_cost.saturating_add(heuristic),
        }
    }
}

impl Ord for NodeState {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f_cost
            .cmp(&self.f_cost)
            .then_with(|| other.g_cost.cmp(&self.g_cost))
            .then_with(|| other.cell.column().cmp(&self.cell.column()))
            .then_with(|| other.cell.row().cmp(&self.cell.row()))
    }
}

impl PartialOrd for NodeState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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

fn heuristic_to_goal(cell: CellCoord, goal: CellCoord) -> u32 {
    cell.manhattan_distance(goal)
}

fn index(columns: u32, rows: u32, cell: CellCoord) -> Option<usize> {
    if cell.column() >= columns || cell.row() >= rows {
        return None;
    }

    let width = usize::try_from(columns).ok()?;
    let row = usize::try_from(cell.row()).ok()?;
    let column = usize::try_from(cell.column()).ok()?;
    Some(row * width + column)
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
    fn path_planning_is_consistent_across_generations() {
        let mut movement = Movement::default();
        let columns = 5;
        let rows = 4;
        let target = CellCoord::new(4, 3);
        assert_eq!(
            movement.planner.prepare_workspace(
                columns,
                rows,
                &navigation_stub(columns, rows),
                &[target]
            ),
            20
        );
        let goal = Goal::at(target);
        let blocked = [
            CellCoord::new(1, 0),
            CellCoord::new(1, 1),
            CellCoord::new(1, 2),
            CellCoord::new(3, 2),
        ];
        let is_cell_blocked = |cell: CellCoord| blocked.iter().any(|candidate| *candidate == cell);

        let expected_path = vec![
            CellCoord::new(0, 1),
            CellCoord::new(0, 2),
            CellCoord::new(0, 3),
            CellCoord::new(1, 3),
            CellCoord::new(2, 3),
            CellCoord::new(3, 3),
            CellCoord::new(4, 3),
        ];

        let first_path = collect_path(
            &mut movement,
            bug_snapshot_at(CellCoord::new(0, 0)),
            goal,
            columns,
            rows,
            &is_cell_blocked,
        );
        assert_eq!(first_path, expected_path);

        let _ = movement.planner.prepare_workspace(
            columns,
            rows,
            &navigation_stub(columns, rows),
            &[target],
        );
        let second_path = collect_path(
            &mut movement,
            bug_snapshot_at(CellCoord::new(0, 0)),
            goal,
            columns,
            rows,
            &is_cell_blocked,
        );
        assert_eq!(second_path, expected_path);
    }

    #[test]
    fn heuristic_matches_manhattan_distance() {
        let from = CellCoord::new(0, 0);
        let goal = CellCoord::new(3, 4);
        assert_eq!(heuristic_to_goal(from, goal), 7);
    }

    fn collect_path<F>(
        movement: &mut Movement,
        mut bug: BugSnapshot,
        goal: Goal,
        columns: u32,
        rows: u32,
        is_cell_blocked: &F,
    ) -> Vec<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let mut path = Vec::new();
        while bug.cell != goal.cell() {
            let Some(next) =
                movement
                    .planner
                    .plan_next_hop(&bug, goal, columns, rows, is_cell_blocked)
            else {
                break;
            };
            path.push(next);
            bug.cell = next;
        }
        path
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
