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
    OccupancyView, PlayMode, ReservationLedgerView, CONGESTION_LOOKAHEAD, DETOUR_RADIUS,
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
    detour_queue: Vec<DetourNode>,
    detour_marks: Vec<u32>,
    detour_generation: u32,
    reserved_destinations: Vec<CellCoord>,
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

        if self.detour_marks.len() < field_cells {
            self.detour_marks.resize(field_cells, 0);
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
        let mut ordered: Vec<_> = bug_view.iter().collect();
        ordered.sort_by_key(|bug| bug.id);

        self.prepare_per_tick(&ordered, navigation_view);
        self.emit_step_commands(
            &ordered,
            occupancy_view,
            navigation_view,
            reservation_ledger,
            is_cell_blocked,
            out,
        );
    }

    fn prepare_per_tick(
        &mut self,
        ordered: &[&BugSnapshot],
        navigation_view: &NavigationFieldView<'_>,
    ) {
        if self.congestion.len() < navigation_view.cells().len() {
            self.congestion.resize(navigation_view.cells().len(), 0);
        }
        for value in &mut self.congestion {
            *value = 0;
        }

        self.build_congestion_map(ordered, navigation_view);

        self.detour_queue.clear();
        self.reserved_destinations.clear();
        self.reserved_destinations.reserve(ordered.len());
    }

    fn emit_step_commands<F>(
        &mut self,
        ordered: &[&BugSnapshot],
        occupancy_view: OccupancyView<'_>,
        navigation_view: &NavigationFieldView<'_>,
        reservation_ledger: &ReservationLedgerView<'_>,
        is_cell_blocked: &F,
        out: &mut Vec<Command>,
    ) where
        F: Fn(CellCoord) -> bool,
    {
        self.last_cell.begin_tick(&ordered);
        self.stalled_for.begin_tick(&ordered);

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

            let Some(next_cell) = self.plan_next_hop(
                index,
                bug,
                ordered,
                navigation_view,
                occupancy_view,
                reservation_ledger,
                is_cell_blocked,
            ) else {
                continue;
            };

            if next_cell != goal.cell()
                && !self.cell_passable(
                    next_cell,
                    bug.id,
                    occupancy_view,
                    reservation_ledger,
                    is_cell_blocked,
                )
            {
                continue;
            }

            if let Some(direction) = direction_between(bug.cell, next_cell) {
                out.push(Command::StepBug {
                    bug_id: bug.id,
                    direction,
                });
                self.reserved_destinations.push(next_cell);
                self.last_cell.record(index, bug.cell);
            }
        }
    }

    fn plan_next_hop<F>(
        &mut self,
        bug_index: usize,
        bug: &BugSnapshot,
        ordered: &[&BugSnapshot],
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        reservation_ledger: &ReservationLedgerView<'_>,
        is_cell_blocked: &F,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let current_distance = navigation_view.distance(bug.cell).unwrap_or(u16::MAX);
        let width = navigation_view.width();
        let height = navigation_view.height();
        let (occupancy_columns, occupancy_rows) = occupancy_view.dimensions();

        let current_congestion = self.congestion_value(bug.cell, width, height).unwrap_or(0);

        let mut decreasing_best: Option<Candidate> = None;
        let mut flat_best: Option<Candidate> = None;
        let last_cell = self.last_cell.last(bug_index);

        for neighbor in neighbors_within_field(bug.cell, width, height) {
            if self.cell_reserved_by_lower_bug(
                bug.id,
                neighbor,
                ordered,
                reservation_ledger,
                occupancy_columns,
                occupancy_rows,
            ) {
                continue;
            }

            if !self.cell_passable(
                neighbor,
                bug.id,
                occupancy_view,
                reservation_ledger,
                is_cell_blocked,
            ) {
                continue;
            }

            let Some(distance) = navigation_view.distance(neighbor) else {
                continue;
            };

            let Some(index) = field_index(neighbor, width, height) else {
                continue;
            };

            let neighbor_congestion = self.congestion.get(index).copied().unwrap_or_default();
            let distance_delta = i32::from(distance) - i32::from(current_distance);

            if distance_delta < 0 {
                let candidate = Candidate {
                    cell: neighbor,
                    distance,
                    congestion: neighbor_congestion,
                };
                update_best(&mut decreasing_best, candidate);
                continue;
            }

            if distance_delta == 0
                && neighbor_congestion < current_congestion
                && last_cell.map_or(true, |last| last != neighbor)
            {
                let candidate = Candidate {
                    cell: neighbor,
                    distance,
                    congestion: neighbor_congestion,
                };
                update_best(&mut flat_best, candidate);
            }
        }

        if let Some(candidate) = decreasing_best {
            self.stalled_for.reset(bug_index);
            return Some(candidate.cell);
        }

        if let Some(candidate) = flat_best {
            self.stalled_for.reset(bug_index);
            return Some(candidate.cell);
        }

        if let Some(next_cell) = self.search_detour(
            bug,
            ordered,
            navigation_view,
            occupancy_view,
            reservation_ledger,
            is_cell_blocked,
            current_distance,
            occupancy_columns,
            occupancy_rows,
        ) {
            self.stalled_for.reset(bug_index);
            return Some(next_cell);
        }

        self.stalled_for.increment(bug_index);
        None
    }

    fn search_detour<F>(
        &mut self,
        bug: &BugSnapshot,
        ordered: &[&BugSnapshot],
        navigation_view: &NavigationFieldView<'_>,
        occupancy_view: OccupancyView<'_>,
        reservation_ledger: &ReservationLedgerView<'_>,
        is_cell_blocked: &F,
        current_distance: u16,
        occupancy_columns: u32,
        occupancy_rows: u32,
    ) -> Option<CellCoord>
    where
        F: Fn(CellCoord) -> bool,
    {
        let radius = usize::try_from(DETOUR_RADIUS).unwrap_or(0);
        if radius == 0 {
            return None;
        }
        let radius_u32 = u32::try_from(radius).unwrap_or(0);
        if radius_u32 == 0 {
            return None;
        }

        let width = navigation_view.width();
        let height = navigation_view.height();

        self.detour_generation = self.detour_generation.wrapping_add(1);
        if self.detour_generation == 0 {
            self.detour_marks.fill(0);
            self.detour_generation = 1;
        }
        let generation = self.detour_generation;

        self.detour_queue.clear();
        self.detour_queue.push(DetourNode {
            cell: bug.cell,
            depth: 0,
            first_hop: bug.cell,
        });

        if let Some(index) = field_index(bug.cell, width, height) {
            if let Some(mark) = self.detour_marks.get_mut(index) {
                *mark = generation;
            }
        }

        let mut best_fallback: Option<(Candidate, CellCoord)> = None;
        let mut head = 0;

        while head < self.detour_queue.len() {
            let node = self.detour_queue[head];
            head += 1;

            if node.depth >= radius_u32 {
                continue;
            }

            for neighbor in neighbors_within_field(node.cell, width, height) {
                let Some(index) = field_index(neighbor, width, height) else {
                    continue;
                };

                if self.detour_marks.get(index).copied().unwrap_or(0) == generation {
                    continue;
                }

                if self.cell_reserved_by_lower_bug(
                    bug.id,
                    neighbor,
                    ordered,
                    reservation_ledger,
                    occupancy_columns,
                    occupancy_rows,
                ) {
                    continue;
                }

                if node.depth > 0 {
                    if let Some(occupant) = occupancy_view.occupant(neighbor) {
                        if occupant != bug.id {
                            continue;
                        }
                    }
                }

                if !self.cell_passable(
                    neighbor,
                    bug.id,
                    occupancy_view,
                    reservation_ledger,
                    is_cell_blocked,
                ) {
                    continue;
                }

                if let Some(mark) = self.detour_marks.get_mut(index) {
                    *mark = generation;
                }

                let Some(distance) = navigation_view.distance(neighbor) else {
                    continue;
                };

                let congestion = self.congestion.get(index).copied().unwrap_or_default();

                let first_hop = if node.depth == 0 {
                    neighbor
                } else {
                    node.first_hop
                };

                let distance_delta = i32::from(distance) - i32::from(current_distance);

                if distance_delta < 0 {
                    return Some(first_hop);
                }

                if distance_delta == 0 {
                    let candidate = Candidate {
                        cell: neighbor,
                        distance,
                        congestion,
                    };

                    match &best_fallback {
                        None => best_fallback = Some((candidate, first_hop)),
                        Some((best, _)) if candidate_better_than(candidate, *best) => {
                            best_fallback = Some((candidate, first_hop));
                        }
                        _ => {}
                    }
                }

                if node.depth + 1 < radius_u32 {
                    self.detour_queue.push(DetourNode {
                        cell: neighbor,
                        depth: node.depth + 1,
                        first_hop,
                    });
                }
            }
        }

        best_fallback.map(|(_, first_hop)| first_hop)
    }

    fn cell_passable<F>(
        &self,
        cell: CellCoord,
        bug_id: BugId,
        occupancy_view: OccupancyView<'_>,
        reservation_ledger: &ReservationLedgerView<'_>,
        is_cell_blocked: &F,
    ) -> bool
    where
        F: Fn(CellCoord) -> bool,
    {
        if let Some(occupant) = occupancy_view.occupant(cell) {
            if occupant == bug_id {
                return true;
            }

            return reservation_ledger.claim_for(occupant).is_some();
        }

        !is_cell_blocked(cell)
    }

    fn cell_reserved_by_lower_bug(
        &self,
        bug_id: BugId,
        cell: CellCoord,
        ordered: &[&BugSnapshot],
        reservation_ledger: &ReservationLedgerView<'_>,
        occupancy_columns: u32,
        occupancy_rows: u32,
    ) -> bool {
        if self
            .reserved_destinations
            .iter()
            .any(|reserved| *reserved == cell)
        {
            return true;
        }

        reservation_ledger
            .iter()
            .filter(|claim| claim.bug_id() < bug_id)
            .any(|claim| {
                let Some(origin) = bug_position(ordered, claim.bug_id()) else {
                    return false;
                };
                let Some(destination) =
                    step_cell(origin, claim.direction(), occupancy_columns, occupancy_rows)
                else {
                    return false;
                };
                destination == cell
            })
    }

    fn build_congestion_map(
        &mut self,
        ordered: &[&BugSnapshot],
        navigation_view: &NavigationFieldView<'_>,
    ) {
        let width = navigation_view.width();
        let height = navigation_view.height();
        let lookahead = usize::try_from(CONGESTION_LOOKAHEAD).unwrap_or(0);

        for bug in ordered.iter().copied() {
            let mut current = bug.cell;
            let Some(mut current_distance) = navigation_view.distance(current) else {
                continue;
            };

            if current_distance == 0 {
                continue;
            }

            for _ in 0..lookahead {
                let mut next_cell: Option<(CellCoord, u16)> = None;

                for neighbor in neighbors_within_field(current, width, height) {
                    let Some(distance) = navigation_view.distance(neighbor) else {
                        continue;
                    };

                    if distance >= current_distance {
                        continue;
                    }

                    match next_cell {
                        None => next_cell = Some((neighbor, distance)),
                        Some((best_cell, best_distance)) => {
                            if distance < best_distance
                                || (distance == best_distance
                                    && lexicographically_less(neighbor, best_cell))
                            {
                                next_cell = Some((neighbor, distance));
                            }
                        }
                    }
                }

                let Some((step, step_distance)) = next_cell else {
                    break;
                };

                if let Some(index) = field_index(step, width, height) {
                    if let Some(value) = self.congestion.get_mut(index) {
                        *value = value.saturating_add(1);
                    }
                }

                current = step;
                current_distance = step_distance;

                if current_distance == 0 {
                    break;
                }
            }
        }
    }

    fn congestion_value(&self, cell: CellCoord, width: u32, height: u32) -> Option<u8> {
        let index = field_index(cell, width, height)?;
        self.congestion.get(index).copied()
    }
}

