#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Macroquad-backed rendering adapter for Maze Defence.
//!
//! Macroquad's optional audio stack depends on native ALSA development
//! libraries, which are unavailable in the containerised CI environment.
//! To keep `cargo test` usable everywhere we depend on macroquad without its
//! default `audio` feature. Consumers that need sound playback can opt back
//! in by enabling `macroquad/audio` in their own `Cargo.toml` dependency
//! specification.
//!
//! The adapter uses Macroquad's immediate-mode UI module so the control panel
//! can host widgets. All UI-specific calls live inside the local `ui` module to
//! avoid leaking Macroquad UI types throughout the renderer.

mod sprites;
mod ui;

use self::ui::{draw_control_panel_ui, ControlPanelUiContext, ControlPanelUiResult};
use anyhow::{Context, Result};
use glam::Vec2;
use macroquad::math::Vec2 as MacroquadVec2;
use macroquad::{
    color::BLACK,
    input::{is_key_pressed, is_mouse_button_pressed, mouse_position, KeyCode, MouseButton},
};
use maze_defence_core::{CellRect, PlayMode, TowerId, TowerKind, WaveDifficulty};
use maze_defence_rendering::{
    visuals::heading_from_target_line, BugPresentation, BugVisual, Color, ControlPanelView,
    FrameInput, FrameSimulationBreakdown, Presentation, RenderingBackend, Scene, SceneProjectile,
    SceneTower, SceneWall, SpawnEffect, SpriteInstance, SpriteKey, TileGridPresentation,
    TowerPreview, TowerTargetLine, TowerVisual,
};
use std::{
    collections::{HashMap, VecDeque},
    f32::consts::{FRAC_PI_2, PI},
    sync::mpsc,
    time::{Duration, Instant},
};

use self::sprites::SpriteAtlas;

/// Tracks UI-sourced interactions so they can be merged with physical input on the next frame.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlPanelInputState {
    mode_toggle_latched: bool,
    start_wave_latched: Option<WaveDifficulty>,
}

impl ControlPanelInputState {
    /// Returns whether the UI requested a mode toggle and clears the latch so the
    /// action fires only once.
    pub fn take_mode_toggle(&mut self) -> bool {
        let latched = self.mode_toggle_latched;
        self.mode_toggle_latched = false;
        latched
    }

    /// Records that the control-panel button requested a mode toggle this frame.
    pub fn register_mode_toggle(&mut self) {
        self.mode_toggle_latched = true;
    }

    /// Returns the latched wave launch request, clearing it so the action fires once.
    pub fn take_start_wave(&mut self) -> Option<WaveDifficulty> {
        self.start_wave_latched.take()
    }

    /// Records that the control-panel button requested a wave launch this frame.
    pub fn register_start_wave(&mut self, difficulty: WaveDifficulty) {
        self.start_wave_latched = Some(difficulty);
    }
}

/// Snapshot of edge-triggered keyboard shortcuts observed during a single frame.
#[derive(Clone, Copy, Debug, Default)]
struct KeyboardShortcuts {
    /// `Q` or `Escape` to quit the game loop.
    quit_requested: bool,
    /// `T` toggles tower targeting line visibility.
    toggle_target_lines: bool,
    /// `H` toggles bug health-bar overlays.
    toggle_bug_health_bars: bool,
    /// `Enter` launches an attack wave at normal difficulty.
    spawn_wave: bool,
    /// `Delete` removes the currently selected element.
    delete_pressed: bool,
}

impl KeyboardShortcuts {
    fn poll() -> Self {
        let quit_requested = is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::Q);
        let toggle_target_lines = is_key_pressed(KeyCode::T);
        let toggle_bug_health_bars = is_key_pressed(KeyCode::H);
        let spawn_wave = is_key_pressed(KeyCode::Enter);
        let delete_pressed = is_key_pressed(KeyCode::Delete);

        Self {
            quit_requested,
            toggle_target_lines,
            toggle_bug_health_bars,
            spawn_wave,
            delete_pressed,
        }
    }
}

/// Rendering backend implemented on top of macroquad.
#[derive(Debug)]
pub struct MacroquadBackend {
    swap_interval: Option<i32>,
    show_fps: bool,
    sprite_atlas: Option<SpriteAtlas>,
    turret_headings: HashMap<TowerId, f32>,
    load_sprites: bool,
}

impl Default for MacroquadBackend {
    fn default() -> Self {
        Self {
            swap_interval: None,
            show_fps: false,
            sprite_atlas: None,
            turret_headings: HashMap::new(),
            load_sprites: true,
        }
    }
}

impl MacroquadBackend {
    /// Returns a backend that requests the platform's default swap interval.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the backend to request a specific swap interval from the platform.
    #[must_use]
    pub fn with_swap_interval(mut self, swap_interval: Option<i32>) -> Self {
        self.swap_interval = swap_interval;
        self
    }

    /// Configures the backend to either synchronise presentation with the display refresh rate
    /// or render as fast as possible.
    #[must_use]
    pub fn with_vsync(self, enabled: bool) -> Self {
        let swap_interval = if enabled { Some(1) } else { Some(0) };
        self.with_swap_interval(swap_interval)
    }

    /// Configures whether the backend prints frame timing metrics once per second.
    #[must_use]
    pub fn with_show_fps(mut self, show: bool) -> Self {
        self.show_fps = show;
        self
    }

    /// Configures whether the backend should attempt to load sprite assets.
    #[must_use]
    pub fn with_sprite_loading(mut self, enabled: bool) -> Self {
        self.load_sprites = enabled;
        self
    }
}

fn scene_requests_sprites(scene: &Scene) -> bool {
    scene.ground.is_some()
        || scene
            .towers
            .iter()
            .any(|tower| matches!(tower.visual, TowerVisual::Sprite { .. }))
        || scene
            .bugs
            .iter()
            .any(|bug| matches!(bug.style, BugVisual::Sprite { .. }))
}

/// Tracks the average frames-per-second produced by the render loop.
#[derive(Clone, Copy, Debug, Default)]
struct FrameBreakdown {
    frame: Duration,
    simulation: Duration,
    pathfinding: Duration,
    scene_population: Duration,
    render: Duration,
}

#[derive(Debug, Default)]
struct FpsCounter {
    elapsed: Duration,
    frames: u32,
    frame_times: VecDeque<Duration>,
    window_duration: Duration,
    simulation_accum: Duration,
    pathfinding_accum: Duration,
    scene_population_accum: Duration,
    render_accum: Duration,
}

#[derive(Clone, Copy, Debug)]
struct FpsMetrics {
    per_second: f32,
    trailing_ten_seconds: f32,
    avg_simulation: Duration,
    avg_pathfinding: Duration,
    avg_scene_population: Duration,
    avg_render: Duration,
}

impl FpsCounter {
    /// Records a rendered frame and returns the per-second and trailing ten-second averages once
    /// one second has elapsed.
    fn record_frame(&mut self, breakdown: FrameBreakdown) -> Option<FpsMetrics> {
        self.elapsed += breakdown.frame;
        self.frames = self.frames.saturating_add(1);

        self.simulation_accum += breakdown.simulation;
        self.pathfinding_accum += breakdown.pathfinding;
        self.scene_population_accum += breakdown.scene_population;
        self.render_accum += breakdown.render;

        self.frame_times.push_back(breakdown.frame);
        self.window_duration += breakdown.frame;

        let trailing_window = Duration::from_secs(10);
        while self.window_duration > trailing_window {
            if let Some(removed) = self.frame_times.pop_front() {
                self.window_duration = self.window_duration.saturating_sub(removed);
            } else {
                break;
            }
        }

        if self.elapsed < Duration::from_secs(1) {
            return None;
        }

        let seconds = self.elapsed.as_secs_f32();
        if seconds <= f32::EPSILON {
            self.elapsed = Duration::ZERO;
            self.frames = 0;
            self.simulation_accum = Duration::ZERO;
            self.pathfinding_accum = Duration::ZERO;
            self.scene_population_accum = Duration::ZERO;
            self.render_accum = Duration::ZERO;
            return None;
        }

        let per_second = self.frames as f32 / seconds;
        let window_seconds = self.window_duration.as_secs_f32();
        let trailing_ten_seconds = if window_seconds <= f32::EPSILON {
            per_second
        } else {
            self.frame_times.len() as f32 / window_seconds
        };
        let frames = self.frames;
        let avg_simulation = if frames == 0 {
            Duration::ZERO
        } else {
            self.simulation_accum / frames
        };
        let avg_pathfinding = if frames == 0 {
            Duration::ZERO
        } else {
            self.pathfinding_accum / frames
        };
        let avg_scene_population = if frames == 0 {
            Duration::ZERO
        } else {
            self.scene_population_accum / frames
        };
        let avg_render = if frames == 0 {
            Duration::ZERO
        } else {
            self.render_accum / frames
        };
        self.elapsed = Duration::ZERO;
        self.frames = 0;
        self.simulation_accum = Duration::ZERO;
        self.pathfinding_accum = Duration::ZERO;
        self.scene_population_accum = Duration::ZERO;
        self.render_accum = Duration::ZERO;
        Some(FpsMetrics {
            per_second,
            trailing_ten_seconds,
            avg_simulation,
            avg_pathfinding,
            avg_scene_population,
            avg_render,
        })
    }
}

