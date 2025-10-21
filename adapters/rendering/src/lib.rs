#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Shared rendering contracts for Maze Defence adapters.

use anyhow::Result as AnyResult;
use glam::Vec2;
use maze_defence_core::{
    CellCoord, CellRect, PlacementError, PlayMode, RemovalError, TowerId, TowerKind,
};
use std::{error::Error, fmt, time::Duration};

/// RGBA color used when presenting frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red channel intensity in the range 0.0..=1.0.
    pub red: f32,
    /// Green channel intensity in the range 0.0..=1.0.
    pub green: f32,
    /// Blue channel intensity in the range 0.0..=1.0.
    pub blue: f32,
    /// Alpha channel intensity in the range 0.0..=1.0.
    pub alpha: f32,
}

impl Color {
    /// Creates a new color from floating point channels.
    #[must_use]
    pub const fn new(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Creates an opaque color from byte RGB values.
    #[must_use]
    pub const fn from_rgb_u8(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red: red as f32 / 255.0,
            green: green as f32 / 255.0,
            blue: blue as f32 / 255.0,
            alpha: 1.0,
        }
    }

    /// Returns a new color lightened towards white by the provided amount.
    #[must_use]
    pub fn lighten(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);

        Self {
            red: lighten_channel(self.red, amount),
            green: lighten_channel(self.green, amount),
            blue: lighten_channel(self.blue, amount),
            alpha: self.alpha,
        }
    }
}

fn lighten_channel(channel: f32, amount: f32) -> f32 {
    channel + (1.0 - channel) * amount
}

/// Input snapshot gathered by adapters before updating the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameInput {
    /// Whether the adapter detected a toggle press on this frame.
    pub mode_toggle: bool,
    /// Cursor position expressed in world units, clamped to the playable grid bounds.
    pub cursor_world_space: Option<Vec2>,
    /// Cursor position snapped to tile coordinates with half-tile resolution within the playable grid.
    pub cursor_tile_space: Option<TileSpacePosition>,
    /// Size of the active placement preview footprint measured in tile units.
    pub preview_footprint_in_tiles: Vec2,
    /// Whether the adapter detected a placement confirmation on this frame.
    pub confirm_action: bool,
    /// Whether the adapter detected a tower removal request on this frame.
    pub remove_action: bool,
}

impl Default for FrameInput {
    fn default() -> Self {
        Self {
            mode_toggle: false,
            cursor_world_space: None,
            cursor_tile_space: None,
            preview_footprint_in_tiles: Vec2::splat(1.0),
            confirm_action: false,
            remove_action: false,
        }
    }
}

/// Tile-space coordinate pair snapped to half-tile increments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileSpacePosition {
    column_half_steps: u32,
    row_half_steps: u32,
}

impl TileSpacePosition {
    /// Creates a new tile-space position from zero-based integer indices.
    #[must_use]
    pub const fn from_indices(column: u32, row: u32) -> Self {
        Self {
            column_half_steps: column * 2,
            row_half_steps: row * 2,
        }
    }

    /// Creates a new tile-space position expressed in half-tile increments.
    #[must_use]
    pub const fn from_half_steps(column_half_steps: u32, row_half_steps: u32) -> Self {
        Self {
            column_half_steps,
            row_half_steps,
        }
    }

    /// Number of half-tile steps offset along the column axis.
    #[must_use]
    pub const fn column_half_steps(&self) -> u32 {
        self.column_half_steps
    }

    /// Number of half-tile steps offset along the row axis.
    #[must_use]
    pub const fn row_half_steps(&self) -> u32 {
        self.row_half_steps
    }

    /// Position expressed in tile units along the column axis.
    #[must_use]
    pub fn column_in_tiles(&self) -> f32 {
        self.column_half_steps as f32 * 0.5
    }

    /// Position expressed in tile units along the row axis.
    #[must_use]
    pub fn row_in_tiles(&self) -> f32 {
        self.row_half_steps as f32 * 0.5
    }

    /// Returns `true` when the position lies on whole-tile indices.
    #[must_use]
    pub const fn is_integer_aligned(&self) -> bool {
        self.column_half_steps % 2 == 0 && self.row_half_steps % 2 == 0
    }
}

/// Immutable snapshot describing a tower placed within the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneTower {
    /// Identifier allocated to the tower by the world.
    pub id: TowerId,
    /// Kind of tower placed at the associated region.
    pub kind: TowerKind,
    /// Region of cells occupied by the tower.
    pub region: CellRect,
}

