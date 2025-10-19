#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Shared rendering contracts for Maze Defence adapters.

use anyhow::Result;

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
    /// Color used when drawing grid lines.
    pub line_color: Color,
}

impl TileGridPresentation {
    /// Creates a new tile grid descriptor.
    #[must_use]
    pub const fn new(columns: u32, rows: u32, tile_length: f32, line_color: Color) -> Self {
        Self {
            columns,
            rows,
            tile_length,
            line_color,
        }
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

/// Scene description combining the tile grid and outer wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// Tile grid that composes the main play area.
    pub tile_grid: TileGridPresentation,
    /// Wall drawn outside the play area.
    pub wall: WallPresentation,
}

impl Scene {
    /// Creates a new scene descriptor.
    #[must_use]
    pub const fn new(tile_grid: TileGridPresentation, wall: WallPresentation) -> Self {
        Self { tile_grid, wall }
    }

    /// Height of the entire scene including the wall.
    #[must_use]
    pub const fn total_height(&self) -> f32 {
        self.tile_grid.height() + self.wall.thickness
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
    fn run(self, presentation: Presentation) -> Result<()>;
}