impl RenderingBackend for MacroquadBackend {
    fn run<F>(self, presentation: Presentation, mut update_scene: F) -> Result<()>
    where
        F: FnMut(Duration, FrameInput, &mut Scene) -> FrameSimulationBreakdown + 'static,
    {
        let Self {
            swap_interval,
            show_fps,
            sprite_atlas,
            turret_headings,
            load_sprites,
        } = self;

        let Presentation {
            window_title,
            clear_color,
            scene,
        } = presentation;

        let mut config = macroquad::window::Conf {
            window_title,
            window_width: 960,
            window_height: 960,
            ..macroquad::window::Conf::default()
        };
        if let Some(swap_interval) = swap_interval {
            config.platform.swap_interval = Some(swap_interval);
        }

        let sprite_support_enabled = load_sprites;
        let (atlas_init_sender, atlas_init_receiver) = mpsc::channel::<Result<()>>();

        macroquad::Window::from_config(config, async move {
            let mut init_sender = Some(atlas_init_sender);
            let mut scene = scene;
            let mut turret_headings = turret_headings;
            let mut sprite_atlas = sprite_atlas;

            if sprite_support_enabled {
                if sprite_atlas.is_none() {
                    match SpriteAtlas::new().context("failed to initialise sprite atlas") {
                        Ok(atlas) => {
                            sprite_atlas = Some(atlas);
                        }
                        Err(error) => {
                            if let Some(sender) = init_sender.take() {
                                let _ = sender.send(Err(error));
                            }
                            return;
                        }
                    }
                }

                debug_assert!(sprite_atlas
                    .as_ref()
                    .map(|atlas| {
                        atlas.contains(SpriteKey::TowerBase)
                            && atlas.contains(SpriteKey::TowerTurret)
                            && atlas.contains(SpriteKey::BugBody)
                    })
                    .unwrap_or(false));
            } else {
                sprite_atlas = None;
            }

            if let Some(sender) = init_sender.take() {
                let _ = sender.send(Ok(()));
            }

            if let Some(atlas) = &sprite_atlas {
                let _ = atlas.len();
            }

            let background = to_macroquad_color(clear_color);
            let mut fps_counter = FpsCounter::default();
            let mut show_tower_target_lines = false;
            let mut show_bug_health_bars = false;
            let mut control_panel_input = ControlPanelInputState::default();

            loop {
                let keyboard = KeyboardShortcuts::poll();
                if keyboard.quit_requested {
                    break;
                }

                if keyboard.toggle_target_lines {
                    show_tower_target_lines = !show_tower_target_lines;
                }

                if keyboard.toggle_bug_health_bars {
                    show_bug_health_bars = !show_bug_health_bars;
                }

                macroquad::window::clear_background(background);

                let screen_width = macroquad::window::screen_width();
                let screen_height = macroquad::window::screen_height();

                let dt_seconds = macroquad::time::get_frame_time();
                let frame_dt = Duration::from_secs_f32(dt_seconds.max(0.0));
                let metrics_before = SceneMetrics::from_scene(&scene, screen_width, screen_height);
                let mode_toggle = control_panel_input.take_mode_toggle();
                let start_wave = control_panel_input.take_start_wave();
                let frame_input =
                    gather_frame_input(&scene, &metrics_before, mode_toggle, start_wave, keyboard);

                let simulation_breakdown = update_scene(frame_dt, frame_input, &mut scene);

                if !sprite_support_enabled {
                    debug_assert!(!scene_requests_sprites(&scene));
                }

                turret_headings
                    .retain(|tower_id, _| scene.towers.iter().any(|tower| tower.id == *tower_id));

                let tile_grid = scene.tile_grid;
                let metrics = SceneMetrics::from_scene(&scene, screen_width, screen_height);

                let render_start = Instant::now();
                draw_ground(&scene, &metrics, sprite_atlas.as_ref());
                if scene.play_mode == PlayMode::Builder {
                    let grid_color = to_macroquad_color(tile_grid.line_color);
                    let subgrid_color = to_macroquad_color(tile_grid.line_color.lighten(0.6));

                    draw_subgrid(&metrics, &tile_grid, subgrid_color);
                    draw_tile_grid(&metrics, &tile_grid, grid_color);
                }
                draw_cell_walls(&scene, &metrics);
                draw_spawn_effects(&scene.spawn_effects, &metrics);

                let builder_preview = active_builder_preview(&scene);
                if let Some(preview) = builder_preview {
                    draw_tower_range_indicator(
                        preview.kind,
                        preview.region,
                        &scene.tile_grid,
                        &metrics,
                    );
                } else if let Some(tower) = hovered_tower(&scene) {
                    draw_tower_range_indicator(
                        tower.kind,
                        tower.region,
                        &scene.tile_grid,
                        &metrics,
                    );
                }

                draw_towers(
                    &scene.towers,
                    &scene.bugs,
                    &scene.tower_targets,
                    &metrics,
                    sprite_atlas.as_ref(),
                    &mut turret_headings,
                    TowerDrawStage::Base,
                );
                if show_bug_health_bars {
                    draw_bug_health_bars(&scene.bugs, &metrics);
                }
                draw_bugs(&scene.bugs, &metrics, sprite_atlas.as_ref());
                draw_towers(
                    &scene.towers,
                    &scene.bugs,
                    &scene.tower_targets,
                    &metrics,
                    sprite_atlas.as_ref(),
                    &mut turret_headings,
                    TowerDrawStage::Turret,
                );

                if let Some(preview) = builder_preview {
                    draw_tower_preview(preview, &metrics);
                }

                draw_projectiles(&scene.projectiles, &metrics);
                if let Some(panel_context) = draw_control_panel(&scene, screen_width, screen_height)
                {
                    let mut control_panel_ui = macroquad::ui::root_ui();
                    let ControlPanelUiResult {
                        mode_toggle,
                        start_wave,
                    } = draw_control_panel_ui(&mut control_panel_ui, panel_context);
                    if mode_toggle {
                        control_panel_input.register_mode_toggle();
                    }
                    if let Some(difficulty) = start_wave {
                        control_panel_input.register_start_wave(difficulty);
                    }
                }

                if show_tower_target_lines {
                    draw_tower_targets(&scene.tower_targets, &metrics);
                }

                let render_duration = render_start.elapsed();

                let frame_breakdown = FrameBreakdown {
                    frame: frame_dt,
                    simulation: simulation_breakdown.simulation,
                    pathfinding: simulation_breakdown.pathfinding,
                    scene_population: simulation_breakdown.scene_population,
                    render: render_duration,
                };

                let fps_metrics = fps_counter.record_frame(frame_breakdown);
                if show_fps {
                    if let Some(FpsMetrics {
                        per_second,
                        trailing_ten_seconds,
                        avg_simulation,
                        avg_pathfinding,
                        avg_scene_population,
                        avg_render,
                    }) = fps_metrics
                    {
                        println!(
                            "FPS: {:.2} (10s avg: {:.2}) | sim: {:>6.2}ms (path: {:>6.2}ms) scene: {:>6.2}ms render: {:>6.2}ms",
                            per_second,
                            trailing_ten_seconds,
                            avg_simulation.as_secs_f64() * 1_000.0,
                            avg_pathfinding.as_secs_f64() * 1_000.0,
                            avg_scene_population.as_secs_f64() * 1_000.0,
                            avg_render.as_secs_f64() * 1_000.0,
                        );
                    }
                }

                macroquad::window::next_frame().await;
            }
        });

        atlas_init_receiver.recv().unwrap_or_else(|_| Ok(()))?;

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct SceneMetrics {
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    grid_offset_x: f32,
    grid_offset_y: f32,
    grid_width_scaled: f32,
    grid_height_scaled: f32,
    bordered_grid_width_scaled: f32,
    bordered_grid_height_scaled: f32,
    tile_step: f32,
    cell_step: f32,
}

impl SceneMetrics {
    fn from_scene(scene: &Scene, screen_width: f32, screen_height: f32) -> Self {
        let tile_grid = scene.tile_grid;
        let world_width = tile_grid.bordered_width();
        let world_height = tile_grid.bordered_height();
        let panel_width = scene
            .control_panel
            .map(|panel| panel.width.max(0.0))
            .unwrap_or(0.0)
            .min(screen_width);
        let available_width = (screen_width - panel_width).max(0.0);
        let scale = if world_width == 0.0 || world_height == 0.0 {
            1.0
        } else {
            let width_ratio = if available_width <= f32::EPSILON {
                f32::INFINITY
            } else {
                available_width / world_width
            };
            width_ratio.min(screen_height / world_height)
        };

        let scaled_width = world_width * scale;
        let scaled_height = world_height * scale;
        let offset_x = ((available_width - scaled_width) * 0.5).max(0.0);
        let offset_y = (screen_height - scaled_height) * 0.5;

        let grid_width_scaled = tile_grid.width() * scale;
        let grid_height_scaled = tile_grid.height() * scale;
        let bordered_grid_width_scaled = tile_grid.bordered_width() * scale;
        let bordered_grid_height_scaled = tile_grid.bordered_height() * scale;
        let tile_step = tile_grid.tile_length * scale;
        let cell_step = if tile_grid.cells_per_tile == 0 {
            0.0
        } else {
            tile_step / tile_grid.cells_per_tile as f32
        };
        let grid_offset_x =
            offset_x + TileGridPresentation::SIDE_BORDER_CELL_LAYERS as f32 * cell_step;
        let grid_offset_y =
            offset_y + TileGridPresentation::TOP_BORDER_CELL_LAYERS as f32 * cell_step;

        Self {
            scale,
            offset_x,
            offset_y,
            grid_offset_x,
            grid_offset_y,
            grid_width_scaled,
            grid_height_scaled,
            bordered_grid_width_scaled,
            bordered_grid_height_scaled,
            tile_step,
            cell_step,
        }
    }

    fn bug_center(&self, position: Vec2) -> Vec2 {
        Vec2::new(
            self.offset_x + position.x * self.cell_step,
            self.offset_y + position.y * self.cell_step,
        )
    }
}

fn gather_frame_input(
    scene: &Scene,
    metrics: &SceneMetrics,
    mode_toggle: bool,
    ui_start_wave: Option<WaveDifficulty>,
    keyboard: KeyboardShortcuts,
) -> FrameInput {
    let (cursor_x, cursor_y) = mouse_position();
    let confirm_click = is_mouse_button_pressed(MouseButton::Left);
    let remove_click = is_mouse_button_pressed(MouseButton::Right);
    let start_wave = ui_start_wave.or_else(|| {
        if keyboard.spawn_wave {
            Some(WaveDifficulty::Normal)
        } else {
            None
        }
    });
    gather_frame_input_from_observations(
        scene,
        metrics,
        Vec2::new(cursor_x, cursor_y),
        mode_toggle,
        start_wave,
        confirm_click,
        remove_click,
        keyboard.delete_pressed,
    )
}

fn gather_frame_input_from_observations(
    scene: &Scene,
    metrics: &SceneMetrics,
    cursor_position: Vec2,
    mode_toggle: bool,
    start_wave: Option<WaveDifficulty>,
    confirm_click: bool,
    remove_click: bool,
    delete_pressed: bool,
) -> FrameInput {
    let mut input = FrameInput {
        mode_toggle,
        start_wave,
        ..FrameInput::default()
    };

    if metrics.scale <= f32::EPSILON {
        return input;
    }

    let tile_grid = scene.tile_grid;
    if tile_grid.columns == 0 || tile_grid.rows == 0 {
        return input;
    }

    let cursor_x = cursor_position.x;
    let cursor_y = cursor_position.y;

    let world_position = tile_grid.clamp_world_position(Vec2::new(
        (cursor_x - metrics.grid_offset_x) / metrics.scale,
        (cursor_y - metrics.grid_offset_y) / metrics.scale,
    ));

    input.cursor_world_space = Some(world_position);

    let inside = cursor_x >= metrics.grid_offset_x
        && cursor_x < metrics.grid_offset_x + metrics.grid_width_scaled
        && cursor_y >= metrics.grid_offset_y
        && cursor_y < metrics.grid_offset_y + metrics.grid_height_scaled;

    if inside {
        let footprint = scene
            .active_tower_footprint_tiles
            .unwrap_or_else(|| Vec2::splat(1.0));
        input.cursor_tile_space = tile_grid.snap_world_to_tile(world_position, footprint);
        input.confirm_action = confirm_click;
    }

    input.remove_action = remove_click || delete_pressed;

    input
}

fn draw_control_panel(
    scene: &Scene,
    screen_width: f32,
    screen_height: f32,
) -> Option<ControlPanelUiContext> {
    let Some(ControlPanelView { width, background }) = scene.control_panel else {
        return None;
    };
    if width <= f32::EPSILON {
        return None;
    }

    let left = (screen_width - width).max(0.0);
    let background_color = to_macroquad_color(background);
    macroquad::shapes::draw_rectangle(left, 0.0, width, screen_height, background_color);

    Some(ControlPanelUiContext {
        origin: MacroquadVec2::new(left, 0.0),
        size: MacroquadVec2::new(width, screen_height),
        background: background_color,
        play_mode: scene.play_mode,
        gold: scene.gold,
        difficulty: scene.difficulty,
        difficulty_selection: scene.difficulty_selection,
    })
}

fn active_builder_preview(scene: &Scene) -> Option<TowerPreview> {
    if scene.play_mode == PlayMode::Builder {
        scene.tower_preview
    } else {
        None
    }
}

fn hovered_tower(scene: &Scene) -> Option<SceneTower> {
    if scene.play_mode != PlayMode::Attack {
        return None;
    }

    let hovered = scene.hovered_tower?;
    scene
        .towers
        .iter()
        .copied()
        .find(|tower| tower.id == hovered)
}

fn draw_ground(scene: &Scene, metrics: &SceneMetrics, sprite_atlas: Option<&SpriteAtlas>) {
    let Some(tiles) = scene.ground.as_ref() else {
        return;
    };
    let Some(atlas) = sprite_atlas else {
        return;
    };

    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let sprite = &tiles.sprite;
    if sprite.size.x <= f32::EPSILON || sprite.size.y <= f32::EPSILON {
        return;
    }

    if !atlas.contains(sprite.sprite) {
        return;
    }

    let tile_grid = scene.tile_grid;
    if tile_grid.cells_per_tile == 0 {
        return;
    }

    let width_cells = (tile_grid.columns * tile_grid.cells_per_tile) as f32;
    let height_cells = (tile_grid.rows * tile_grid.cells_per_tile) as f32;
    if width_cells <= f32::EPSILON || height_cells <= f32::EPSILON {
        return;
    }

    let step_x = sprite.size.x;
    let step_y = sprite.size.y;
    if step_x <= f32::EPSILON || step_y <= f32::EPSILON {
        return;
    }

    let base_column = TileGridPresentation::SIDE_BORDER_CELL_LAYERS as f32;
    let base_row = TileGridPresentation::TOP_BORDER_CELL_LAYERS as f32;

    let horizontal_tiles = (width_cells / step_x).ceil().max(1.0) as u32;
    let vertical_tiles = (height_cells / step_y).ceil().max(1.0) as u32;

    for row in 0..vertical_tiles {
        let row_base = base_row + step_y * row as f32;
        for column in 0..horizontal_tiles {
            let column_base = base_column + step_x * column as f32;
            let base_position = Vec2::new(column_base, row_base);
            draw_sprite_instance(atlas, sprite, base_position, metrics, None, None);
        }
    }
}

fn draw_subgrid(
    metrics: &SceneMetrics,
    tile_grid: &TileGridPresentation,
    subgrid_color: macroquad::color::Color,
) {
    let total_subcolumns = tile_grid.columns * tile_grid.cells_per_tile
        + 2 * TileGridPresentation::SIDE_BORDER_CELL_LAYERS;
    for column in 0..=total_subcolumns {
        let x = metrics.offset_x + column as f32 * metrics.cell_step;
        macroquad::shapes::draw_line(
            x,
            metrics.offset_y,
            x,
            metrics.offset_y + metrics.bordered_grid_height_scaled,
            0.5,
            subgrid_color,
        );
    }

    let total_subrows = tile_grid.rows * tile_grid.cells_per_tile
        + TileGridPresentation::TOP_BORDER_CELL_LAYERS
        + TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS;
    for row in 0..=total_subrows {
        let y = metrics.offset_y + row as f32 * metrics.cell_step;
        macroquad::shapes::draw_line(
            metrics.offset_x,
            y,
            metrics.offset_x + metrics.bordered_grid_width_scaled,
            y,
            0.5,
            subgrid_color,
        );
    }
}

fn draw_tile_grid(
    metrics: &SceneMetrics,
    tile_grid: &TileGridPresentation,
    grid_color: macroquad::color::Color,
) {
    for column in 0..=tile_grid.columns {
        let x = metrics.grid_offset_x + column as f32 * metrics.tile_step;
        macroquad::shapes::draw_line(
            x,
            metrics.grid_offset_y,
            x,
            metrics.grid_offset_y + metrics.grid_height_scaled,
            1.0,
            grid_color,
        );
    }

    for row in 0..=tile_grid.rows {
        let y = metrics.grid_offset_y + row as f32 * metrics.tile_step;
        macroquad::shapes::draw_line(
            metrics.grid_offset_x,
            y,
            metrics.grid_offset_x + metrics.grid_width_scaled,
            y,
            1.0,
            grid_color,
        );
    }
}

fn draw_cell_walls(scene: &Scene, metrics: &SceneMetrics) {
    if scene.walls.is_empty() {
        return;
    }

    let cell_step = metrics.cell_step;
    if cell_step <= f32::EPSILON {
        return;
    }

    let color = to_macroquad_color(scene.wall_color);

    for SceneWall { column, row } in &scene.walls {
        let x = metrics.offset_x + (*column as f32) * cell_step;
        let y = metrics.offset_y + (*row as f32) * cell_step;
        macroquad::shapes::draw_rectangle(x, y, cell_step, cell_step, color);
    }
}

fn draw_spawn_effects(effects: &[SpawnEffect], metrics: &SceneMetrics) {
    if effects.is_empty() || metrics.cell_step <= f32::EPSILON {
        return;
    }

    let radius = (metrics.cell_step * 0.35).max(1.0);
    let outline_thickness = (metrics.cell_step * 0.08).max(1.0);

    for effect in effects {
        let center_x = metrics.offset_x + (effect.column as f32 + 0.5) * metrics.cell_step;
        let center_y = metrics.offset_y + (effect.row as f32 + 0.5) * metrics.cell_step;
        let fill = Color::new(
            effect.color.red,
            effect.color.green,
            effect.color.blue,
            0.55,
        );
        let outline = effect.color.lighten(0.3);

        macroquad::shapes::draw_circle(center_x, center_y, radius, to_macroquad_color(fill));
        macroquad::shapes::draw_circle_lines(
            center_x,
            center_y,
            radius,
            outline_thickness,
            to_macroquad_color(outline),
        );
    }
}

fn draw_tower_targets(tower_targets: &[TowerTargetLine], metrics: &SceneMetrics) {
    let line_color = to_macroquad_color(Color::new(0.85, 0.9, 1.0, 0.5));
    let thickness = 0.5;
    for (start, end) in tower_target_segments(tower_targets, metrics) {
        macroquad::shapes::draw_line(start.x, start.y, end.x, end.y, thickness, line_color);
    }
}

fn draw_bug_health_bars(bugs: &[BugPresentation], metrics: &SceneMetrics) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let bug_radius = metrics.cell_step * 0.5;
    let bar_margin = metrics.cell_step * 0.1;
    let bar_width = metrics.cell_step;
    let bar_height = (metrics.cell_step * 0.12).max(2.0) + 2.0;

    for bug in bugs {
        let bug_center = metrics.bug_center(bug.position());
        let health = bug.health;
        let bar_left = bug_center.x - bar_width * 0.5;
        let bar_top = bug_center.y + bug_radius + bar_margin;

        macroquad::shapes::draw_rectangle(bar_left, bar_top, bar_width, bar_height, BLACK);

        if health.maximum > 0 && health.current > 0 {
            let ratio = (health.current as f32 / health.maximum as f32).clamp(0.0, 1.0);
            let fill_width = bar_width * ratio;
            if fill_width > f32::EPSILON {
                let fill_color = macroquad::color::Color::new(0.78, 0.0, 0.0, 1.0);
                macroquad::shapes::draw_rectangle(
                    bar_left, bar_top, fill_width, bar_height, fill_color,
                );
            }
        }
    }
}

fn draw_bugs(bugs: &[BugPresentation], metrics: &SceneMetrics, sprite_atlas: Option<&SpriteAtlas>) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let bug_radius = metrics.cell_step * 0.5;
    let border_thickness = (bug_radius * 0.2).max(1.0);