impl SceneTower {
    /// Creates a new scene tower descriptor.
    #[must_use]
    pub const fn new(id: TowerId, kind: TowerKind, region: CellRect) -> Self {
        Self { id, kind, region }
    }
}

/// Declarative builder-mode preview emitted by the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TowerPreview {
    /// Kind of tower proposed for placement.
    pub kind: TowerKind,
    /// Region of cells that would be occupied by the tower if constructed.
    pub region: CellRect,
    /// Indicates whether the preview location is valid for placement.
    pub placeable: bool,
    /// Reason reported by the world for rejecting the placement attempt, if any.
    pub rejection: Option<PlacementError>,
}

impl TowerPreview {
    /// Creates a new tower preview descriptor.
    #[must_use]
    pub const fn new(
        kind: TowerKind,
        region: CellRect,
        placeable: bool,
        rejection: Option<PlacementError>,
    ) -> Self {
        let placeable = if rejection.is_some() {
            false
        } else {
            placeable
        };

        Self {
            kind,
            region,
            placeable,
            rejection,
        }
    }
}

/// Feedback surfaced to adapters about the most recent tower interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TowerInteractionFeedback {
    /// Reports that a placement request was rejected by the world.
    PlacementRejected {
        /// Kind of tower requested for placement.
        kind: TowerKind,
        /// Origin cell provided in the placement request.
        origin: CellCoord,
        /// Reason the placement failed.
        reason: PlacementError,
    },
    /// Reports that a tower removal request was rejected by the world.
    RemovalRejected {
        /// Identifier of the tower targeted for removal.
        tower: TowerId,
        /// Reason the removal failed.
        reason: RemovalError,
    },
}

/// Describes a square tile grid that can be rendered by adapters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGridPresentation {
    /// Number of columns contained in the grid.
    pub columns: u32,
    /// Number of rows contained in the grid.
    pub rows: u32,
    /// Side length of a single tile expressed in world units.
    pub tile_length: f32,
    /// Number of cells drawn along each tile edge.
    pub cells_per_tile: u32,
    /// Color used when drawing grid lines.
    pub line_color: Color,
}

impl TileGridPresentation {
    /// Default number of cells drawn along each tile edge.
    pub const DEFAULT_CELLS_PER_TILE: u32 = 4;

    /// Number of cell layers rendered outside the tile grid on each side.
    pub const SIDE_BORDER_CELL_LAYERS: u32 = 1;

    /// Number of cell layers rendered above the tile grid.
    pub const TOP_BORDER_CELL_LAYERS: u32 = 1;

    /// Number of cell layers rendered below the tile grid.
    pub const BOTTOM_BORDER_CELL_LAYERS: u32 = 0;

    /// Creates a new tile grid descriptor.
    ///
    /// Returns an error when `cells_per_tile` is zero.
    #[must_use]
    pub fn new(
        columns: u32,
        rows: u32,
        tile_length: f32,
        cells_per_tile: u32,
        line_color: Color,
    ) -> std::result::Result<Self, RenderingError> {
        if cells_per_tile == 0 {
            return Err(RenderingError::InvalidCellsPerTile { cells_per_tile });
        }

        Ok(Self {
            columns,
            rows,
            tile_length,
            cells_per_tile,
            line_color,
        })
    }

    /// Length of a single cell derived from the tile length.
    #[must_use]
    pub const fn cell_length(&self) -> f32 {
        self.tile_length / self.cells_per_tile as f32
    }

    /// Calculates the total width of the grid.
    #[must_use]
    pub const fn width(&self) -> f32 {
        self.columns as f32 * self.tile_length
    }

