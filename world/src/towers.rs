//! Authoritative tower state management utilities.

use std::collections::BTreeMap;

use maze_defence_core::{CellRect, CellRectSize, TowerId, TowerKind};

/// Snapshot of a tower stored inside the world.
#[derive(Clone, Debug)]
pub(crate) struct TowerState {
    /// Identifier allocated by the world for the tower.
    pub(crate) id: TowerId,
    /// Kind of tower that was constructed.
    pub(crate) kind: TowerKind,
    /// Region of cells occupied by the tower.
    pub(crate) region: CellRect,
}

/// Registry that stores towers and manages identifier allocation.
#[derive(Debug)]
pub(crate) struct TowerRegistry {
    entries: BTreeMap<TowerId, TowerState>,
    next_tower_id: TowerId,
}

impl TowerRegistry {
    /// Creates an empty tower registry with a reset identifier counter.
    pub(crate) fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_tower_id: TowerId::new(0),
        }
    }

    /// Allocates a fresh tower identifier.
    pub(crate) fn allocate(&mut self) -> TowerId {
        let id = self.next_tower_id;
        let next = self.next_tower_id.get().saturating_add(1);
        self.next_tower_id = TowerId::new(next);
        id
    }

    /// Inserts the provided tower state into the registry.
    pub(crate) fn insert(&mut self, state: TowerState) {
        let previous = self.entries.insert(state.id, state);
        debug_assert!(previous.is_none());
    }

    /// Retrieves the tower state associated with the identifier, if present.
    pub(crate) fn get(&self, id: TowerId) -> Option<&TowerState> {
        self.entries.get(&id)
    }

    /// Removes the tower associated with the identifier, returning its state.
    pub(crate) fn remove(&mut self, id: TowerId) -> Option<TowerState> {
        self.entries.remove(&id)
    }

    /// Reports whether the registry currently stores any towers.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over all tower states in identifier order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &TowerState> {
        self.entries.values()
    }
}

/// Reports the footprint size associated with a tower kind.
pub(crate) fn footprint_for(kind: TowerKind) -> CellRectSize {
    match kind {
        TowerKind::Basic => CellRectSize::new(2, 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::CellCoord;

    #[test]
    fn basic_tower_footprint_is_two_by_two() {
        let footprint = footprint_for(TowerKind::Basic);
        assert_eq!(footprint.width(), 2);
        assert_eq!(footprint.height(), 2);
    }

    #[test]
    fn registry_starts_empty_with_zero_identifier() {
        let registry = TowerRegistry::new();
        assert!(registry.entries.is_empty());
        assert_eq!(registry.next_tower_id.get(), 0);
    }

    #[test]
    fn allocating_identifiers_advances_counter() {
        let mut registry = TowerRegistry::new();
        let first = registry.allocate();
        let second = registry.allocate();

        assert_eq!(first, TowerId::new(0));
        assert_eq!(second, TowerId::new(1));
        assert_eq!(registry.next_tower_id.get(), 2);
    }

    #[test]
    fn insert_and_remove_round_trip_restores_state() {
        let mut registry = TowerRegistry::new();
        let id = registry.allocate();
        let region = CellRect::from_origin_and_size(CellCoord::new(2, 3), CellRectSize::new(2, 2));
        registry.insert(TowerState {
            id,
            kind: TowerKind::Basic,
            region,
        });

        let retrieved = registry.get(id).expect("tower present");
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.region, region);

        let removed = registry.remove(id).expect("tower present");
        assert_eq!(removed.id, id);
        assert!(registry.is_empty());
    }

    #[test]
    fn tower_state_preserves_constructor_fields() {
        let region = CellRect::from_origin_and_size(CellCoord::new(1, 2), CellRectSize::new(2, 3));
        let state = TowerState {
            id: TowerId::new(7),
            kind: TowerKind::Basic,
            region,
        };

        assert_eq!(state.id, TowerId::new(7));
        assert_eq!(state.kind, TowerKind::Basic);
        assert_eq!(state.region, region);
    }
}