    for bug in bugs {
        match bug.style {
            BugVisual::PrimitiveCircle { color } => {
                let bug_center = metrics.bug_center(bug.position());
                macroquad::shapes::draw_circle(
                    bug_center.x,
                    bug_center.y,
                    bug_radius,
                    to_macroquad_color(color),
                );
                macroquad::shapes::draw_circle_lines(
                    bug_center.x,
                    bug_center.y,
                    bug_radius,
                    border_thickness,
                    BLACK,
                );
            }
            BugVisual::Sprite { ref sprite, tint } => match sprite_atlas {
                Some(atlas) => {
                    let base_position = bug.position();
                    draw_sprite_instance(atlas, sprite, base_position, metrics, None, Some(tint));
                }
                None => {
                    debug_assert!(false, "sprite bug visual requested without sprite atlas",);
                }
            },
        }
    }
}

fn draw_projectiles(projectiles: &[SceneProjectile], metrics: &SceneMetrics) {
    let Some(points) = projectile_points(projectiles, metrics) else {
        return;
    };

    let radius = (metrics.cell_step * 0.1).max(1.0);
    let color = macroquad::color::Color::new(0.95, 0.92, 0.25, 1.0);

    for position in points {
        macroquad::shapes::draw_circle(position.x, position.y, radius, color);
    }
}