#[derive(Clone, Copy, Debug)]
struct DetourNode {
    cell: CellCoord,
    depth: u32,
    first_hop: CellCoord,
}

#[derive(Clone, Copy)]
struct Candidate {
    cell: CellCoord,
    distance: u16,
    congestion: u8,
}

fn update_best(current: &mut Option<Candidate>, candidate: Candidate) {
    match current {
        None => *current = Some(candidate),
        Some(existing) => {
            if candidate_better_than(candidate, *existing) {
                *existing = candidate;
            }
        }
    }
}

fn candidate_better_than(candidate: Candidate, existing: Candidate) -> bool {
    if candidate.distance < existing.distance {
        return true;
    }
    if candidate.distance > existing.distance {
        return false;
    }

    if candidate.congestion < existing.congestion {
        return true;
    }
    if candidate.congestion > existing.congestion {
        return false;
    }

    lexicographically_less(candidate.cell, existing.cell)
}

fn bug_position(ordered: &[&BugSnapshot], bug_id: BugId) -> Option<CellCoord> {
    ordered
        .iter()
        .copied()
        .find(|bug| bug.id == bug_id)
        .map(|bug| bug.cell)
}

fn step_cell(cell: CellCoord, direction: Direction, columns: u32, rows: u32) -> Option<CellCoord> {
    match direction {
        Direction::North => cell
            .row()
            .checked_sub(1)
            .map(|row| CellCoord::new(cell.column(), row)),
        Direction::South => cell
            .row()
            .checked_add(1)
            .filter(|row| *row < rows)
            .map(|row| CellCoord::new(cell.column(), row)),
        Direction::East => cell
            .column()
            .checked_add(1)
            .filter(|column| *column < columns)
            .map(|column| CellCoord::new(column, cell.row())),
        Direction::West => cell
            .column()
            .checked_sub(1)
            .map(|column| CellCoord::new(column, cell.row())),
    }
}