    /// Calculates the total height of the grid.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.rows as f32 * self.tile_length
    }

    /// Calculates the total width of the grid including the surrounding cell border.
    #[must_use]
    pub const fn bordered_width(&self) -> f32 {
        self.width() + 2.0 * self.cell_length() * Self::SIDE_BORDER_CELL_LAYERS as f32
    }

    /// Calculates the total height of the grid including the surrounding cell border.
    #[must_use]
    pub const fn bordered_height(&self) -> f32 {
        self.height()
            + self.cell_length()
                * (Self::TOP_BORDER_CELL_LAYERS + Self::BOTTOM_BORDER_CELL_LAYERS) as f32
    }

    /// Clamps a world-space position to the playable grid bounds.
    #[must_use]
    pub fn clamp_world_position(&self, position: Vec2) -> Vec2 {
        if self.columns == 0 || self.rows == 0 {
            return Vec2::ZERO;
        }

        let width = self.width();
        let height = self.height();

        Vec2::new(position.x.clamp(0.0, width), position.y.clamp(0.0, height))
    }

    /// Snaps a world-space position to half-tile increments within the grid bounds.
    ///
    /// The provided `preview_footprint_in_tiles` describes the placement preview's
    /// width and height expressed in whole tiles and is used when clamping the
    /// snapped origin to the grid edges.
    ///
    /// Returns `None` when the position lies outside the grid or the grid has no area.
    #[must_use]
    pub fn snap_world_to_tile(
        &self,
        position: Vec2,
        preview_footprint_in_tiles: Vec2,
    ) -> Option<TileSpacePosition> {
        if self.columns == 0 || self.rows == 0 || self.tile_length <= f32::EPSILON {
            return None;
        }

        let width = self.width();
        let height = self.height();
        if position.x < 0.0 || position.y < 0.0 || position.x > width || position.y > height {
            return None;
        }

        let clamped = self.clamp_world_position(position);
        let column_half_steps = snap_axis_to_half_steps(
            clamped.x / self.tile_length,
            self.columns,
            preview_footprint_in_tiles.x,
        )?;
        let row_half_steps = snap_axis_to_half_steps(
            clamped.y / self.tile_length,
            self.rows,
            preview_footprint_in_tiles.y,
        )?;

        Some(TileSpacePosition::from_half_steps(
            column_half_steps,
            row_half_steps,
        ))
    }
}

fn snap_axis_to_half_steps(
    value_in_tiles: f32,
    tiles: u32,
    preview_size_in_tiles: f32,
) -> Option<u32> {
    if tiles == 0 {
        return None;
    }

    let preview_size_in_tiles = preview_size_in_tiles.max(0.0);
    let half_preview = preview_size_in_tiles * 0.5;
    let min_center = half_preview;
    let max_center = tiles as f32 - half_preview;

    if max_center < min_center {
        return Some(0);
    }

    let snapped_center = (value_in_tiles * 2.0).round() * 0.5;
    let clamped_center = snapped_center.clamp(min_center, max_center);
    let origin = clamped_center - half_preview;

    Some((origin * 2.0).round() as u32)
}

/// Describes an outer wall that should be rendered near the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct WallPresentation {
    /// Thickness of the wall measured in world units.
    pub thickness: f32,
    /// Color used for the wall fill.
    pub color: Color,
    /// Target carved into the wall if one exists.
    pub target: TargetPresentation,
}

impl WallPresentation {
    /// Creates a new wall descriptor.
    #[must_use]
    pub fn new(thickness: f32, color: Color, target: TargetPresentation) -> Self {
        Self {
            thickness,
            color,
            target,
        }
    }
}

/// Target carved into the perimeter wall aligned with the grid cells.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetPresentation {
    /// Cells that compose the target region.
    pub cells: Vec<TargetCellPresentation>,
}

impl TargetPresentation {
    /// Creates a new wall target descriptor.
    #[must_use]
    pub fn new(cells: Vec<TargetCellPresentation>) -> Self {
        Self { cells }
    }

    /// Determines whether the target contains any cells.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// Single cell composing part of a wall target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetCellPresentation {
    /// Column of the cell aligned with the main grid.
    pub column: u32,
    /// Row of the cell relative to the main grid.
    pub row: u32,
}

impl TargetCellPresentation {
    /// Creates a new wall target cell descriptor.
    #[must_use]
    pub const fn new(column: u32, row: u32) -> Self {
        Self { column, row }
    }
}

/// In-game bug rendered as a filled circle scaled to a single cell.
///
/// Bug coordinates are expressed in cell units derived from the tile grid's
/// [`cells_per_tile`](TileGridPresentation::cells_per_tile) configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BugPresentation {
    /// Zero-based column index of the grid cell that contains the bug.
    pub column: u32,
    /// Zero-based row index of the grid cell that contains the bug.
    pub row: u32,
    /// Fill color of the bug's body.
    pub color: Color,
}

impl BugPresentation {
    /// Creates a new bug presentation descriptor.
    #[must_use]
    pub const fn new(column: u32, row: u32, color: Color) -> Self {
        Self { column, row, color }
    }
}