fn tower_target_segments(
    tower_targets: &[TowerTargetLine],
    metrics: &SceneMetrics,
) -> Vec<(Vec2, Vec2)> {
    if tower_targets.is_empty() || metrics.cell_step <= f32::EPSILON {
        return Vec::new();
    }

    tower_targets
        .iter()
        .map(|line| {
            let start = Vec2::new(
                metrics.offset_x + line.from.x * metrics.cell_step,
                metrics.offset_y + line.from.y * metrics.cell_step,
            );
            let end = Vec2::new(
                metrics.offset_x + line.to.x * metrics.cell_step,
                metrics.offset_y + line.to.y * metrics.cell_step,
            );
            (start, end)
        })
        .collect()
}

fn projectile_points(projectiles: &[SceneProjectile], metrics: &SceneMetrics) -> Option<Vec<Vec2>> {
    if projectiles.is_empty() || metrics.cell_step <= f32::EPSILON {
        return None;
    }

    Some(
        projectiles
            .iter()
            .map(|projectile| {
                Vec2::new(
                    metrics.offset_x + projectile.position.x * metrics.cell_step,
                    metrics.offset_y + projectile.position.y * metrics.cell_step,
                )
            })
            .collect(),
    )
}

#[derive(Clone, Copy)]
struct TowerPalette {
    fill: macroquad::color::Color,
    outline: macroquad::color::Color,
    turret: macroquad::color::Color,
}

#[derive(Clone, Copy)]
enum TowerDrawStage {
    Base,
    Turret,
}

fn tower_palette() -> TowerPalette {
    let base_color = Color::from_rgb_u8(78, 52, 128);
    let outline_color = base_color.lighten(0.35);
    let turret_color = base_color.lighten(0.55);

    TowerPalette {
        fill: to_macroquad_color(Color::new(
            base_color.red,
            base_color.green,
            base_color.blue,
            1.0,
        )),
        outline: to_macroquad_color(Color::new(
            outline_color.red,
            outline_color.green,
            outline_color.blue,
            1.0,
        )),
        turret: to_macroquad_color(Color::new(
            turret_color.red,
            turret_color.green,
            turret_color.blue,
            1.0,
        )),
    }
}