impl Default for CrowdPlanner {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            prepared_dimensions: None,
            congestion: Vec::new(),
            detour_queue: Vec::new(),
            detour_marks: Vec::new(),
            detour_generation: 0,
            reserved_destinations: Vec::new(),
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

    fn record(&mut self, index: usize, cell: CellCoord) {
        if let Some(entry) = self.history.get_mut(index) {
            entry.rotate_right(1);
            entry[0] = Some(cell);
        }
    }

    fn last(&self, index: usize) -> Option<CellCoord> {
        self.history
            .get(index)
            .and_then(|entry| entry.get(0))
            .copied()
            .flatten()
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

fn field_index(cell: CellCoord, width: u32, height: u32) -> Option<usize> {
    if cell.column() >= width || cell.row() >= height {
        return None;
    }

    let column = usize::try_from(cell.column()).ok()?;
    let row = usize::try_from(cell.row()).ok()?;
    let width = usize::try_from(width).ok()?;
    Some(row * width + column)
}

/// Lexicographic comparison used during neighbor tie-break: column, then row.
fn lexicographically_less(left: CellCoord, right: CellCoord) -> bool {
    left.column() < right.column() || (left.column() == right.column() && left.row() < right.row())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{BugColor, Health, ReservationClaim, ReservationLedgerView};
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
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement
            .planner
            .congestion
            .iter_mut()
            .for_each(|value| *value = 0);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|_| false,
        );

        assert_eq!(next, Some(CellCoord::new(0, 1)));
        assert_eq!(movement.planner.stalled_for.value(0), 0);
    }

