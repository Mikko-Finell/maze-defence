#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Pure builder-mode system responsible for emitting tower placement and removal commands.

use maze_defence_core::{CellCoord, CellRect, Command, Event, PlayMode, TowerId, TowerKind};

/// Declarative placement preview describing a potential tower construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementPreview {
    /// Kind of tower proposed for placement.
    pub kind: TowerKind,
    /// Origin cell anchoring the proposed tower footprint.
    pub origin: CellCoord,
    /// Region of cells that would be occupied by the tower if placed.
    pub region: CellRect,
    /// Indicates whether the preview represents a valid placement location.
    pub placeable: bool,
}

impl PlacementPreview {
    /// Creates a new placement preview descriptor.
    #[must_use]
    pub const fn new(
        kind: TowerKind,
        origin: CellCoord,
        region: CellRect,
        placeable: bool,
    ) -> Self {
        Self {
            kind,
            origin,
            region,
            placeable,
        }
    }
}

/// Input snapshot distilled from adapter-provided frame input data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuilderInput {
    /// Indicates whether the player confirmed a placement on this frame.
    pub confirm_action: bool,
    /// Indicates whether the player requested tower removal on this frame.
    pub remove_action: bool,
    /// Cell currently hovered by the cursor in builder mode.
    pub cursor_cell: Option<CellCoord>,
}

impl BuilderInput {
    /// Creates a new input descriptor with explicit field values.
    #[must_use]
    pub const fn new(
        confirm_action: bool,
        remove_action: bool,
        cursor_cell: Option<CellCoord>,
    ) -> Self {
        Self {
            confirm_action,
            remove_action,
            cursor_cell,
        }
    }
}

impl Default for BuilderInput {
    fn default() -> Self {
        Self {
            confirm_action: false,
            remove_action: false,
            cursor_cell: None,
        }
    }
}

/// Builder-mode system that translates preview + input into placement commands.
#[derive(Debug, Clone)]
pub struct Builder {
    play_mode: PlayMode,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            play_mode: PlayMode::Attack,
        }
    }
}

impl Builder {
    /// Creates a new builder system instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            play_mode: PlayMode::Attack,
        }
    }

    /// Consumes world events and adapter-derived input to emit builder commands.
    ///
    /// The `tower_at` closure should mirror the semantics of the world's
    /// `query::tower_at` helper so the system can identify the hovered tower.
    pub fn handle<F>(
        &mut self,
        events: &[Event],
        preview: Option<PlacementPreview>,
        input: BuilderInput,
        mut tower_at: F,
        out: &mut Vec<Command>,
    ) where
        F: FnMut(CellCoord) -> Option<TowerId>,
    {
        for event in events {
            if let Event::PlayModeChanged { mode } = event {
                self.play_mode = *mode;
            }
        }

        if self.play_mode != PlayMode::Builder {
            return;
        }

        if input.confirm_action {
            if let Some(preview) = preview {
                if preview.placeable {
                    out.push(Command::PlaceTower {
                        kind: preview.kind,
                        origin: preview.origin,
                    });
                }
            }
        }

        if input.remove_action {
            if let Some(cell) = input.cursor_cell {
                if let Some(tower) = tower_at(cell) {
                    out.push(Command::RemoveTower { tower });
                }
            }
        }
    }
}
