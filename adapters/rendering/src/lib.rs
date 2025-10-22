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
    BugId, CellCoord, CellRect, PlacementError, PlayMode, ProjectileId, RemovalError, TowerId,
    TowerKind,
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
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FrameInput {
    /// Whether the adapter detected a toggle press on this frame.
    pub mode_toggle: bool,
    /// Cursor position expressed in world units, clamped to the playable grid bounds.
    pub cursor_world_space: Option<Vec2>,
    /// Cursor position snapped to tile coordinates with adapter-provided subdivision resolution.
    pub cursor_tile_space: Option<TileSpacePosition>,
    /// Whether the adapter detected a placement confirmation on this frame.
    pub confirm_action: bool,
    /// Whether the adapter detected a tower removal request on this frame.
    pub remove_action: bool,
}

/// Per-frame diagnostics emitted by simulations to help adapters report performance breakdowns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSimulationBreakdown {
    /// Total time spent advancing the simulation this frame.
    pub simulation: Duration,
    /// Portion of the simulation advance dedicated to pathfinding.
    pub pathfinding: Duration,
    /// Time spent translating simulation state into renderable scene data.
    pub scene_population: Duration,
}

impl FrameSimulationBreakdown {
    /// Creates a new diagnostics struct populated with the provided durations.
    #[must_use]
    pub const fn new(
        simulation: Duration,
        pathfinding: Duration,
        scene_population: Duration,
    ) -> Self {
        Self {
            simulation,
            pathfinding,
            scene_population,
        }
    }
}

/// Tile-space coordinate pair snapped to deterministic sub-tile increments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileSpacePosition {
    column_steps: u32,
    row_steps: u32,
    steps_per_tile: u32,
}

impl TileSpacePosition {
    /// Creates a new tile-space position from zero-based integer tile indices.
    #[must_use]
    pub const fn from_indices(column: u32, row: u32) -> Self {
        Self::new(column, row, 1)
    }

    /// Creates a new tile-space position expressed in arbitrary sub-tile increments.
    #[must_use]
    pub const fn from_steps(column_steps: u32, row_steps: u32, steps_per_tile: u32) -> Self {
        Self::new(column_steps, row_steps, steps_per_tile)
    }

    /// Creates a new tile-space position expressed in arbitrary sub-tile increments.
    #[must_use]
    pub const fn new(column_steps: u32, row_steps: u32, steps_per_tile: u32) -> Self {
        let steps_per_tile = if steps_per_tile == 0 {
            1
        } else {
            steps_per_tile
        };
        Self {
            column_steps,
            row_steps,
            steps_per_tile,
        }
    }

    /// Number of sub-tile steps offset along the column axis.
    #[must_use]
    pub const fn column_steps(&self) -> u32 {
        self.column_steps
    }

    /// Number of sub-tile steps offset along the row axis.
    #[must_use]
    pub const fn row_steps(&self) -> u32 {
        self.row_steps
    }

    /// Number of sub-tile steps that compose a full tile along each axis.
    #[must_use]
    pub const fn steps_per_tile(&self) -> u32 {
        if self.steps_per_tile == 0 {
            1
        } else {
            self.steps_per_tile
        }
    }

    /// Position expressed in tile units along the column axis.
    #[must_use]
    pub fn column_in_tiles(&self) -> f32 {
        self.column_steps as f32 / self.steps_per_tile() as f32
    }

    /// Position expressed in tile units along the row axis.
    #[must_use]
    pub fn row_in_tiles(&self) -> f32 {
        self.row_steps as f32 / self.steps_per_tile() as f32
    }

