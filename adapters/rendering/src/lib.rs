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
use std::{error::Error, fmt};

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

/// Describes a square tile grid that can be rendered by adapters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGridPresentation {
    /// Number of columns contained in the grid.
    pub columns: u32,
    /// Number of rows contained in the grid.
    pub rows: u32,
    /// Side length of a single tile expressed in world units.
    pub tile_length: f32,
    /// Number of subcells drawn along each tile edge.
    pub subdivisions_per_tile: u32,
    /// Color used when drawing grid lines.
    pub line_color: Color,
}

impl TileGridPresentation {
    /// Default number of subcells drawn along each tile edge.
    pub const DEFAULT_SUBDIVISIONS_PER_TILE: u32 = 4;

    /// Number of subcell layers rendered outside the tile grid on each side.
    pub const SIDE_BORDER_SUBCELL_LAYERS: u32 = 1;

    /// Number of subcell layers rendered above the tile grid.
    pub const TOP_BORDER_SUBCELL_LAYERS: u32 = 1;

    /// Number of subcell layers rendered below the tile grid.
    pub const BOTTOM_BORDER_SUBCELL_LAYERS: u32 = 0;

    /// Creates a new tile grid descriptor.
    ///
    /// Returns an error when `subdivisions_per_tile` is zero.
    #[must_use]
    pub fn new(
        columns: u32,
        rows: u32,
        tile_length: f32,
        subdivisions_per_tile: u32,
        line_color: Color,
    ) -> std::result::Result<Self, RenderingError> {
        if subdivisions_per_tile == 0 {
            return Err(RenderingError::InvalidSubdivisions {
                subdivisions_per_tile,
            });
        }

        Ok(Self {
            columns,
            rows,
            tile_length,
            subdivisions_per_tile,
            line_color,
        })
    }

    /// Length of a single subcell derived from the tile length.
    #[must_use]
    pub const fn subcell_length(&self) -> f32 {
        self.tile_length / self.subdivisions_per_tile as f32
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

    /// Calculates the total width of the grid including the surrounding subcell border.
    #[must_use]
    pub const fn bordered_width(&self) -> f32 {
        self.width() + 2.0 * self.subcell_length() * Self::SIDE_BORDER_SUBCELL_LAYERS as f32
    }

    /// Calculates the total height of the grid including the surrounding subcell border.
    #[must_use]
    pub const fn bordered_height(&self) -> f32 {
        self.height()
            + self.subcell_length()
                * (Self::TOP_BORDER_SUBCELL_LAYERS + Self::BOTTOM_BORDER_SUBCELL_LAYERS) as f32
    }
}

/// Describes an outer wall that should be rendered near the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallPresentation {
    /// Thickness of the wall measured in world units.
    pub thickness: f32,
    /// Color used for the wall fill.
    pub color: Color,
}

impl WallPresentation {
    /// Creates a new wall descriptor.
    #[must_use]
    pub const fn new(thickness: f32, color: Color) -> Self {
        Self { thickness, color }
    }
}

/// In-game bug rendered as a filled circle occupying a single grid cell.
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
    /// Bugs currently visible within the maze.
    pub bugs: Vec<BugPresentation>,
}

impl Scene {
    /// Creates a new scene descriptor.
    #[must_use]
    pub fn new(
        tile_grid: TileGridPresentation,
        wall: WallPresentation,
        bugs: Vec<BugPresentation>,
    ) -> Self {
        Self {
            tile_grid,
            wall,
            bugs,
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
    fn run(self, presentation: Presentation) -> AnyResult<()>;
}

/// Errors that can occur when constructing rendering descriptors.
#[derive(Debug, PartialEq, Eq)]
pub enum RenderingError {
    /// Subdivision count must be positive to avoid a zero-sized subcell.
    InvalidSubdivisions {
        /// Provided subdivision count that failed validation.
        subdivisions_per_tile: u32,
    },
}

impl fmt::Display for RenderingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSubdivisions {
                subdivisions_per_tile,
            } => {
                write!(
                    f,
                    "subdivisions_per_tile must be positive (received {subdivisions_per_tile})"
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
    fn tile_grid_creation_accepts_positive_subdivisions() {
        let presentation = TileGridPresentation::new(10, 5, 32.0, 4, Color::from_rgb_u8(0, 0, 0))
            .expect("positive subdivisions should succeed");

        assert_eq!(presentation.subdivisions_per_tile, 4);
    }

    #[test]
    fn tile_grid_creation_rejects_zero_subdivisions_without_panicking() {
        let error = TileGridPresentation::new(10, 5, 32.0, 0, Color::from_rgb_u8(0, 0, 0))
            .expect_err("zero subdivisions must be rejected");

        assert!(matches!(
            error,
            RenderingError::InvalidSubdivisions {
                subdivisions_per_tile: 0
            }
        ));
    }
}