fn draw_towers(
    towers: &[SceneTower],
    bugs: &[BugPresentation],
    tower_targets: &[TowerTargetLine],
    metrics: &SceneMetrics,
    sprite_atlas: Option<&SpriteAtlas>,
    turret_headings: &mut HashMap<TowerId, f32>,
    stage: TowerDrawStage,
) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let palette = tower_palette();

    for tower in towers {
        match tower.visual {
            TowerVisual::PrimitiveRect => {
                draw_primitive_tower(tower, bugs, tower_targets, metrics, &palette, stage);
            }
            TowerVisual::Sprite {
                ref base,
                ref turret,
            } => {
                if let Some(atlas) = sprite_atlas {
                    draw_sprite_tower(
                        atlas,
                        tower,
                        base,
                        turret,
                        bugs,
                        tower_targets,
                        metrics,
                        turret_headings,
                        stage,
                    );
                } else {
                    draw_primitive_tower(tower, bugs, tower_targets, metrics, &palette, stage);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sprite_tower(
    atlas: &SpriteAtlas,
    tower: &SceneTower,
    base: &SpriteInstance,
    turret: &SpriteInstance,
    bugs: &[BugPresentation],
    tower_targets: &[TowerTargetLine],
    metrics: &SceneMetrics,
    turret_headings: &mut HashMap<TowerId, f32>,
    stage: TowerDrawStage,
) {
    let region = tower.region;
    let size = region.size();
    if size.width() == 0 || size.height() == 0 {
        return;
    }

    let origin = region.origin();
    let base_position = Vec2::new(origin.column() as f32, origin.row() as f32);
    match stage {
        TowerDrawStage::Base => {
            draw_sprite_instance(atlas, base, base_position, metrics, None, None);
        }
        TowerDrawStage::Turret => {
            let Some(center_cells) = tower_region_center(region) else {
                return;
            };

            let heading = resolve_turret_heading(
                tower.id,
                center_cells,
                turret,
                tower_targets,
                bugs,
                turret_headings,
            );
            draw_sprite_instance(atlas, turret, base_position, metrics, Some(heading), None);
        }
    }
}

fn draw_primitive_tower(
    tower: &SceneTower,
    bugs: &[BugPresentation],
    tower_targets: &[TowerTargetLine],
    metrics: &SceneMetrics,
    palette: &TowerPalette,
    stage: TowerDrawStage,
) {
    let region = tower.region;
    let size = region.size();
    if size.width() == 0 || size.height() == 0 {
        return;
    }

    match stage {
        TowerDrawStage::Base => {
            let origin = region.origin();
            let x = metrics.offset_x + origin.column() as f32 * metrics.cell_step;
            let y = metrics.offset_y + origin.row() as f32 * metrics.cell_step;
            let width = size.width() as f32 * metrics.cell_step;
            let height = size.height() as f32 * metrics.cell_step;
            let outline_thickness = (metrics.cell_step * 0.12).max(1.0);

            macroquad::shapes::draw_rectangle(x, y, width, height, palette.fill);
            macroquad::shapes::draw_rectangle_lines(
                x,
                y,
                width,
                height,
                outline_thickness,
                palette.outline,
            );
        }
        TowerDrawStage::Turret => {
            let Some(center_cells) = tower_region_center(region) else {
                return;
            };

            let max_dimension = size.width().max(size.height()) as f32;
            let half_length_cells = max_dimension * 0.5;
            let direction = turret_direction_for(tower.id, center_cells, tower_targets, bugs);
            draw_turret(
                center_cells,
                direction,
                half_length_cells,
                metrics,
                palette.turret,
            );
        }
    }
}

fn normalise_radians(angle: f32) -> f32 {
    if !angle.is_finite() {
        return 0.0;
    }

    let two_pi = 2.0 * PI;
    if two_pi <= f32::EPSILON {
        return angle.clamp(-PI, PI);
    }

    let mut wrapped = angle % two_pi;
    if wrapped > PI {
        wrapped -= two_pi;
    } else if wrapped < -PI {
        wrapped += two_pi;
    }

    wrapped.clamp(-PI, PI)
}

fn resolve_turret_heading(
    tower: TowerId,
    center_cells: Vec2,
    turret: &SpriteInstance,
    tower_targets: &[TowerTargetLine],
    bugs: &[BugPresentation],
    turret_headings: &mut HashMap<TowerId, f32>,
) -> f32 {
    if let Some(line) = tower_targets.iter().find(|line| line.tower == tower) {
        let direction = turret_direction_for(tower, center_cells, tower_targets, bugs);
        let heading_line = TowerTargetLine {
            from: center_cells,
            to: center_cells + direction,
            ..*line
        };
        let heading = normalise_radians(heading_from_target_line(&heading_line) + FRAC_PI_2);
        let _ = turret_headings.insert(tower, heading);
        heading
    } else {
        let fallback = normalise_radians(turret.rotation_radians);
        *turret_headings.entry(tower).or_insert(fallback)
    }
}

fn draw_sprite_instance(
    atlas: &SpriteAtlas,
    instance: &SpriteInstance,
    base_position: Vec2,
    metrics: &SceneMetrics,
    rotation_override: Option<f32>,
    tint: Option<Color>,
) {
    let texture_size = atlas.dimensions(instance.sprite);
    let Some((position, scale, pivot, rotation)) = sprite_draw_parameters(
        instance,
        base_position,
        metrics,
        rotation_override,
        texture_size,
    ) else {
        return;
    };

    let mut params = sprites::DrawParams::new(position)
        .with_scale(scale)
        .with_rotation(rotation)
        .with_pivot(pivot);
    if let Some(tint) = tint {
        params = params.with_tint(to_macroquad_color(tint));
    }
    atlas.draw(instance.sprite, params);
}

fn sprite_draw_parameters(
    instance: &SpriteInstance,
    base_position: Vec2,
    metrics: &SceneMetrics,
    rotation_override: Option<f32>,
    texture_size: MacroquadVec2,
) -> Option<(MacroquadVec2, MacroquadVec2, MacroquadVec2, f32)> {
    if metrics.cell_step <= f32::EPSILON {
        return None;
    }

    if texture_size.x <= f32::EPSILON || texture_size.y <= f32::EPSILON {
        return None;
    }

    let dest_size = MacroquadVec2::new(
        instance.size.x * metrics.cell_step,
        instance.size.y * metrics.cell_step,
    );
    if dest_size.x <= f32::EPSILON || dest_size.y <= f32::EPSILON {
        return None;
    }

    let offset = instance.offset.unwrap_or(Vec2::ZERO);
    let anchor_cells = base_position + offset;
    let anchor_pixels = MacroquadVec2::new(
        metrics.offset_x + anchor_cells.x * metrics.cell_step,
        metrics.offset_y + anchor_cells.y * metrics.cell_step,
    );

    debug_assert!(
        instance.pivot.x.is_finite() && instance.pivot.y.is_finite(),
        "sprite pivots must be finite values"
    );
    debug_assert!(
        (0.0..=1.0).contains(&instance.pivot.x) && (0.0..=1.0).contains(&instance.pivot.y),
        "sprite pivots must be normalised (received {:?})",
        instance.pivot
    );

    let pivot_offset = MacroquadVec2::new(
        dest_size.x * instance.pivot.x,
        dest_size.y * instance.pivot.y,
    );

    let position = MacroquadVec2::new(
        anchor_pixels.x - pivot_offset.x,
        anchor_pixels.y - pivot_offset.y,
    );
    let pivot = anchor_pixels;
    let scale = MacroquadVec2::new(dest_size.x / texture_size.x, dest_size.y / texture_size.y);

    if !scale.x.is_finite() || !scale.y.is_finite() {
        return None;
    }

    let rotation = rotation_override.unwrap_or(instance.rotation_radians);
    Some((position, scale, pivot, rotation))
}

fn turret_direction_for(
    tower: TowerId,
    center_cells: Vec2,
    tower_targets: &[TowerTargetLine],
    bugs: &[BugPresentation],
) -> Vec2 {
    let default = default_turret_direction();
    let Some(target_line) = tower_targets.iter().find(|line| line.tower == tower) else {
        return default;
    };

    if let Some(position) = bugs
        .iter()
        .find(|bug| bug.id == target_line.bug)
        .map(BugPresentation::position)
    {
        let direction = position - center_cells;
        if direction.length_squared() > f32::EPSILON {
            return direction.normalize();
        }
    }

    let direction = target_line.to - target_line.from;
    if direction.length_squared() > f32::EPSILON {
        return direction.normalize();
    }

    default
}

fn default_turret_direction() -> Vec2 {
    Vec2::new(0.0, -1.0)
}

fn draw_turret(
    center_cells: Vec2,
    direction: Vec2,
    half_length_cells: f32,
    metrics: &SceneMetrics,
    color: macroquad::color::Color,
) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let mut direction = direction;
    if direction.length_squared() <= f32::EPSILON {
        direction = default_turret_direction();
    } else {
        direction = direction.normalize();
    }

    let turret_width = metrics.cell_step * 0.5;
    let back_offset = turret_width * 0.5;
    let forward_length_cells = half_length_cells + 1.0;
    let forward_length = forward_length_cells * metrics.cell_step;
    let total_length = forward_length + back_offset;

    let center = Vec2::new(
        metrics.offset_x + center_cells.x * metrics.cell_step,
        metrics.offset_y + center_cells.y * metrics.cell_step,
    );

    let start = center - direction * back_offset;
    let end = start + direction * total_length;
    let perpendicular = Vec2::new(-direction.y, direction.x);
    let half_width = turret_width * 0.5;
    let offset = perpendicular * half_width;

    let p1 = start + offset;
    let p2 = start - offset;
    let p3 = end - offset;
    let p4 = end + offset;

    macroquad::shapes::draw_triangle(
        MacroquadVec2::new(p1.x, p1.y),
        MacroquadVec2::new(p2.x, p2.y),
        MacroquadVec2::new(p3.x, p3.y),
        color,
    );
    macroquad::shapes::draw_triangle(
        MacroquadVec2::new(p1.x, p1.y),
        MacroquadVec2::new(p3.x, p3.y),
        MacroquadVec2::new(p4.x, p4.y),
        color,
    );
}

fn draw_tower_range_indicator(
    kind: TowerKind,
    region: CellRect,
    tile_grid: &TileGridPresentation,
    metrics: &SceneMetrics,
) {
    if metrics.cell_step <= f32::EPSILON {
        return;
    }

    let radius_cells = kind.range_in_cells(tile_grid.cells_per_tile);
    if radius_cells == 0 {
        return;
    }

    let Some(center) = tower_region_center(region) else {
        return;
    };

    let radius = radius_cells as f32 * metrics.cell_step;
    if radius <= f32::EPSILON {
        return;
    }

    let center_x = metrics.offset_x + center.x * metrics.cell_step;
    let center_y = metrics.offset_y + center.y * metrics.cell_step;
    let fill = macroquad::color::Color::new(1.0, 0.0, 0.0, 0.15);
    let outline_thickness = (metrics.cell_step * 0.06).max(1.0);

    macroquad::shapes::draw_circle(center_x, center_y, radius, fill);
    macroquad::shapes::draw_circle_lines(center_x, center_y, radius, outline_thickness, BLACK);
}

fn tower_region_center(region: CellRect) -> Option<Vec2> {
    let size = region.size();
    if size.width() == 0 || size.height() == 0 {
        return None;
    }

    let origin = region.origin();
    Some(Vec2::new(
        origin.column() as f32 + size.width() as f32 * 0.5,
        origin.row() as f32 + size.height() as f32 * 0.5,
    ))
}

fn draw_tower_preview(preview: TowerPreview, metrics: &SceneMetrics) {
    let Some((x, y, width, height)) = preview_rectangle(preview, metrics) else {
        return;
    };

    let (fill_color, outline_color) = if preview.placeable {
        let base = Color::from_rgb_u8(78, 52, 128);
        let outline = base.lighten(0.4);
        (
            Color::new(base.red, base.green, base.blue, 0.35),
            Color::new(outline.red, outline.green, outline.blue, 0.7),
        )
    } else {
        let base = Color::from_rgb_u8(176, 52, 68);
        let outline = base.lighten(0.3);
        (
            Color::new(base.red, base.green, base.blue, 0.45),
            Color::new(outline.red, outline.green, outline.blue, 0.8),
        )
    };

    macroquad::shapes::draw_rectangle(x, y, width, height, to_macroquad_color(fill_color));
    macroquad::shapes::draw_rectangle_lines(
        x,
        y,
        width,
        height,
        (metrics.cell_step * 0.1).max(1.0),
        to_macroquad_color(outline_color),
    );
}

fn preview_rectangle(
    preview: TowerPreview,
    metrics: &SceneMetrics,
) -> Option<(f32, f32, f32, f32)> {
    if metrics.cell_step <= f32::EPSILON {
        return None;
    }

    let size = preview.region.size();
    if size.width() == 0 || size.height() == 0 {
        return None;
    }

    let origin = preview.region.origin();
    let x = metrics.offset_x + origin.column() as f32 * metrics.cell_step;
    let y = metrics.offset_y + origin.row() as f32 * metrics.cell_step;
    let width = size.width() as f32 * metrics.cell_step;
    let height = size.height() as f32 * metrics.cell_step;

    Some((x, y, width, height))
}

fn to_macroquad_color(color: maze_defence_rendering::Color) -> macroquad::color::Color {
    macroquad::color::Color::new(color.red, color.green, color.blue, color.alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use maze_defence_core::{
        BugId, CellCoord, CellRect, CellRectSize, Gold, ProjectileId, TowerId, TowerKind,
    };
    use maze_defence_rendering::{
        BugHealthPresentation, ControlPanelView, DifficultyPresentation, GoldPresentation,
        SpriteInstance, SpriteKey, TowerTargetLine,
    };
    use std::{collections::HashMap, f32::consts::FRAC_PI_2, time::Duration};

    fn base_scene(play_mode: PlayMode, placement_preview: Option<TowerPreview>) -> Scene {
        let grid = TileGridPresentation::new(
            4,
            4,
            32.0,
            TileGridPresentation::DEFAULT_CELLS_PER_TILE,
            Color::from_rgb_u8(40, 40, 40),
        )
        .expect("valid grid");
        let wall_color = Color::from_rgb_u8(64, 64, 64);

        Scene::new(
            grid,
            wall_color,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            play_mode,
            placement_preview,
            None,
            None,
            Some(ControlPanelView::new(200.0, Color::from_rgb_u8(0, 0, 0))),
            Some(GoldPresentation::new(Gold::new(0))),
            Some(DifficultyPresentation::new(0)),
            None,
        )
    }

    #[test]
    fn active_builder_preview_suppresses_attack_mode_preview() {
        let preview_region =
            CellRect::from_origin_and_size(CellCoord::new(2, 2), CellRectSize::new(4, 4));
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        let mut scene = base_scene(PlayMode::Attack, Some(preview));

        assert!(active_builder_preview(&scene).is_none());

        scene.play_mode = PlayMode::Builder;
        scene.tower_preview = Some(preview);

        assert_eq!(active_builder_preview(&scene), Some(preview));
    }

    #[test]
    fn confirm_action_only_set_when_cursor_inside_grid() {
        let mut scene = base_scene(PlayMode::Builder, None);
        scene.active_tower_footprint_tiles = Some(Vec2::new(1.5, 0.5));
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        let inside_cursor = Vec2::new(
            metrics.grid_offset_x + metrics.cell_step * 0.5,
            metrics.grid_offset_y + metrics.cell_step * 0.5,
        );
        let inside_input = gather_frame_input_from_observations(
            &scene,
            &metrics,
            inside_cursor,
            false,
            None,
            true,
            false,
            false,
        );
        assert!(
            inside_input.confirm_action,
            "left click inside the grid should be treated as a confirm action",
        );

        let outside_cursor = Vec2::new(metrics.grid_offset_x - 10.0, metrics.grid_offset_y - 10.0);
        let outside_input = gather_frame_input_from_observations(
            &scene,
            &metrics,
            outside_cursor,
            false,
            None,
            true,
            false,
            false,
        );
        assert!(
            outside_input.cursor_tile_space.is_none(),
            "cursor outside the grid must not snap to tile space",
        );
        assert!(
            !outside_input.confirm_action,
            "clicking outside the grid must not emit confirm actions",
        );
    }

    #[test]
    fn cursor_tile_space_respects_active_tower_footprint() {
        let mut scene = base_scene(PlayMode::Builder, None);
        let footprint = Vec2::new(1.5, 0.5);
        scene.active_tower_footprint_tiles = Some(footprint);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        let cursor = Vec2::new(
            metrics.grid_offset_x + metrics.grid_width_scaled - 1.0,
            metrics.grid_offset_y + metrics.grid_height_scaled - 1.0,
        );
        let input = gather_frame_input_from_observations(
            &scene, &metrics, cursor, false, None, false, false, false,
        );

        let tile = input
            .cursor_tile_space
            .expect("cursor inside grid should snap to tile space");
        let origin_column_tiles = tile.column_in_tiles();
        let origin_row_tiles = tile.row_in_tiles();

        assert!(origin_column_tiles + footprint.x <= scene.tile_grid.columns as f32 + 1e-5);
        assert!(origin_row_tiles + footprint.y <= scene.tile_grid.rows as f32 + 1e-5);
    }

    #[test]
    fn scene_metrics_respect_bordered_grid_height() {
        let scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 640.0, 480.0);
        let expected_height = scene.tile_grid.bordered_height() * metrics.scale;

        assert!((metrics.bordered_grid_height_scaled - expected_height).abs() <= f32::EPSILON);
    }

    #[test]
    fn scene_metrics_bottom_border_scales_with_cells_per_tile() {
        let tile_length = 48.0;
        let tile_color = Color::from_rgb_u8(32, 32, 32);
        let wall_color = Color::from_rgb_u8(64, 64, 64);
        let screen_width = 800.0;
        let screen_height = 600.0;

        let tolerance = 1e-4;

        for cells_per_tile in [1, 2, 3, 4] {
            let tile_grid =
                TileGridPresentation::new(6, 4, tile_length, cells_per_tile, tile_color)
                    .expect("cells_per_tile must be positive");
            let scene = Scene::new(
                tile_grid,
                wall_color,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
                PlayMode::Attack,
                None,
                None,
                None,
                Some(ControlPanelView::new(200.0, Color::from_rgb_u8(0, 0, 0))),
                Some(GoldPresentation::new(Gold::new(0))),
                Some(DifficultyPresentation::new(0)),
                None,
            );
            let metrics = SceneMetrics::from_scene(&scene, screen_width, screen_height);

            let total_border_height_scaled =
                metrics.bordered_grid_height_scaled - metrics.grid_height_scaled;
            let expected_border_height_scaled = tile_grid.cell_length()
                * (TileGridPresentation::TOP_BORDER_CELL_LAYERS
                    + TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS) as f32
                * metrics.scale;
            let total_delta = (total_border_height_scaled - expected_border_height_scaled).abs();
            assert!(
                total_delta <= tolerance,
                "bordered height mismatch for cells_per_tile {cells_per_tile}: {total_delta}"
            );

            let bottom_border_scaled = (metrics.offset_y + metrics.bordered_grid_height_scaled)
                - (metrics.grid_offset_y + metrics.grid_height_scaled);
            let expected_bottom_scaled = tile_grid.cell_length()
                * TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS as f32
                * metrics.scale;
            let bottom_delta = (bottom_border_scaled - expected_bottom_scaled).abs();
            assert!(
                bottom_delta <= tolerance,
                "bottom border mismatch for cells_per_tile {cells_per_tile}: {bottom_delta}"
            );

            let layers = bottom_border_scaled / metrics.cell_step;
            let layer_delta =
                (layers - TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS as f32).abs();
            assert!(
                layer_delta <= tolerance,
                "bottom border should span {} layer(s); measured {layers} (delta {layer_delta})",
                TileGridPresentation::BOTTOM_BORDER_CELL_LAYERS
            );
        }
    }

    #[test]
    fn tower_target_segments_empty_when_no_targets() {
        let mut scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        assert!(tower_target_segments(&scene.tower_targets, &metrics).is_empty());

        scene.tower_targets.push(TowerTargetLine::new(
            TowerId::new(1),
            BugId::new(2),
            Vec2::new(3.0, 4.0),
            Vec2::new(5.5, 6.5),
        ));
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);
        let segments = tower_target_segments(&scene.tower_targets, &metrics);

        assert_eq!(segments.len(), 1);
        let (start, end) = segments[0];
        let expected_start = Vec2::new(
            metrics.offset_x + 3.0 * metrics.cell_step,
            metrics.offset_y + 4.0 * metrics.cell_step,
        );
        let expected_end = Vec2::new(
            metrics.offset_x + 5.5 * metrics.cell_step,
            metrics.offset_y + 6.5 * metrics.cell_step,
        );
        assert_eq!(start, expected_start);
        assert_eq!(end, expected_end);
    }

    #[test]
    fn turret_direction_defaults_to_north_without_target() {
        let direction = turret_direction_for(TowerId::new(7), Vec2::new(5.0, 5.0), &[], &[]);

        assert_vec2_close(direction, Vec2::new(0.0, -1.0));
    }

    #[test]
    fn turret_direction_tracks_bug_position() {
        let tower = TowerId::new(9);
        let bug = BugId::new(12);
        let center = Vec2::new(4.0, 4.0);
        let target_line = TowerTargetLine::new(tower, bug, center, Vec2::new(6.0, 4.0));
        let bugs = vec![BugPresentation::new_circle(
            bug,
            Vec2::new(7.0, 4.0),
            Color::from_rgb_u8(200, 100, 50),
            BugHealthPresentation::new(3, 3),
        )];

        let direction = turret_direction_for(tower, center, &[target_line], &bugs);

        assert_vec2_close(direction, Vec2::new(1.0, 0.0));
    }

    #[test]
    fn turret_direction_falls_back_to_target_line_when_bug_missing() {
        let tower = TowerId::new(5);
        let bug = BugId::new(8);
        let center = Vec2::new(3.0, 3.0);
        let target_line = TowerTargetLine::new(tower, bug, center, Vec2::new(3.0, 1.0));

        let direction = turret_direction_for(tower, center, &[target_line], &[]);

        assert_vec2_close(direction, Vec2::new(0.0, -1.0));
    }

    #[test]
    fn resolve_turret_heading_respects_cache_and_defaults() {
        let tower = TowerId::new(42);
        let bug = BugId::new(7);
        let turret = SpriteInstance::square(SpriteKey::TowerTurret, Vec2::splat(1.0));
        let from = Vec2::new(2.5, 1.0);
        let to = Vec2::new(2.5, 4.0);
        let line = TowerTargetLine::new(tower, bug, from, to);
        let bug_position = Vec2::new(from.x + 1.0, from.y);
        let bugs = vec![BugPresentation::new_circle(
            bug,
            bug_position,
            Color::from_rgb_u8(180, 90, 40),
            BugHealthPresentation::new(5, 5),
        )];
        let mut cache = HashMap::new();

        let heading = resolve_turret_heading(tower, from, &turret, &[line], &bugs, &mut cache);
        let direction = turret_direction_for(tower, from, &[line], &bugs);
        let expected_line = TowerTargetLine {
            from,
            to: from + direction,
            ..line
        };
        let expected = normalise_radians(heading_from_target_line(&expected_line) + FRAC_PI_2);
        assert!((heading - expected).abs() <= 1e-6);
        assert_eq!(cache.get(&tower).copied(), Some(expected));

        let cached = resolve_turret_heading(tower, from, &turret, &[], &[], &mut cache);
        assert_eq!(cached, expected);

        let base_heading = 0.0;
        let oriented_turret = SpriteInstance::square(SpriteKey::TowerTurret, Vec2::splat(1.0))
            .with_rotation(base_heading);
        let fallback_tower = TowerId::new(7);
        let mut empty_cache = HashMap::new();
        let fallback = resolve_turret_heading(
            fallback_tower,
            Vec2::new(0.0, 0.0),
            &oriented_turret,
            &[],
            &[],
            &mut empty_cache,
        );
        assert!((fallback - base_heading).abs() <= 1e-6);
        assert_eq!(
            empty_cache.get(&fallback_tower).copied(),
            Some(base_heading)
        );
    }

    fn assert_vec2_close(actual: Vec2, expected: Vec2) {
        let delta = actual - expected;
        assert!(
            delta.length() <= 1e-4,
            "expected {expected:?}, got {actual:?} (delta {delta:?})"
        );
    }

    #[test]
    fn sprite_draw_parameters_compute_expected_values() {
        let scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);
        let instance = SpriteInstance::new(SpriteKey::TowerBase, Vec2::new(2.0, 1.5))
            .with_offset(Some(Vec2::new(1.0, 0.75)))
            .with_pivot(Vec2::new(0.5, 0.25))
            .with_rotation(0.35);
        let base_position = Vec2::new(3.0, 4.0);
        let texture_size = MacroquadVec2::new(128.0, 96.0);
        let rotation_override = 1.2;

        let (position, scale, pivot, rotation) = sprite_draw_parameters(
            &instance,
            base_position,
            &metrics,
            Some(rotation_override),
            texture_size,
        )
        .expect("expected sprite parameters");

        let dest_size = MacroquadVec2::new(
            instance.size.x * metrics.cell_step,
            instance.size.y * metrics.cell_step,
        );
        let offset = instance.offset.expect("offset present");
        let anchor_cells = base_position + offset;
        let anchor_pixels = MacroquadVec2::new(
            metrics.offset_x + anchor_cells.x * metrics.cell_step,
            metrics.offset_y + anchor_cells.y * metrics.cell_step,
        );
        let pivot_offset = MacroquadVec2::new(
            dest_size.x * instance.pivot.x,
            dest_size.y * instance.pivot.y,
        );
        let expected_position = MacroquadVec2::new(
            anchor_pixels.x - pivot_offset.x,
            anchor_pixels.y - pivot_offset.y,
        );
        let expected_pivot = anchor_pixels;
        let expected_scale =
            MacroquadVec2::new(dest_size.x / texture_size.x, dest_size.y / texture_size.y);

        assert_macroquad_vec2_close(position, expected_position);
        assert_macroquad_vec2_close(scale, expected_scale);
        assert_macroquad_vec2_close(pivot, expected_pivot);
        assert!((rotation - rotation_override).abs() <= 1e-6);

        let (_, _, _, default_rotation) =
            sprite_draw_parameters(&instance, base_position, &metrics, None, texture_size)
                .expect("expected default rotation");
        assert!((default_rotation - instance.rotation_radians).abs() <= 1e-6);
    }

    fn assert_macroquad_vec2_close(actual: MacroquadVec2, expected: MacroquadVec2) {
        let delta_x = (actual.x - expected.x).abs();
        let delta_y = (actual.y - expected.y).abs();
        assert!(
            delta_x <= 1e-4 && delta_y <= 1e-4,
            "expected {:?}, got {:?}",
            expected,
            actual
        );
    }

    #[test]
    fn sprite_draw_parameters_reject_zero_cell_step() {
        let instance = SpriteInstance::new(SpriteKey::TowerBase, Vec2::splat(1.0));
        let base_position = Vec2::new(1.0, 2.0);
        let metrics = SceneMetrics {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            grid_offset_x: 0.0,
            grid_offset_y: 0.0,
            grid_width_scaled: 0.0,
            grid_height_scaled: 0.0,
            bordered_grid_width_scaled: 0.0,
            bordered_grid_height_scaled: 0.0,
            tile_step: 0.0,
            cell_step: 0.0,
        };
        let texture_size = MacroquadVec2::new(64.0, 64.0);

        assert!(
            sprite_draw_parameters(&instance, base_position, &metrics, None, texture_size)
                .is_none()
        );
    }

    #[test]
    fn sprite_draw_parameters_reject_degenerate_inputs() {
        let scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);
        let base_position = Vec2::new(2.0, 3.0);
        let instance = SpriteInstance::new(SpriteKey::TowerBase, Vec2::splat(1.0));

        let zero_texture_width = MacroquadVec2::new(0.0, 64.0);
        assert!(sprite_draw_parameters(
            &instance,
            base_position,
            &metrics,
            None,
            zero_texture_width
        )
        .is_none());

        let zero_texture_height = MacroquadVec2::new(64.0, 0.0);
        assert!(sprite_draw_parameters(
            &instance,
            base_position,
            &metrics,
            None,
            zero_texture_height
        )
        .is_none());

        let zero_size_instance = SpriteInstance::new(SpriteKey::TowerBase, Vec2::ZERO);
        let valid_texture = MacroquadVec2::new(64.0, 64.0);
        assert!(sprite_draw_parameters(
            &zero_size_instance,
            base_position,
            &metrics,
            None,
            valid_texture,
        )
        .is_none());
    }

    #[test]
    fn projectile_points_empty_when_no_projectiles() {
        let mut scene = base_scene(PlayMode::Attack, None);
        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);

        assert!(projectile_points(&scene.projectiles, &metrics).is_none());

        scene.projectiles.push(SceneProjectile::new(
            ProjectileId::new(1),
            Vec2::new(1.0, 1.0),
            Vec2::new(3.0, 3.0),
            Vec2::new(2.0, 2.0),
            0.5,
        ));
        scene.projectiles.push(SceneProjectile::new(
            ProjectileId::new(2),
            Vec2::new(4.0, 5.0),
            Vec2::new(6.0, 7.0),
            Vec2::new(5.0, 6.0),
            0.75,
        ));

        let metrics = SceneMetrics::from_scene(&scene, 960.0, 960.0);
        let points = projectile_points(&scene.projectiles, &metrics)
            .expect("projectile points should be available");

        assert_eq!(points.len(), 2);
        let expected_first = Vec2::new(
            metrics.offset_x + 2.0 * metrics.cell_step,
            metrics.offset_y + 2.0 * metrics.cell_step,
        );
        let expected_second = Vec2::new(
            metrics.offset_x + 5.0 * metrics.cell_step,
            metrics.offset_y + 6.0 * metrics.cell_step,
        );
        assert_eq!(points[0], expected_first);
        assert_eq!(points[1], expected_second);
    }

    #[test]
    fn preview_rectangle_matches_footprint_in_world_space() {
        let mut scene = base_scene(PlayMode::Builder, None);
        let origin = CellCoord::new(4, 2);
        let size = CellRectSize::new(6, 4);
        let preview_region = CellRect::from_origin_and_size(origin, size);
        let preview = TowerPreview::new(TowerKind::Basic, preview_region, true, None);
        scene.tower_preview = Some(preview);
        let metrics = SceneMetrics::from_scene(&scene, 640.0, 640.0);

        let (_, _, width, height) =
            preview_rectangle(preview, &metrics).expect("preview geometry should be available");
        let footprint_tiles = Vec2::new(
            size.width() as f32 / scene.tile_grid.cells_per_tile as f32,
            size.height() as f32 / scene.tile_grid.cells_per_tile as f32,
        );
        let expected_width = footprint_tiles.x * scene.tile_grid.tile_length * metrics.scale;
        let expected_height = footprint_tiles.y * scene.tile_grid.tile_length * metrics.scale;

        assert!((width - expected_width).abs() < 1e-5);
        assert!((height - expected_height).abs() < 1e-5);
    }

    #[test]
    fn fps_counter_reports_average_frames_per_second() {
        let mut counter = FpsCounter::default();
        let frame = |millis| FrameBreakdown {
            frame: Duration::from_millis(millis),
            ..FrameBreakdown::default()
        };
        assert!(counter.record_frame(frame(250)).is_none());
        assert!(counter.record_frame(frame(250)).is_none());
        assert!(counter.record_frame(frame(250)).is_none());

        let metrics = counter
            .record_frame(frame(250))
            .expect("should report FPS after one second of samples");
        assert!((metrics.per_second - 4.0).abs() <= 1e-3);
        assert!((metrics.trailing_ten_seconds - 4.0).abs() <= 1e-3);
        assert!(counter.record_frame(frame(250)).is_none());
    }

    #[test]
    fn fps_counter_tracks_trailing_ten_second_average() {
        let mut counter = FpsCounter::default();
        let frame = |millis| FrameBreakdown {
            frame: Duration::from_millis(millis),
            ..FrameBreakdown::default()
        };

        for _ in 0..10 {
            for sample in 0..5 {
                let metrics = counter.record_frame(frame(200));
                if sample == 4 {
                    let metrics = metrics.expect("should report every second");
                    assert!((metrics.per_second - 5.0).abs() <= 1e-3);
                    assert!((metrics.trailing_ten_seconds - 5.0).abs() <= 1e-3);
                } else {
                    assert!(metrics.is_none());
                }
            }
        }

        for sample in 0..10 {
            let metrics = counter.record_frame(frame(100));
            if sample == 9 {
                let metrics = metrics.expect("should report every second");
                assert!((metrics.per_second - 10.0).abs() <= 1e-3);
                assert!((metrics.trailing_ten_seconds - 5.5).abs() <= 1e-3);
            } else {
                assert!(metrics.is_none());
            }
        }
    }
}