/// Scene description combining the tile grid, outer wall and inhabitants.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Tile grid that composes the main play area.
    pub tile_grid: TileGridPresentation,
    /// Wall drawn outside the play area.
    pub wall: WallPresentation,
    /// Bugs currently visible within the maze, positioned using cell coordinates.
    pub bugs: Vec<BugPresentation>,
    /// Towers currently visible within the maze.
    pub towers: Vec<SceneTower>,
    /// Active play mode for the simulation.
    pub play_mode: PlayMode,
    /// Optional builder placement preview emitted by the simulation.
    pub tower_preview: Option<TowerPreview>,
    /// Feedback about the last tower placement/removal attempt.
    pub tower_feedback: Option<TowerInteractionFeedback>,
}

impl Scene {
    /// Creates a new scene descriptor.
    #[must_use]
    pub fn new(
        tile_grid: TileGridPresentation,
        wall: WallPresentation,
        bugs: Vec<BugPresentation>,
        towers: Vec<SceneTower>,
        play_mode: PlayMode,
        tower_preview: Option<TowerPreview>,
        tower_feedback: Option<TowerInteractionFeedback>,
    ) -> Self {
        Self {
            tile_grid,
            wall,
            bugs,
            towers,
            play_mode,
            tower_preview,
            tower_feedback,
        }
    }

    /// Height of the entire scene including the wall.
    #[must_use]
    pub fn total_height(&self) -> f32 {
        self.tile_grid.bordered_height() + self.wall.thickness
    }
}

/// Presentation descriptor consumed by rendering backends.
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    /// Title used by the created window.
    pub window_title: String,
    /// Solid color used to clear each frame.
    pub clear_color: Color,
    /// Scene content that should be displayed.
    pub scene: Scene,
}

impl Presentation {
    /// Constructs a new presentation descriptor.
    #[must_use]
    pub fn new<T>(window_title: T, clear_color: Color, scene: Scene) -> Self
    where
        T: Into<String>,
    {
        Self {
            window_title: window_title.into(),
            clear_color,
            scene,
        }
    }
}

/// Rendering backend capable of presenting Maze Defence scenes.
pub trait RenderingBackend {
    /// Runs the rendering backend until it is requested to exit.
    ///
    /// The provided `update_scene` closure receives the simulated frame delta,
    /// per-frame input captured by the adapter, and may mutate the scene before
    /// it is rendered, allowing adapters to animate world snapshots
    /// deterministically.
    fn run<F>(self, presentation: Presentation, update_scene: F) -> AnyResult<()>
    where
        F: FnMut(Duration, FrameInput, &mut Scene) + 'static;
}

/// Errors that can occur when constructing rendering descriptors.
#[derive(Debug, PartialEq, Eq)]
pub enum RenderingError {
    /// Cells per tile must be positive to avoid a zero-sized cell.
    InvalidCellsPerTile {
        /// Provided cell count that failed validation.
        cells_per_tile: u32,
    },
}

impl fmt::Display for RenderingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCellsPerTile { cells_per_tile } => {
                write!(
                    f,
                    "cells_per_tile must be positive (received {cells_per_tile})"
                )
            }
        }
    }
}

impl Error for RenderingError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_grid_creation_accepts_positive_cells_per_tile() {
        let presentation = TileGridPresentation::new(10, 5, 32.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("positive cells_per_tile should succeed");

        assert_eq!(presentation.cells_per_tile, 4);
    }

    #[test]
    fn tile_grid_creation_rejects_zero_cells_per_tile_without_panicking() {
        let error = TileGridPresentation::new(10, 5, 32.0, 0, Color::from_rgb_u8(0, 0, 0))
            .expect_err("zero cells_per_tile must be rejected");

        assert!(matches!(
            error,
            RenderingError::InvalidCellsPerTile { cells_per_tile: 0 }
        ));
    }