    /// Returns `true` when the position lies on whole-tile indices.
    #[must_use]
    pub const fn is_integer_aligned(&self) -> bool {
        let steps_per_tile = if self.steps_per_tile == 0 {
            1
        } else {
            self.steps_per_tile
        };
        self.column_steps % steps_per_tile == 0 && self.row_steps % steps_per_tile == 0
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

/// Cell-sized wall rendered inside the maze interior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneWall {
    /// Zero-based column index of the cell guarded by the wall.
    pub column: u32,
    /// Zero-based row index of the cell guarded by the wall.
    pub row: u32,
}

impl SceneWall {
    /// Creates a new scene wall descriptor.
    #[must_use]
    pub const fn new(column: u32, row: u32) -> Self {
        Self { column, row }
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
    ///
    /// The bottom border represents the visible perimeter wall row rendered
    /// beneath the playable tiles.
    pub const BOTTOM_BORDER_CELL_LAYERS: u32 = 1;

    /// Creates a new tile grid descriptor.
    ///
    /// Returns an error when `cells_per_tile` is zero.
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

    /// Snaps a world-space position to deterministic sub-tile increments within the grid bounds.
    ///
    /// Returns `None` when the position lies outside the grid or the grid has no area.
    #[must_use]
    pub fn snap_world_to_tile(
        &self,
        position: Vec2,
        footprint_in_tiles: Vec2,
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
        let steps_per_tile = self.cells_per_tile.max(1);
        let column_steps = snap_axis_to_steps(
            clamped.x / self.tile_length,
            self.columns,
            footprint_in_tiles.x,
            steps_per_tile,
        )?;
        let row_steps = snap_axis_to_steps(
            clamped.y / self.tile_length,
            self.rows,
            footprint_in_tiles.y,
            steps_per_tile,
        )?;

        Some(TileSpacePosition::from_steps(
            column_steps,
            row_steps,
            steps_per_tile,
        ))
    }
}

fn snap_axis_to_steps(
    value_in_tiles: f32,
    tiles: u32,
    footprint_in_tiles: f32,
    steps_per_tile: u32,
) -> Option<u32> {
    if tiles == 0 || steps_per_tile == 0 {
        return None;
    }

    let total_steps = tiles.saturating_mul(steps_per_tile);
    if total_steps == 0 {
        return None;
    }

    let preview_size = (footprint_in_tiles * steps_per_tile as f32).max(0.0);
    let half_preview = preview_size * 0.5;
    let min_center = half_preview;
    let max_center = total_steps as f32 - half_preview;

    if max_center < min_center {
        return Some(0);
    }

    let value_in_steps = value_in_tiles * steps_per_tile as f32;
    let snapped_center = value_in_steps.round();
    let clamped_center = snapped_center.clamp(min_center, max_center);
    let origin = clamped_center - half_preview;

    let clamped_origin = origin.max(0.0).min(total_steps as f32);
    Some(clamped_origin.round() as u32)
}

/// In-game bug rendered as a filled circle scaled to a single cell.
///
/// Bug coordinates are expressed in cell units derived from the tile grid's
/// [`cells_per_tile`](TileGridPresentation::cells_per_tile) configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BugPresentation {
    /// Bug position expressed in cell-space coordinates.
    pub position: Vec2,
    /// Fill color of the bug's body.
    pub color: Color,
    /// Health configuration used to draw the bug's health bar.
    pub health: BugHealthPresentation,
}

impl BugPresentation {
    /// Creates a new bug presentation descriptor.
    #[must_use]
    pub fn new(position: Vec2, color: Color, health: BugHealthPresentation) -> Self {
        Self {
            position,
            color,
            health,
        }
    }
}

/// Health values required to render a bug's health bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BugHealthPresentation {
    /// Current health remaining for the bug.
    pub current: u32,
    /// Maximum health the bug started with.
    pub maximum: u32,
}

impl BugHealthPresentation {
    /// Creates a new bug health descriptor.
    #[must_use]
    pub fn new(current: u32, maximum: u32) -> Self {
        Self { current, maximum }
    }
}

/// Cell-space line segment describing an active tower targeting beam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TowerTargetLine {
    /// Identifier of the tower emitting the beam.
    pub tower: TowerId,
    /// Identifier of the bug being tracked by the tower.
    pub bug: BugId,
    /// Start of the beam expressed in cell coordinates.
    pub from: Vec2,
    /// End of the beam expressed in cell coordinates.
    pub to: Vec2,
}

impl TowerTargetLine {
    /// Creates a new tower targeting beam descriptor.
    #[must_use]
    pub fn new(tower: TowerId, bug: BugId, from: Vec2, to: Vec2) -> Self {
        Self {
            tower,
            bug,
            from,
            to,
        }
    }
}

/// Projectile currently travelling between a tower and its cached target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneProjectile {
    /// Identifier allocated to the projectile by the world.
    pub id: ProjectileId,
    /// Cached origin of the projectile expressed in cell coordinates.
    pub from: Vec2,
    /// Cached destination expressed in cell coordinates.
    pub to: Vec2,
    /// Current projectile position expressed in cell coordinates.
    pub position: Vec2,
    /// Normalised travel progress in the inclusive range `0.0..=1.0`.
    pub progress: f32,
}

impl SceneProjectile {
    /// Creates a new projectile scene descriptor.
    #[must_use]
    pub fn new(id: ProjectileId, from: Vec2, to: Vec2, position: Vec2, progress: f32) -> Self {
        Self {
            id,
            from,
            to,
            position,
            progress,
        }
    }
}

