//! Static navigation field builder used by the world crate.

use std::collections::VecDeque;

use maze_defence_core::CellCoord;

/// Dense Manhattan-distance grid seeded from the maze exits.
///
/// The field mirrors the world's occupancy dimensions, including the hidden
/// exit row, and stores the reverse breadth-first search results that drive the
/// crowd planner. Distances default to `u16::MAX` for unreachable cells so
/// callers can distinguish walls from traversable tiles.
#[derive(Clone, Debug, Default)]
pub(crate) struct NavigationField {
    width: u32,
    height: u32,
    distances: Vec<u16>,
}

impl NavigationField {
    /// Rebuilds the navigation distances using a reverse breadth-first search.
    pub(crate) fn rebuild_with<F>(
        &mut self,
        width: u32,
        height: u32,
        exits: &[CellCoord],
        mut is_blocked: F,
    ) where
        F: FnMut(CellCoord) -> bool,
    {
        let width_usize = usize::try_from(width).unwrap_or(0);
        let height_usize = usize::try_from(height).unwrap_or(0);
        let cell_count = width_usize.checked_mul(height_usize).unwrap_or(0);

        if cell_count == 0 {
            self.width = width;
            self.height = height;
            self.distances.clear();
            return;
        }

        if self.distances.len() != cell_count {
            self.distances = vec![u16::MAX; cell_count];
        } else {
            self.distances.fill(u16::MAX);
        }

        self.width = width;
        self.height = height;

        let mut queue = VecDeque::new();

        for &exit in exits {
            if exit.column() >= width || exit.row() >= height {
                continue;
            }

            if is_blocked(exit) {
                continue;
            }

            if let Some(index) = index(width_usize, exit) {
                if self.distances[index] == 0 {
                    continue;
                }

                self.distances[index] = 0;
                queue.push_back(exit);
            }
        }

        while let Some(cell) = queue.pop_front() {
            let Some(current_index) = index(width_usize, cell) else {
                continue;
            };
            let current_distance = self.distances[current_index];

            if current_distance >= u16::MAX.saturating_sub(1) {
                continue;
            }

            let next_distance = current_distance + 1;

            for neighbor in neighbors(cell, width, height) {
                if is_blocked(neighbor) {
                    continue;
                }

                let Some(neighbor_index) = index(width_usize, neighbor) else {
                    continue;
                };

                if self.distances[neighbor_index] <= next_distance {
                    continue;
                }

                self.distances[neighbor_index] = next_distance;
                queue.push_back(neighbor);
            }
        }
    }

    /// Width of the navigation field in cells.
    #[must_use]
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    /// Height of the navigation field in cells.
    #[must_use]
    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    /// Dense navigation distances stored in row-major order.
    #[must_use]
    pub(crate) fn cells(&self) -> &[u16] {
        &self.distances
    }

    /// Distance captured for the provided cell, if it lies within the field.
    #[must_use]
    pub(crate) fn distance(&self, cell: CellCoord) -> Option<u16> {
        if cell.column() >= self.width || cell.row() >= self.height {
            return None;
        }

        let width = usize::try_from(self.width).ok()?;
        index(width, cell).and_then(|offset| self.distances.get(offset).copied())
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

fn index(width: usize, cell: CellCoord) -> Option<usize> {
    let column = usize::try_from(cell.column()).ok()?;
    let row = usize::try_from(cell.row()).ok()?;
    row.checked_mul(width)?.checked_add(column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_with_sets_exit_cells_to_zero() {
        let mut field = NavigationField::default();
        let exits = [CellCoord::new(1, 2)];

        field.rebuild_with(3, 4, &exits, |_| false);

        assert_eq!(field.distance(CellCoord::new(1, 2)), Some(0));
        assert_eq!(field.distance(CellCoord::new(1, 1)), Some(1));
        assert_eq!(field.distance(CellCoord::new(1, 0)), Some(2));
        assert_eq!(field.distance(CellCoord::new(0, 0)), Some(3));
    }

    #[test]
    fn rebuild_with_respects_walls() {
        let mut field = NavigationField::default();
        let exits = [CellCoord::new(1, 2)];
        let wall = CellCoord::new(1, 1);

        field.rebuild_with(3, 4, &exits, |cell| cell == wall);

        assert_eq!(field.distance(wall), Some(u16::MAX));
        assert_eq!(field.distance(CellCoord::new(1, 0)), Some(4));
        assert_eq!(field.distance(CellCoord::new(0, 1)), Some(2));
    }
}