    #[test]
    fn clamp_world_position_limits_coordinates_to_grid_bounds() {
        let presentation = TileGridPresentation::new(5, 4, 32.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let clamped = presentation.clamp_world_position(Vec2::new(-10.0, 170.0));

        assert_eq!(clamped, Vec2::new(0.0, presentation.height()));
    }

    #[test]
    fn snap_world_to_tile_snaps_to_half_tile_increments() {
        let presentation = TileGridPresentation::new(6, 3, 24.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let snapped = presentation
            .snap_world_to_tile(Vec2::new(24.0, 24.0), Vec2::splat(1.0))
            .expect("position inside grid should snap");

        assert_eq!(snapped.column_half_steps(), 1);
        assert_eq!(snapped.row_half_steps(), 1);
        assert!(!snapped.is_integer_aligned());
    }

    #[test]
    fn snap_world_to_tile_clamps_preview_to_grid_bounds() {
        let presentation = TileGridPresentation::new(6, 3, 24.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let snapped = presentation
            .snap_world_to_tile(Vec2::new(143.9, 71.2), Vec2::splat(1.0))
            .expect("position inside grid should snap");

        assert_eq!(snapped.column_half_steps(), 10);
        assert_eq!(snapped.row_half_steps(), 4);
        assert!(snapped.is_integer_aligned());
    }

    #[test]
    fn snap_world_to_tile_rejects_positions_outside_grid() {
        let presentation = TileGridPresentation::new(3, 2, 16.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");

        assert!(presentation
            .snap_world_to_tile(Vec2::new(100.0, 10.0), Vec2::splat(1.0))
            .is_none());
        assert!(presentation
            .snap_world_to_tile(Vec2::new(10.0, 100.0), Vec2::splat(1.0))
            .is_none());
    }

    #[test]
    fn snap_world_to_tile_respects_preview_footprint() {
        let presentation = TileGridPresentation::new(4, 3, 20.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let footprint = Vec2::new(1.5, 0.5);
        let snapped = presentation
            .snap_world_to_tile(Vec2::new(5.0, 55.0), footprint)
            .expect("position inside grid should snap");

        let origin_column_tiles = snapped.column_half_steps() as f32 * 0.5;
        let origin_row_tiles = snapped.row_half_steps() as f32 * 0.5;

        assert!(origin_column_tiles >= 0.0);
        assert!(origin_row_tiles >= 0.0);
        assert!(origin_column_tiles + footprint.x <= presentation.columns as f32);
        assert!(origin_row_tiles + footprint.y <= presentation.rows as f32);
    }

    #[test]
    fn scene_new_does_not_inject_builder_defaults() {
        let tile_grid = TileGridPresentation::new(
            6,
            4,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(64, 64, 64),
        )
        .expect("default cells_per_tile is valid");
        let wall = WallPresentation::new(
            8.0,
            Color::from_rgb_u8(128, 128, 128),
            TargetPresentation::new(vec![]),
        );
        let bugs = vec![BugPresentation::new(2, 3, Color::from_rgb_u8(255, 0, 0))];

        let scene = Scene::new(
            tile_grid.clone(),
            wall.clone(),
            bugs.clone(),
            Vec::new(),
            PlayMode::Attack,
            None,
            None,
        );

        assert_eq!(scene.tile_grid, tile_grid);
        assert_eq!(scene.wall, wall);
        assert_eq!(scene.bugs, bugs);
        assert_eq!(scene.play_mode, PlayMode::Attack);
        assert!(scene.tower_preview.is_none());
        assert!(scene.towers.is_empty());
        assert!(scene.tower_feedback.is_none());
    }

    #[test]
    fn scene_new_preserves_builder_preview() {
        let tile_grid = TileGridPresentation::new(
            5,
            5,
            24.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(32, 32, 32),
        )
        .expect("default cells_per_tile is valid");
        let wall = WallPresentation::new(
            6.0,
            Color::from_rgb_u8(90, 90, 90),
            TargetPresentation::new(vec![]),
        );
        let preview_region = CellRect::from_origin_and_size(
            maze_defence_core::CellCoord::new(4, 6),
            maze_defence_core::CellRectSize::new(2, 2),
        );
        let placement_preview = TowerPreview::new(
            TowerKind::Basic,
            preview_region,
            false,
            Some(PlacementError::Occupied),
        );

        let scene = Scene::new(
            tile_grid.clone(),
            wall.clone(),
            vec![],
            vec![SceneTower::new(
                TowerId::new(1),
                TowerKind::Basic,
                preview_region,
            )],
            PlayMode::Builder,
            Some(placement_preview),
            Some(TowerInteractionFeedback::PlacementRejected {
                kind: TowerKind::Basic,
                origin: maze_defence_core::CellCoord::new(4, 6),
                reason: PlacementError::Occupied,
            }),
        );

        assert_eq!(scene.play_mode, PlayMode::Builder);
        assert_eq!(scene.tower_preview, Some(placement_preview));
        assert_eq!(scene.towers.len(), 1);
        assert_eq!(scene.tile_grid, tile_grid);
        assert_eq!(scene.wall, wall);
        assert_eq!(
            scene.tower_feedback,
            Some(TowerInteractionFeedback::PlacementRejected {
                kind: TowerKind::Basic,
                origin: maze_defence_core::CellCoord::new(4, 6),
                reason: PlacementError::Occupied,
            })
        );
    }
}