    #[test]
    fn plan_next_hop_breaks_ties_by_column_then_row() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![5, 4, 5, 4, 5, 6, 5, 6, 7], 3, 3);
        let target = CellCoord::new(0, 0);
        let occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(1, 1));
        let ordered = vec![&bug];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement
            .planner
            .congestion
            .iter_mut()
            .for_each(|value| *value = 0);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|_| false,
        );

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
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement
            .planner
            .congestion
            .iter_mut()
            .for_each(|value| *value = 0);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let occupancy_blocked = occupancy;
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| occupancy_blocked.occupant(cell).is_some(),
        );

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
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement
            .planner
            .congestion
            .iter_mut()
            .for_each(|value| *value = 0);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|_| true,
        );

        assert_eq!(next, None);
        assert_eq!(movement.planner.stalled_for.value(0), 1);
    }

    #[test]
    fn plan_next_hop_prefers_lower_congestion_flat_neighbor() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![3, 2, 3, 2, 2, 2, 1, 0, 1], 3, 3);
        let target = CellCoord::new(1, 2);
        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(1, 1));
        let ordered = vec![&bug];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.congestion.resize(9, 0);
        movement.planner.congestion[4] = 4;
        movement.planner.congestion[3] = 3;
        movement.planner.congestion[5] = 1;
        movement.planner.congestion[1] = 5;

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        occupancy_cells[7] = Some(BugId::new(2));
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let occupancy_blocked = occupancy;
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| occupancy_blocked.occupant(cell).is_some(),
        );

        assert_eq!(next, Some(CellCoord::new(2, 1)));
        assert_eq!(movement.planner.stalled_for.value(0), 0);
    }

    #[test]
    fn plan_next_hop_skips_flat_move_to_last_cell() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![3, 2, 3, 2, 2, 2, 1, 0, 1], 3, 3);
        let target = CellCoord::new(1, 2);
        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let bug = bug_snapshot_at(CellCoord::new(1, 1));
        let ordered = vec![&bug];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.congestion.resize(9, 0);
        movement.planner.congestion[4] = 4;
        movement.planner.congestion[3] = 0;
        movement.planner.congestion[5] = 1;
        movement.planner.congestion[1] = 5;

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        occupancy_cells[7] = Some(BugId::new(2));
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);

        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.last_cell.record(0, CellCoord::new(0, 1));
        movement.planner.stalled_for.begin_tick(&ordered);

        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let occupancy_blocked = occupancy;
        let next = movement.planner.plan_next_hop(
            0,
            &bug,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| occupancy_blocked.occupant(cell).is_some(),
        );

        assert_eq!(next, Some(CellCoord::new(2, 1)));
        assert_eq!(movement.planner.stalled_for.value(0), 0);
    }

    #[test]
    fn detour_bfs_finds_progress_cell() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![4, 3, 4, 3, 2, 3, 2, 1, 2], 3, 3);
        let target = CellCoord::new(1, 2);
        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let blocker = bug_snapshot_with_id(0, CellCoord::new(1, 1));
        let seeker = bug_snapshot_with_id(1, CellCoord::new(1, 0));
        let ordered = vec![&blocker, &seeker];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        occupancy_cells[4] = Some(blocker.id);
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);
        let reservation = ReservationLedgerView::from_owned(Vec::new());
        let occupancy_blocked = occupancy;

        let next = movement.planner.plan_next_hop(
            1,
            &seeker,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| cell.column() == 2 || occupancy_blocked.occupant(cell).is_some(),
        );

        assert_eq!(next, Some(CellCoord::new(0, 0)));
        assert_eq!(movement.planner.stalled_for.value(1), 0);
    }

    #[test]
    fn detour_bfs_skips_vacating_intermediate_occupant() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![5, 4, 3, 4, 3, 2, 3, 2, 1], 3, 3);
        let target = CellCoord::new(2, 2);
        let _ = movement
            .planner
            .prepare_workspace(3, 3, &navigation, &[target]);

        let vacating = bug_snapshot_with_id(0, CellCoord::new(1, 0));
        let seeker = bug_snapshot_with_id(1, CellCoord::new(0, 1));
        let ordered = vec![&vacating, &seeker];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.congestion.resize(9, 0);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 9];
        occupancy_cells[1] = Some(vacating.id);
        occupancy_cells[3] = Some(seeker.id);
        let occupancy = OccupancyView::new(&occupancy_cells, 3, 3);
        let reservation = ReservationLedgerView::from_owned(vec![ReservationClaim::new(
            vacating.id,
            Direction::East,
        )]);

        let _ = movement.planner.plan_next_hop(
            1,
            &seeker,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| cell == CellCoord::new(1, 1) || cell == CellCoord::new(0, 2),
        );

        assert!(movement
            .planner
            .detour_queue
            .iter()
            .any(|node| node.cell == CellCoord::new(0, 0)));
        assert!(movement
            .planner
            .detour_queue
            .iter()
            .all(|node| node.cell != CellCoord::new(1, 0)));
    }

    #[test]
    fn plan_next_hop_allows_vacating_occupant() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![3, 2, 1, 2, 1, 0], 1, 6);
        let target = CellCoord::new(0, 5);
        let _ = movement
            .planner
            .prepare_workspace(1, 6, &navigation, &[target]);

        let blocker = bug_snapshot_with_id(0, CellCoord::new(0, 1));
        let seeker = bug_snapshot_with_id(1, CellCoord::new(0, 0));
        let ordered = vec![&blocker, &seeker];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 6];
        occupancy_cells[1] = Some(blocker.id);
        let occupancy = OccupancyView::new(&occupancy_cells, 1, 6);
        let reservation = ReservationLedgerView::from_owned(vec![ReservationClaim::new(
            blocker.id,
            Direction::South,
        )]);
        let occupancy_blocked = occupancy;

        let next = movement.planner.plan_next_hop(
            1,
            &seeker,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| occupancy_blocked.occupant(cell).is_some(),
        );

        assert_eq!(next, Some(CellCoord::new(0, 1)));
        assert_eq!(movement.planner.stalled_for.value(1), 0);
    }

    #[test]
    fn plan_next_hop_skips_cells_reserved_by_lower_bug() {
        let mut movement = Movement::default();
        let navigation = NavigationFieldView::from_owned(vec![2, 1, 3, 2], 2, 2);
        let target = CellCoord::new(1, 0);
        let _ = movement
            .planner
            .prepare_workspace(2, 2, &navigation, &[target]);

        let reserver = bug_snapshot_with_id(0, CellCoord::new(0, 0));
        let seeker = bug_snapshot_with_id(1, CellCoord::new(1, 1));
        let ordered = vec![&reserver, &seeker];
        movement.planner.prepare_per_tick(&ordered, &navigation);
        movement.planner.last_cell.begin_tick(&ordered);
        movement.planner.stalled_for.begin_tick(&ordered);

        let mut occupancy_cells: Vec<Option<BugId>> = vec![None; 4];
        occupancy_cells[0] = Some(reserver.id);
        let occupancy = OccupancyView::new(&occupancy_cells, 2, 2);
        let reservation = ReservationLedgerView::from_owned(vec![ReservationClaim::new(
            reserver.id,
            Direction::East,
        )]);
        let occupancy_blocked = occupancy;

        let next = movement.planner.plan_next_hop(
            1,
            &seeker,
            &ordered,
            &navigation,
            occupancy,
            &reservation,
            &|cell| occupancy_blocked.occupant(cell).is_some(),
        );

        assert_eq!(next, None);
        assert_eq!(movement.planner.stalled_for.value(1), 1);
    }

    fn navigation_stub(width: u32, height: u32) -> NavigationFieldView<'static> {
        let cells = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        NavigationFieldView::from_owned(vec![0; cells], width, height)
    }

    fn bug_snapshot_at(cell: CellCoord) -> BugSnapshot {
        bug_snapshot_with_id(1, cell)
    }

    fn bug_snapshot_with_id(id: u32, cell: CellCoord) -> BugSnapshot {
        BugSnapshot {
            id: BugId::new(id),
            cell,
            color: BugColor::from_rgb(0, 0, 0),
            max_health: Health::new(3),
            health: Health::new(3),
            ready_for_step: true,
            accumulated: Duration::default(),
        }
    }
}