/// Scene description combining the tile grid, perimeter wall colour and inhabitants.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Tile grid that composes the main play area.
    pub tile_grid: TileGridPresentation,
    /// Color applied to perimeter cell walls.
    pub wall_color: Color,
    /// Cell-sized walls populating the maze interior.
    pub walls: Vec<SceneWall>,
    /// Bugs currently visible within the maze, positioned using cell coordinates.
    pub bugs: Vec<BugPresentation>,
    /// Towers currently visible within the maze.
    pub towers: Vec<SceneTower>,
    /// Projectiles currently travelling across the maze.
    pub projectiles: Vec<SceneProjectile>,
    /// Targeting beams emitted by towers while in attack mode.
    pub tower_targets: Vec<TowerTargetLine>,
    /// Active play mode for the simulation.
    pub play_mode: PlayMode,
    /// Optional builder placement preview emitted by the simulation.
    pub tower_preview: Option<TowerPreview>,
    /// Footprint of the currently selected tower expressed in tile units.
    pub active_tower_footprint_tiles: Option<Vec2>,
    /// Feedback about the last tower placement/removal attempt.
    pub tower_feedback: Option<TowerInteractionFeedback>,
}

impl Scene {
    /// Creates a new scene descriptor.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // Scene construction intentionally enumerates every channel explicitly.
    pub fn new(
        tile_grid: TileGridPresentation,
        wall_color: Color,
        walls: Vec<SceneWall>,
        bugs: Vec<BugPresentation>,
        towers: Vec<SceneTower>,
        projectiles: Vec<SceneProjectile>,
        tower_targets: Vec<TowerTargetLine>,
        play_mode: PlayMode,
        tower_preview: Option<TowerPreview>,
        active_tower_footprint_tiles: Option<Vec2>,
        tower_feedback: Option<TowerInteractionFeedback>,
    ) -> Self {
        Self {
            tile_grid,
            wall_color,
            walls,
            bugs,
            towers,
            projectiles,
            tower_targets,
            play_mode,
            tower_preview,
            active_tower_footprint_tiles,
            tower_feedback,
        }
    }

