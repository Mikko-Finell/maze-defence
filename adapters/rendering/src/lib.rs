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

/// Presentation descriptor consumed by rendering backends.
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    /// Title used by the created window.
    pub window_title: String,
    /// Solid color used to clear each frame.
    pub clear_color: Color,
}

impl Presentation {
    /// Constructs a new presentation descriptor.
    #[must_use]
    pub fn new<T>(window_title: T, clear_color: Color) -> Self
    where
        T: Into<String>,
    {
        Self {
            window_title: window_title.into(),
            clear_color,
        }
    }
}

/// Rendering backend capable of presenting Maze Defence scenes.
pub trait RenderingBackend {
    /// Runs the rendering backend until it is requested to exit.
    fn run(self, presentation: Presentation) -> Result<()>;
}
