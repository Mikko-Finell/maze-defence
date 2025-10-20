#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic movement system that plans paths and proposes bug steps.

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
};

use maze_defence_core::{BugId, CellCoord, Command, Direction, Event};
use maze_defence_world::query::{BugSnapshot, BugView, OccupancyView};

/// Pure system that reacts to world events and emits movement commands.
#[derive(Debug, Default)]
pub struct Movement {
    frontier: BinaryHeap<Reverse<NodeState>>,
    came_from: Vec<Option<CellCoord>>,
    g_score: Vec<u32>,
    targets: Vec<CellCoord>,
    target_columns: Vec<u32>,
    prepared_dimensions: Option<(u32, u32)>,
    workspace_nodes: usize,
    active_nodes: usize,
}

impl Movement {
    /// Consumes world events and immutable views to emit movement commands.
    pub fn handle(
        &mut self,
        events: &[Event],
        bug_view: &BugView,
        occupancy_view: OccupancyView<'_>,
        targets: &[CellCoord],
        out: &mut Vec<Command>,
    ) {
        let (columns, rows) = occupancy_view.dimensions();
        let node_count = self.prepare_workspace(columns, rows, targets);
        if node_count == 0 {
            return;
        }

        if !events
            .iter()
            .any(|event| matches!(event, Event::TimeAdvanced { .. }))
        {
            return;
        }

        self.emit_step_commands(bug_view, occupancy_view, columns, rows, out);
    }

    fn emit_step_commands(
        &mut self,
        bug_view: &BugView,
        occupancy_view: OccupancyView<'_>,
        columns: u32,
        rows: u32,
        out: &mut Vec<Command>,
    ) {
        for bug in bug_view.iter() {
            if !bug.ready_for_step {
                continue;
            }

            let Some(next_cell) = self.plan_next_hop(bug, columns, rows) else {
                continue;
            };

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

    fn plan_next_hop(&mut self, bug: &BugSnapshot, columns: u32, rows: u32) -> Option<CellCoord> {
        if self.targets.is_empty() {
            return None;
        }

        if self.targets.iter().any(|target| *target == bug.cell) {
            return None;
        }

        let rows_with_exit = rows.saturating_add(1);
        let start_index = index(columns, rows_with_exit, bug.cell)?;

        self.reset_workspace();
        self.g_score[start_index] = 0;
        self.frontier.push(Reverse(NodeState::new(
            bug.cell,
            0,
            heuristic_to_targets(bug.cell, &self.targets),
        )));

        while let Some(Reverse(current)) = self.frontier.pop() {
            if self.targets.iter().any(|target| *target == current.cell) {
                return self.reconstruct_first_hop(bug.cell, current.cell, columns, rows_with_exit);
            }

            let neighbors = enumerate_neighbors(current.cell, columns, rows, &self.target_columns);
            for neighbor in neighbors {
                let Some(neighbor_index) = index(columns, rows_with_exit, neighbor) else {
                    continue;
                };

                let tentative = current.g_cost + 1;
                if tentative >= self.g_score[neighbor_index] {
                    continue;
                }

                self.came_from[neighbor_index] = Some(current.cell);
                self.g_score[neighbor_index] = tentative;
                self.frontier.push(Reverse(NodeState::new(
                    neighbor,
                    tentative,
                    heuristic_to_targets(neighbor, &self.targets),
                )));
            }
        }

        None
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
            let Some(previous) = self.came_from[index] else {
                return None;
            };

            if previous == start {
                return Some(current);
            }

            current = previous;
        }
    }

    fn prepare_workspace(&mut self, columns: u32, rows: u32, targets: &[CellCoord]) -> usize {
        if targets.is_empty() {
            self.targets.clear();
            self.target_columns.clear();
            self.prepared_dimensions = Some((columns, rows));
            self.active_nodes = 0;
            return 0;
        }

        if self.prepared_dimensions != Some((columns, rows)) || self.targets.as_slice() != targets {
            self.targets.clear();
            self.targets.extend_from_slice(targets);
            self.target_columns = self.targets.iter().map(|cell| cell.column()).collect();
            self.prepared_dimensions = Some((columns, rows));
        }

        let rows_with_exit = rows.saturating_add(1);
        let node_count_u64 = u64::from(columns) * u64::from(rows_with_exit);
        let node_count = usize::try_from(node_count_u64).unwrap_or(0);
        if node_count > self.workspace_nodes {
            self.g_score.resize(node_count, u32::MAX);
            self.came_from.resize(node_count, None);
            self.workspace_nodes = node_count;
        }
        self.active_nodes = node_count;
        node_count
    }

    fn reset_workspace(&mut self) {
        self.frontier.clear();
        for value in self.g_score.iter_mut().take(self.active_nodes) {
            *value = u32::MAX;
        }
        for entry in self.came_from.iter_mut().take(self.active_nodes) {
            *entry = None;
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

fn enumerate_neighbors(
    cell: CellCoord,
    columns: u32,
    rows: u32,
    target_columns: &[u32],
) -> NeighborIter {
    let mut neighbors = NeighborIter::default();
    if cell.row() < rows {
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
        } else if cell.row() + 1 == rows {
            if target_columns.iter().any(|column| *column == cell.column()) {
                neighbors.push(CellCoord::new(cell.column(), rows));
            }
        }
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

fn heuristic_to_targets(cell: CellCoord, targets: &[CellCoord]) -> u32 {
    targets
        .iter()
        .map(|target| manhattan_distance(cell, *target))
        .min()
        .unwrap_or(0)
}

fn manhattan_distance(from: CellCoord, to: CellCoord) -> u32 {
    from.column().abs_diff(to.column()) + from.row().abs_diff(to.row())
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

        assert_eq!(movement.prepare_workspace(0, 0, &[]), 0);
        assert!(movement.targets.is_empty());
        assert!(movement.target_columns.is_empty());
        assert_eq!(movement.active_nodes, 0);

        let targets = vec![CellCoord::new(1, 4)];
        assert_eq!(movement.prepare_workspace(3, 4, &targets), 15);
        assert_eq!(movement.targets, targets);

        let alternate_targets = vec![CellCoord::new(2, 2), CellCoord::new(2, 3)];
        assert_eq!(movement.prepare_workspace(4, 3, &alternate_targets), 16);
        assert_eq!(movement.targets, alternate_targets);
    }

    #[test]
    fn manhattan_distance_matches_expectation() {
        let from = CellCoord::new(0, 0);
        let to = CellCoord::new(3, 4);
        assert_eq!(manhattan_distance(from, to), 7);
    }
}