    /// Height of the entire scene including the wall.
    #[must_use]
    pub fn total_height(&self) -> f32 {
        self.tile_grid.bordered_height()
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
        F: FnMut(Duration, FrameInput, &mut Scene) -> FrameSimulationBreakdown + 'static;
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
    fn tile_grid_bordered_height_includes_visible_wall_row() {
        let presentation = TileGridPresentation::new(3, 2, 32.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let expected_border = presentation.cell_length()
            * (TileGridPresentation::TOP_BORDER_CELL_LAYERS
                + TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS) as f32;

        assert_eq!(
            presentation.bordered_height(),
            presentation.height() + expected_border
        );
    }

    #[test]
    fn tile_grid_bottom_border_scales_with_cells_per_tile() {
        let columns = 4;
        let rows = 3;
        let tile_length = 48.0;
        let color = Color::from_rgb_u8(0, 0, 0);

        for cells_per_tile in [1, 2, 3, 4] {
            let presentation =
                TileGridPresentation::new(columns, rows, tile_length, cells_per_tile, color)
                    .expect("cells_per_tile must be positive");

            let total_border_height = presentation.bordered_height() - presentation.height();
            let top_border_height =
                presentation.cell_length() * TileGridPresentation::TOP_BORDER_CELL_LAYERS as f32;
            let bottom_border_height = total_border_height - top_border_height;
            let expected_bottom_border =
                presentation.cell_length() * TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS as f32;

            assert!(
                (bottom_border_height - expected_bottom_border).abs() <= f32::EPSILON,
                "bottom border must span {} cell layer(s) for cells_per_tile {}",
                TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS,
                cells_per_tile
            );

            let measured_layers = bottom_border_height / presentation.cell_length();
            assert!(
                (measured_layers - TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS as f32).abs()
                    <= f32::EPSILON,
                "bottom border must measure {} layer(s), found {measured_layers}",
                TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS
            );
        }
    }

    #[test]
    fn clamp_world_position_limits_coordinates_to_grid_bounds() {
        let presentation = TileGridPresentation::new(5, 4, 32.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let clamped = presentation.clamp_world_position(Vec2::new(-10.0, 170.0));

        assert_eq!(clamped, Vec2::new(0.0, presentation.height()));
    }

    #[test]
    fn snap_world_to_tile_snaps_to_cell_increments() {
        let presentation = TileGridPresentation::new(6, 3, 24.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let snapped = presentation
            .snap_world_to_tile(Vec2::new(24.0, 24.0), Vec2::splat(1.0))
            .expect("position inside grid should snap");

        assert_eq!(snapped.steps_per_tile(), 4);
        assert_eq!(snapped.column_steps(), 2);
        assert_eq!(snapped.row_steps(), 2);
        assert!(!snapped.is_integer_aligned());
    }

    #[test]
    fn snap_world_to_tile_clamps_preview_to_grid_bounds() {
        let presentation = TileGridPresentation::new(6, 3, 24.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("valid grid");
        let footprint = Vec2::new(1.5, 0.5);
        let snapped = presentation
            .snap_world_to_tile(Vec2::new(143.9, 71.2), footprint)
            .expect("position inside grid should snap");

        assert_eq!(snapped.steps_per_tile(), 4);
        assert_eq!(snapped.column_steps(), 18);
        assert_eq!(snapped.row_steps(), 10);
        let origin_column_tiles = snapped.column_in_tiles();
        let origin_row_tiles = snapped.row_in_tiles();
        assert!(origin_column_tiles >= 0.0);
        assert!(origin_row_tiles >= 0.0);
        assert!(origin_column_tiles + footprint.x <= presentation.columns as f32 + 1e-5);
        assert!(origin_row_tiles + footprint.y <= presentation.rows as f32 + 1e-5);
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
    fn scene_new_does_not_inject_builder_defaults() {
        let tile_grid = TileGridPresentation::new(
            6,
            4,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(64, 64, 64),
        )
        .expect("default cells_per_tile is valid");
        let wall_color = Color::from_rgb_u8(128, 128, 128);
        let bugs = vec![BugPresentation::new(
            Vec2::new(2.0, 3.0),
            Color::from_rgb_u8(255, 0, 0),
            BugHealthPresentation::new(3, 3),
        )];

        let scene = Scene::new(
            tile_grid,
            wall_color,
            Vec::new(),
            bugs.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            PlayMode::Attack,
            None,
            None,
            None,
        );

        assert_eq!(scene.tile_grid, tile_grid);
        assert_eq!(scene.wall_color, wall_color);
        assert!(scene.walls.is_empty());
        assert_eq!(scene.bugs, bugs);
        assert_eq!(scene.play_mode, PlayMode::Attack);
        assert!(scene.tower_preview.is_none());
        assert!(scene.active_tower_footprint_tiles.is_none());
        assert!(scene.towers.is_empty());
        assert!(scene.projectiles.is_empty());
        assert!(scene.tower_targets.is_empty());
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
        let wall_color = Color::from_rgb_u8(90, 90, 90);
        let preview_region = CellRect::from_origin_and_size(
            maze_defence_core::CellCoord::new(4, 6),
            maze_defence_core::CellRectSize::new(4, 4),
        );
        let placement_preview = TowerPreview::new(
            TowerKind::Basic,
            preview_region,
            false,
            Some(PlacementError::Occupied),
        );

        let target_line = TowerTargetLine::new(
            TowerId::new(1),
            maze_defence_core::BugId::new(3),
            Vec2::new(4.0, 6.0),
            Vec2::new(6.5, 8.5),
        );

        let scene = Scene::new(
            tile_grid,
            wall_color,
            Vec::new(),
            vec![],
            vec![SceneTower::new(
                TowerId::new(1),
                TowerKind::Basic,
                preview_region,
            )],
            Vec::new(),
            vec![target_line],
            PlayMode::Builder,
            Some(placement_preview),
            Some(Vec2::splat(1.0)),
            Some(TowerInteractionFeedback::PlacementRejected {
                kind: TowerKind::Basic,
                origin: maze_defence_core::CellCoord::new(4, 6),
                reason: PlacementError::Occupied,
            }),
        );

        assert_eq!(scene.play_mode, PlayMode::Builder);
        assert_eq!(scene.tower_preview, Some(placement_preview));
        assert_eq!(scene.active_tower_footprint_tiles, Some(Vec2::splat(1.0)));
        assert_eq!(scene.towers.len(), 1);
        assert_eq!(scene.tile_grid, tile_grid);
        assert_eq!(scene.wall_color, wall_color);
        assert!(scene.walls.is_empty());
        assert_eq!(
            scene.tower_feedback,
            Some(TowerInteractionFeedback::PlacementRejected {
                kind: TowerKind::Basic,
                origin: maze_defence_core::CellCoord::new(4, 6),
                reason: PlacementError::Occupied,
            })
        );
        assert_eq!(scene.tower_targets, vec![target_line]);
        assert!(scene.projectiles.is_empty());
    }

    #[test]
    fn scene_total_height_matches_bordered_grid_height() {
        let tile_grid = TileGridPresentation::new(
            4,
            3,
            24.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(100, 100, 100),
        )
        .expect("default cells_per_tile is valid");

        let scene = Scene::new(
            tile_grid,
            Color::from_rgb_u8(64, 64, 64),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            PlayMode::Attack,
            None,
            None,
            None,
        );

        assert_eq!(scene.total_height(), tile_grid.bordered_height());
    }
}
