use std::collections::VecDeque;

use maze_defence_core::{AnalyticsLayoutSnapshot, CellCoord, NavigationFieldView};

use crate::AnalyticsScratch;

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
    use super::select_shortest_navigation_path;
    use crate::AnalyticsScratch;
    use maze_defence_core::{AnalyticsLayoutSnapshot, CellCoord, NavigationFieldView};
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
}
