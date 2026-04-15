use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::emath::GuiRounding;
use egui::epaint::PaintCallbackInfo;
use egui::{Color32, FontId, Pos2, Rect, Stroke, StrokeKind, vec2};

use crate::game::map::{CardinalDirection, LocationType, MapState, ObjectEntry, TileGridEncoding};
use crate::game::offsets::{
    COMBAT_TERRAIN_HEIGHT, COMBAT_TERRAIN_LEN, COMBAT_TERRAIN_STRIDE, COMBAT_TERRAIN_WIDTH,
    DUNGEON_LEVEL_HEIGHT, DUNGEON_LEVEL_LEN, DUNGEON_LEVEL_WIDTH, VIEWPORT_VISIBILITY_HEIGHT,
    VIEWPORT_VISIBILITY_LEN, VIEWPORT_VISIBILITY_WIDTH,
};
use crate::game::world_map::{WorldLabelCategory, WorldLocation, WorldMap};
use crate::tiles::atlas::{TILE_COUNT, TILE_SIZE, TileAtlas};
use crate::tiles::ega::is_ega_black_rgba;

use super::minimap_fog::{
    FOG_HIDDEN_TILE, FOG_VISIBILITY_EXPLORED, FOG_VISIBILITY_UNSEEN, FOG_VISIBILITY_VISIBLE,
    FogScene, FogState,
};
use super::minimap_gl::MinimapGl;

const ZOOM_MIN: usize = 11;
const ZOOM_MAX: usize = 256;
const ZOOM_DEFAULT: usize = 48;
const TILE_RGBA_BYTES: usize = TILE_SIZE * TILE_SIZE * 4;
const PLAYER_MARKER_MIN_RADIUS: f32 = 4.0;
const PLAYER_MARKER_TILE_MARGIN_PX: f32 = 3.0;
const PLAYER_MARKER_FILL_TILE_THRESHOLD_PX: f32 = 6.0;
const PLAYER_MARKER_COLOR: Color32 = Color32::from_rgb(255, 230, 128);
const PLAYER_FACING_ARROW_MIN_LEN: f32 = 10.0;
const PLAYER_FACING_ARROW_EXTRA_LEN: f32 = 7.0;
const PLAYER_FACING_ARROW_WIDTH: f32 = 6.0;
const DUNGEON_VIEW_PREF_FILE: &str = "minimap_dungeon_view.txt";
const DUNGEON_CORNER_VIEW_PREF_FILE: &str = "minimap_dungeon_corner_view.txt";

/// Controls whether the dungeon minimap's main panel follows the player or
/// shows the canonical fixed 8x8 floor layout.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DungeonPrimaryView {
    #[default]
    Centered,
    Fixed,
}

impl DungeonPrimaryView {
    fn swapped(self) -> Self {
        match self {
            Self::Centered => Self::Fixed,
            Self::Fixed => Self::Centered,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Centered => "centered",
            Self::Fixed => "fixed",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Centered => "Centered Main",
            Self::Fixed => "Fixed Main",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PlayerMarkerStyle {
    radius: f32,
    filled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LowpassTileSample {
    rgb: [u8; 3],
    coverage: u8,
}

/// Premultiplied overview sample used while collapsing multiple map layers
/// into one low-pass texel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PremultipliedCoverageSample {
    rgb: [u32; 3],
    coverage: u32,
}

impl PremultipliedCoverageSample {
    fn from_lowpass_tile(sample: LowpassTileSample) -> Self {
        let coverage = sample.coverage as u32;
        Self {
            rgb: [
                (sample.rgb[0] as u32 * coverage + 127) / 255,
                (sample.rgb[1] as u32 * coverage + 127) / 255,
                (sample.rgb[2] as u32 * coverage + 127) / 255,
            ],
            coverage,
        }
    }

    fn composite_over(self, overlay: LowpassTileSample) -> Self {
        let overlay = Self::from_lowpass_tile(overlay);
        let inverse_overlay = 255 - overlay.coverage;
        Self {
            rgb: [
                (self.rgb[0] * inverse_overlay + 127) / 255 + overlay.rgb[0],
                (self.rgb[1] * inverse_overlay + 127) / 255 + overlay.rgb[1],
                (self.rgb[2] * inverse_overlay + 127) / 255 + overlay.rgb[2],
            ],
            coverage: (self.coverage * inverse_overlay + 127) / 255 + overlay.coverage,
        }
    }

    fn to_rgba(self) -> [u8; 4] {
        [
            self.rgb[0].min(255) as u8,
            self.rgb[1].min(255) as u8,
            self.rgb[2].min(255) as u8,
            self.coverage.min(255) as u8,
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct LabelFilters {
    towns: bool,
    dwellings: bool,
    castles: bool,
    keeps: bool,
    dungeons: bool,
    shrines: bool,
}

impl Default for LabelFilters {
    fn default() -> Self {
        Self {
            towns: true,
            dwellings: true,
            castles: true,
            keeps: true,
            dungeons: true,
            shrines: true,
        }
    }
}

impl LabelFilters {
    fn shows(self, category: WorldLabelCategory) -> bool {
        match category {
            WorldLabelCategory::Town => self.towns,
            WorldLabelCategory::Dwelling => self.dwellings,
            WorldLabelCategory::Castle => self.castles,
            WorldLabelCategory::Keep => self.keeps,
            WorldLabelCategory::Dungeon => self.dungeons,
            WorldLabelCategory::Shrine => self.shrines,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OverworldOverlayOptions {
    cx: u8,
    cy: u8,
    grid_dims: (usize, usize),
    show_labels: bool,
    label_filters: LabelFilters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Distinguishes shared outdoor rendering from 32x32 local map rendering.
enum GridSource {
    Local,
    Outdoor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Cache identity for the current minimap grid contents.
///
/// Local maps need their own discriminator so switching towns, dungeon floors,
/// or outdoor layers at the same coordinates still invalidates the cached
/// terrain texture.
struct GridCacheKey {
    center: (u8, u8),
    zoom: usize,
    grid_dims: (usize, usize),
    source: GridSource,
    local_map: Option<(LocationType, u8)>,
    outdoor_z: Option<u8>,
}

/// Shared state accessed by both the UI thread (for updates) and the paint
/// callback (for rendering). Protected by a mutex.
struct GpuState {
    renderer: Option<MinimapGl>,
    grid_dirty: bool,
    fog_dirty: bool,
    grid_data: Vec<u8>,
    objects_data: Vec<u8>,
    lowpass_data: Vec<u8>,
    fog_data: Vec<u8>,
    grid_dims: (u32, u32),
    player_tile: [f32; 2],
}

pub struct MinimapState {
    pub map: Option<MapState>,
    gpu: Arc<Mutex<GpuState>>,
    /// Raw sequential atlas RGBA, captured once from TileAtlas for lazy GPU upload.
    raw_atlas: Option<Arc<Vec<u8>>>,
    tile_lowpass_lut: Option<Vec<LowpassTileSample>>,
    zoom: usize,
    show_labels: bool,
    label_filters: LabelFilters,
    last_grid_key: Option<GridCacheKey>,
    last_stable_visibility_scene: Option<FogScene>,
    last_stable_visibility_tiles: Option<[u8; VIEWPORT_VISIBILITY_LEN]>,
    dungeon_primary_view: DungeonPrimaryView,
    show_dungeon_corner_view: bool,
    fog: FogState,
    fog_reset_confirm_open: bool,
    fog_reset_confirm_text: String,
    fog_reset_error: Option<String>,
    fog_reset_target_game_key: Option<String>,
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            map: None,
            gpu: Arc::new(Mutex::new(GpuState {
                renderer: None,
                grid_dirty: false,
                fog_dirty: false,
                grid_data: Vec::new(),
                objects_data: Vec::new(),
                lowpass_data: Vec::new(),
                fog_data: Vec::new(),
                grid_dims: (0, 0),
                player_tile: [0.0, 0.0],
            })),
            raw_atlas: None,
            tile_lowpass_lut: None,
            zoom: ZOOM_DEFAULT,
            show_labels: true,
            label_filters: LabelFilters::default(),
            last_grid_key: None,
            last_stable_visibility_scene: None,
            last_stable_visibility_tiles: None,
            dungeon_primary_view: DungeonPrimaryView::default(),
            show_dungeon_corner_view: true,
            fog: FogState::new(),
            fog_reset_confirm_open: false,
            fog_reset_confirm_text: String::new(),
            fog_reset_error: None,
            fog_reset_target_game_key: None,
        }
    }
}

impl MinimapState {
    /// Construct an empty minimap state with default zoom and label filters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load persisted dungeon minimap preferences from the local app-data store.
    pub fn load_persistent_preferences(&mut self) {
        self.dungeon_primary_view = load_dungeon_primary_view();
        self.show_dungeon_corner_view = load_dungeon_corner_view();
        self.fog.load_preferences();
    }

    /// Return the currently selected primary dungeon view mode.
    pub fn dungeon_primary_view(&self) -> DungeonPrimaryView {
        self.dungeon_primary_view
    }

    /// Swap the primary and inset dungeon view modes and persist the choice.
    pub fn swap_dungeon_primary_view(&mut self) {
        self.dungeon_primary_view = self.dungeon_primary_view.swapped();
        save_dungeon_primary_view(self.dungeon_primary_view);
    }

    /// Whether the alternate dungeon orientation inset should be shown.
    pub fn show_dungeon_corner_view(&self) -> bool {
        self.show_dungeon_corner_view
    }

    /// Persistently enable or disable the alternate dungeon orientation inset.
    pub fn set_show_dungeon_corner_view(&mut self, visible: bool) {
        self.show_dungeon_corner_view = visible;
        save_dungeon_corner_view(visible);
    }

    pub fn set_game_directory(&mut self, game_dir: Option<&Path>) {
        if self.fog.set_game_dir(game_dir) {
            self.cancel_fog_reset_confirmation();
        }
    }

    pub fn fog_enabled(&self) -> bool {
        self.fog.enabled()
    }

    pub fn set_fog_enabled(&mut self, enabled: bool) {
        self.fog.set_enabled(enabled);
    }

    pub fn fog_supported_for_map(&self, map: &MapState) -> bool {
        FogScene::from_map(map).is_some()
    }

    pub fn can_reset_fog(&self) -> bool {
        self.fog.can_reset()
    }

    pub fn fog_persistence_error(&self) -> Option<&str> {
        self.fog.persistence_error()
    }

    pub fn reset_fog(&mut self) -> std::io::Result<()> {
        let Some(target_game_key) = self.fog_reset_target_game_key.as_deref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no fog reset target is selected",
            ));
        };
        self.fog.reset_game_by_key(target_game_key)?;
        self.fog_reset_error = None;
        Ok(())
    }

    pub fn begin_fog_reset_confirmation(&mut self) {
        self.fog_reset_confirm_open = true;
        self.fog_reset_confirm_text.clear();
        self.fog_reset_error = None;
        self.fog_reset_target_game_key = self.fog.current_game_key().map(str::to_owned);
    }

    pub fn cancel_fog_reset_confirmation(&mut self) {
        self.fog_reset_confirm_open = false;
        self.fog_reset_confirm_text.clear();
        self.fog_reset_error = None;
        self.fog_reset_target_game_key = None;
    }

    pub fn fog_reset_confirmation_open(&self) -> bool {
        self.fog_reset_confirm_open
    }

    pub fn fog_reset_confirmation_text(&mut self) -> &mut String {
        &mut self.fog_reset_confirm_text
    }

    pub fn fog_reset_confirmation_ready(&self) -> bool {
        self.fog_reset_confirm_text == "RESET"
    }

    pub fn fog_reset_error(&self) -> Option<&str> {
        self.fog_reset_error.as_deref()
    }

    pub fn set_fog_reset_error(&mut self, error: impl Into<String>) {
        self.fog_reset_error = Some(error.into());
    }

    /// Clear cached map data so the next loaded snapshot forces a fresh upload.
    pub fn clear(&mut self) {
        self.map = None;
        self.raw_atlas = None;
        self.tile_lowpass_lut = None;
        self.last_grid_key = None;
        self.last_stable_visibility_scene = None;
        self.last_stable_visibility_tiles = None;
        self.cancel_fog_reset_confirmation();
        self.fog_reset_error = None;

        let mut gpu = self.gpu.lock().unwrap();
        gpu.renderer = None;
        gpu.grid_dirty = false;
        gpu.fog_dirty = false;
        gpu.grid_data.clear();
        gpu.objects_data.clear();
        gpu.lowpass_data.clear();
        gpu.fog_data.clear();
        gpu.grid_dims = (0, 0);
        gpu.player_tile = [0.0, 0.0];
    }
}

/// Render the minimap controls and the current map view.
///
/// Non-dungeon scenes require a tile atlas for the GL-backed terrain path.
/// Dungeon scenes may instead be rendered through [`show_dungeon_without_atlas`]
/// when atlas loading fails.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut MinimapState,
    atlas: &TileAtlas,
    world_map: Option<&WorldMap>,
) {
    let Some(map) = state.map.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for map data...");
        });
        return;
    };
    let is_dungeon = matches!(map.location, LocationType::Dungeon(_));
    let is_outdoor = map.is_outdoor();
    let is_underworld = map.is_underworld();

    render_minimap_header(ui, state, &map, world_map.is_some());

    if is_dungeon {
        show_dungeon_map(ui, state, &map);
        return;
    }

    // Capture raw atlas data once for lazy GPU upload
    if state.raw_atlas.is_none() {
        state.raw_atlas = Some(Arc::new(atlas.raw_data().to_vec()));
    }
    if state.tile_lowpass_lut.is_none() {
        state.tile_lowpass_lut = state
            .raw_atlas
            .as_ref()
            .map(|raw_atlas| build_tile_lowpass_lut(raw_atlas.as_slice()));
    }
    let zoom = state.zoom;
    let cx = map.x;
    let cy = map.y;
    let panel_rect = ui
        .available_rect_before_wrap()
        .round_to_pixels(ui.pixels_per_point());
    let panel_size = panel_rect.size();
    let target_grid_dims = if is_outdoor && world_map.is_some() {
        outdoor_grid_dims_for_panel(panel_size, zoom)
    } else {
        local_scene_grid_dims(&map)
    };
    let grid_source = if is_outdoor && world_map.is_some() {
        GridSource::Outdoor
    } else {
        GridSource::Local
    };
    let grid_key = GridCacheKey {
        center: (cx, cy),
        zoom,
        grid_dims: target_grid_dims,
        source: grid_source,
        local_map: (!is_outdoor).then_some((map.location, map.z)),
        outdoor_z: is_outdoor.then_some(map.z),
    };

    // Prepare tile grid data on CPU when map state changes.
    // Object positions change every game turn, so always update when objects are present.
    let has_objects = !map.objects.is_empty();
    let needs_update = needs_grid_refresh(state.last_grid_key, grid_key, has_objects);

    let (grid_dims, player_tile, fog_data) = if needs_update {
        let (grid_data, grid_w, grid_h, player_tile) =
            if let Some(wm) = world_map.filter(|_| is_outdoor) {
                let grid = extract_outdoor_grid(wm, cx, cy, target_grid_dims, map.z);
                (
                    grid,
                    target_grid_dims.0 as u32,
                    target_grid_dims.1 as u32,
                    [
                        target_grid_dims.0 as f32 / 2.0,
                        target_grid_dims.1 as f32 / 2.0,
                    ],
                )
            } else {
                extract_local_scene_grid(&map)
            };

        let objects_data =
            build_objects_overlay(&map.objects, grid_w as usize, grid_h as usize, &map);
        let lowpass_data = build_lowpass_map(
            &grid_data,
            &objects_data,
            state.tile_lowpass_lut.as_ref().unwrap(),
            grid_w as usize,
            grid_h as usize,
        );
        let fog_data = build_fog_texture(state, &map, grid_source, (grid_w, grid_h));

        let mut gpu = state.gpu.lock().unwrap();
        gpu.grid_data = grid_data;
        gpu.objects_data = objects_data;
        gpu.lowpass_data = lowpass_data;
        gpu.grid_dims = (grid_w, grid_h);
        gpu.player_tile = player_tile;
        gpu.grid_dirty = true;
        gpu.fog_data = fog_data.clone();
        gpu.fog_dirty = true;

        state.last_grid_key = Some(grid_key);
        ((grid_w, grid_h), player_tile, fog_data)
    } else {
        let (grid_dims, player_tile) = {
            let gpu = state.gpu.lock().unwrap();
            (gpu.grid_dims, gpu.player_tile)
        };
        let fog_data = build_fog_texture(state, &map, grid_source, grid_dims);

        let mut gpu = state.gpu.lock().unwrap();
        if gpu.fog_data != fog_data {
            gpu.fog_data = fog_data.clone();
            gpu.fog_dirty = true;
        }

        (grid_dims, player_tile, fog_data)
    };

    let _response = ui.allocate_rect(panel_rect, egui::Sense::hover());
    let map_rect = if matches!(grid_source, GridSource::Outdoor) {
        fit_rect_to_grid(panel_rect, (grid_dims.0 as usize, grid_dims.1 as usize))
            .round_to_pixels(ui.pixels_per_point())
    } else {
        panel_rect
    };
    if (panel_rect.width() - map_rect.width()).abs() > f32::EPSILON
        || (panel_rect.height() - map_rect.height()).abs() > f32::EPSILON
    {
        ui.painter()
            .rect_filled(panel_rect, 0.0, Color32::from_rgb(4, 4, 4));
    }

    let gpu = state.gpu.clone();
    let raw_atlas = state.raw_atlas.clone().unwrap();
    let callback = egui_glow::CallbackFn::new(move |info: PaintCallbackInfo, painter| {
        let gl = painter.gl();
        let mut gpu = gpu.lock().unwrap();

        // Lazy-init: create renderer and upload atlas on first callback
        if gpu.renderer.is_none() {
            gpu.renderer = Some(MinimapGl::new(gl, &raw_atlas));
            // Force grid upload on first frame
            gpu.grid_dirty = true;
            gpu.fog_dirty = true;
        }

        // Upload grid and objects textures if dirty
        if gpu.grid_dirty {
            let renderer = gpu.renderer.as_ref().unwrap();
            renderer.update_grid(gl, &gpu.grid_data, gpu.grid_dims.0, gpu.grid_dims.1);
            renderer.update_objects(gl, &gpu.objects_data, gpu.grid_dims.0, gpu.grid_dims.1);
            renderer.update_lowpass(gl, &gpu.lowpass_data, gpu.grid_dims.0, gpu.grid_dims.1);
            gpu.grid_dirty = false;
        }
        if gpu.fog_dirty {
            let renderer = gpu.renderer.as_ref().unwrap();
            renderer.update_fog(gl, &gpu.fog_data, gpu.grid_dims.0, gpu.grid_dims.1);
            gpu.fog_dirty = false;
        }

        let grid_size = [gpu.grid_dims.0 as f32, gpu.grid_dims.1 as f32];
        gpu.renderer.as_ref().unwrap().paint(gl, &info, grid_size);
    });

    ui.painter().add(egui::PaintCallback {
        rect: map_rect,
        callback: Arc::new(callback),
    });

    if let Some(wm) = world_map.filter(|_| is_outdoor && !is_underworld) {
        let overlay = OverworldOverlayOptions {
            cx,
            cy,
            grid_dims: (grid_dims.0 as usize, grid_dims.1 as usize),
            show_labels: state.show_labels,
            label_filters: state.label_filters,
        };
        paint_overworld_overlay(
            ui,
            map_rect,
            wm,
            overlay,
            state.fog_enabled().then_some(fog_data.as_slice()),
        );
    }

    paint_player_marker(ui, map_rect, grid_dims, player_tile, None);
    show_fog_reset_confirmation(ui.ctx(), state);
}

/// Render the dungeon minimap without requiring a loaded tile atlas.
pub fn show_dungeon_without_atlas(ui: &mut egui::Ui, state: &mut MinimapState) {
    let Some(map) = state.map.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for map data...");
        });
        return;
    };

    if !matches!(map.location, LocationType::Dungeon(_)) {
        show_no_atlas(ui, "Tile atlas is unavailable for this scene.");
        return;
    }

    render_minimap_header(ui, state, &map, false);
    show_dungeon_map(ui, state, &map);
    show_fog_reset_confirmation(ui.ctx(), state);
}

/// Render a placeholder when the tile atlas has not been loaded yet.
pub fn show_no_atlas(ui: &mut egui::Ui, status: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(status);
    });
}

fn render_minimap_header(
    ui: &mut egui::Ui,
    state: &mut MinimapState,
    map: &MapState,
    world_map_loaded: bool,
) {
    let is_dungeon = matches!(map.location, LocationType::Dungeon(_));
    let is_outdoor = map.is_outdoor();
    let is_underworld = map.is_underworld();
    let fog_supported = state.fog_supported_for_map(map);
    let header = if is_outdoor || map.z == 0xFF {
        format!("{} ({}, {})", map.display_location_name(), map.x, map.y)
    } else {
        format!(
            "{} ({}, {}) Z:{}",
            map.display_location_name(),
            map.x,
            map.y,
            map.z
        )
    };

    ui.vertical(|ui| {
        // Keep the changing coordinate text on its own line so movement cannot
        // reflow the controls row and resize the map panel.
        ui.add(egui::Label::new(&header).truncate());

        ui.horizontal_wrapped(|ui| {
            if is_dungeon {
                ui.label("8x8 dungeon grid");
                ui.separator();
                ui.label(state.dungeon_primary_view().title());
                if ui.small_button("Swap Views").clicked() {
                    state.swap_dungeon_primary_view();
                }
                ui.separator();
                let mut show_corner_view = state.show_dungeon_corner_view();
                if ui
                    .checkbox(&mut show_corner_view, "Show Orientation Inset")
                    .changed()
                {
                    state.set_show_dungeon_corner_view(show_corner_view);
                }
            } else {
                if ui.small_button("\u{2796}").clicked() {
                    state.zoom = (state.zoom * 4 / 3).min(ZOOM_MAX);
                }
                let mut zoom_f = state.zoom as f64;
                let slider = egui::Slider::new(&mut zoom_f, ZOOM_MAX as f64..=ZOOM_MIN as f64)
                    .logarithmic(true)
                    .show_value(false)
                    .clamping(egui::SliderClamping::Always);
                if ui.add(slider).changed() {
                    state.zoom = zoom_f as usize;
                }
                if ui.small_button("\u{2795}").clicked() {
                    state.zoom = (state.zoom * 3 / 4).max(ZOOM_MIN);
                }
                ui.label(format!("Zoom {}", state.zoom));
                ui.separator();
                if fog_supported {
                    let mut fog_enabled = state.fog_enabled();
                    if ui.checkbox(&mut fog_enabled, "Fog").changed() {
                        state.set_fog_enabled(fog_enabled);
                    }
                }
                let reset = ui.add_enabled(state.can_reset_fog(), egui::Button::new("Reset Fog…"));
                if reset.clicked() {
                    state.begin_fog_reset_confirmation();
                }
            }
        });

        if is_outdoor && !is_underworld && world_map_loaded {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut state.show_labels, "Labels");
                ui.add_enabled_ui(state.show_labels, |ui| {
                    ui.checkbox(&mut state.label_filters.towns, "Towns");
                    ui.checkbox(&mut state.label_filters.dwellings, "Dwellings");
                    ui.checkbox(&mut state.label_filters.castles, "Castles");
                    ui.checkbox(&mut state.label_filters.keeps, "Keeps");
                    ui.checkbox(&mut state.label_filters.dungeons, "Dungeons");
                    ui.checkbox(&mut state.label_filters.shrines, "Shrines");
                });
            });
        }

        if let Some(error) = state.fog_persistence_error() {
            ui.colored_label(
                Color32::from_rgb(225, 109, 109),
                format!("Fog save failed: {error}"),
            );
        }
    });
}

fn show_fog_reset_confirmation(ctx: &egui::Context, state: &mut MinimapState) {
    if !state.fog_reset_confirmation_open() {
        return;
    }

    let mut keep_open = true;
    egui::Window::new("Confirm Fog Reset")
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .open(&mut keep_open)
        .show(ctx, |ui| {
            ui.label("This will permanently clear all discovered fog data for the current Ultima V install.");
            ui.label("Type `RESET` to enable the destructive action.");
            if let Some(error) = state.fog_reset_error() {
                ui.add_space(6.0);
                ui.colored_label(
                    Color32::from_rgb(225, 109, 109),
                    format!("Reset failed: {error}"),
                );
            }
            ui.add_space(6.0);
            ui.text_edit_singleline(state.fog_reset_confirmation_text());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    state.cancel_fog_reset_confirmation();
                }
                let confirm = ui.add_enabled(
                    state.fog_reset_confirmation_ready(),
                    egui::Button::new("Reset Fog"),
                );
                if confirm.clicked() {
                    match state.reset_fog() {
                        Ok(()) => state.cancel_fog_reset_confirmation(),
                        Err(err) => state.set_fog_reset_error(err.to_string()),
                    }
                }
            });
        });

    if !keep_open {
        state.cancel_fog_reset_confirmation();
    }
}

/// Extract a rectangular outdoor window centered on (cx, cy), wrapping at 256.
fn extract_outdoor_grid(
    world: &WorldMap,
    cx: u8,
    cy: u8,
    grid_dims: (usize, usize),
    z: u8,
) -> Vec<u8> {
    let half_w = grid_dims.0 as i32 / 2;
    let half_h = grid_dims.1 as i32 / 2;
    let mut grid = vec![0u8; grid_dims.0 * grid_dims.1];
    for vy in 0..grid_dims.1 {
        for vx in 0..grid_dims.0 {
            let wx = (cx as i32 - half_w + vx as i32).rem_euclid(256) as u8;
            let wy = (cy as i32 - half_h + vy as i32).rem_euclid(256) as u8;
            grid[vy * grid_dims.0 + vx] = world.outdoor_tile(z, wx, wy);
        }
    }
    grid
}

/// Decode the overworld MAP_TILES buffer when we do not have BRIT.DAT available.
///
/// The live overworld buffer is arranged as four 16x16 chunks.
fn linearize_chunked_grid(tiles: &[u8; 1024]) -> Vec<u8> {
    let mut grid = vec![0u8; 32 * 32];
    for gy in 0..32usize {
        for gx in 0..32usize {
            let cx = gx / 16;
            let cy = gy / 16;
            let lx = gx % 16;
            let ly = gy % 16;
            grid[gy * 32 + gx] = tiles[(cy * 2 + cx) * 256 + ly * 16 + lx];
        }
    }
    grid
}

/// Decode a plain 32x32 row-major MAP_TILES window.
fn extract_row_major_grid(tiles: &[u8; 1024]) -> Vec<u8> {
    tiles.to_vec()
}

fn local_scene_grid_dims(map: &MapState) -> (usize, usize) {
    match map.location.tile_grid_encoding() {
        TileGridEncoding::Chunked16x16 | TileGridEncoding::RowMajor32 => (32, 32),
        TileGridEncoding::Dungeon8x8 => (DUNGEON_LEVEL_WIDTH, DUNGEON_LEVEL_HEIGHT),
        TileGridEncoding::Combat11x11Stride32 => (COMBAT_TERRAIN_WIDTH, COMBAT_TERRAIN_HEIGHT),
    }
}

fn normalize_odd_span(span: usize, max: usize) -> usize {
    match span.clamp(1, max) {
        1 => 1,
        value if value == max && value % 2 == 0 => value - 1,
        value if value % 2 == 1 => value,
        value => value + 1,
    }
}

fn outdoor_grid_dims_for_panel(panel_size: egui::Vec2, zoom: usize) -> (usize, usize) {
    let rows = normalize_odd_span(zoom.clamp(ZOOM_MIN, ZOOM_MAX), ZOOM_MAX);
    if panel_size.x <= 0.0 || panel_size.y <= 0.0 {
        return (rows, rows);
    }

    let panel_w = panel_size.x.round().max(1.0) as usize;
    let panel_h = panel_size.y.round().max(1.0) as usize;
    let cols = rows
        .saturating_mul(panel_w)
        .checked_div(panel_h)
        .unwrap_or(0);
    // Keep the outdoor view on odd spans so the player's tile lands on the
    // true geometric center instead of straddling two cells.
    let cols = normalize_odd_span(cols, 256);
    (cols, rows)
}

fn fit_rect_to_grid(bounds: Rect, grid_dims: (usize, usize)) -> Rect {
    let grid_w = grid_dims.0.max(1) as f32;
    let grid_h = grid_dims.1.max(1) as f32;
    let scale = (bounds.width() / grid_w).min(bounds.height() / grid_h);
    Rect::from_center_size(bounds.center(), vec2(grid_w * scale, grid_h * scale))
}

/// Decode the current non-overworld scene into a renderable grid plus player position.
fn extract_local_scene_grid(map: &MapState) -> (Vec<u8>, u32, u32, [f32; 2]) {
    match map.location.tile_grid_encoding() {
        TileGridEncoding::Chunked16x16 => {
            let pbx = map.x.wrapping_sub(map.scroll_x) as f32;
            let pby = map.y.wrapping_sub(map.scroll_y) as f32;
            (linearize_chunked_grid(&map.tiles), 32, 32, [pbx, pby])
        }
        TileGridEncoding::RowMajor32 => (
            extract_row_major_grid(&map.tiles),
            32,
            32,
            local_scene_player_tile(map),
        ),
        TileGridEncoding::Dungeon8x8 => (
            extract_dungeon_grid(&map.tiles),
            DUNGEON_LEVEL_WIDTH as u32,
            DUNGEON_LEVEL_HEIGHT as u32,
            local_scene_player_tile(map),
        ),
        TileGridEncoding::Combat11x11Stride32 => {
            debug_assert!(
                map.combat_tiles.is_some(),
                "combat scenes should include the dedicated combat terrain grid"
            );
            (
                map.combat_tiles
                    .as_ref()
                    .map(extract_combat_grid)
                    .unwrap_or_else(|| vec![0; COMBAT_TERRAIN_WIDTH * COMBAT_TERRAIN_HEIGHT]),
                COMBAT_TERRAIN_WIDTH as u32,
                COMBAT_TERRAIN_HEIGHT as u32,
                [map.x as f32, map.y as f32],
            )
        }
    }
}

fn extract_dungeon_grid(tiles: &[u8; 1024]) -> Vec<u8> {
    tiles[..DUNGEON_LEVEL_LEN].to_vec()
}

fn local_scene_player_tile(map: &MapState) -> [f32; 2] {
    match map.location {
        // Dungeon mode keeps the active local map in a dedicated runtime buffer
        // while scroll_x/scroll_y still reflect the entrance chunk in Britannia.
        LocationType::Dungeon(_) | LocationType::Combat(_) => [map.x as f32, map.y as f32],
        _ => [
            map.x.wrapping_sub(map.scroll_x) as f32,
            map.y.wrapping_sub(map.scroll_y) as f32,
        ],
    }
}

fn build_fog_texture(
    state: &mut MinimapState,
    map: &MapState,
    grid_source: GridSource,
    grid_dims: (u32, u32),
) -> Vec<u8> {
    let grid_dims = (grid_dims.0 as usize, grid_dims.1 as usize);
    let len = grid_dims.0 * grid_dims.1;
    let Some(scene) = FogScene::from_map(map) else {
        return vec![FOG_VISIBILITY_VISIBLE; len];
    };

    let fog_enabled = state.fog_enabled();
    let (visibility_tiles, persist_visible_tiles) = if let Some(tiles) = map.visibility_tiles {
        state.last_stable_visibility_scene = Some(scene);
        state.last_stable_visibility_tiles = Some(tiles);
        (Some(tiles), true)
    } else if state.last_stable_visibility_scene == Some(scene) {
        (state.last_stable_visibility_tiles, false)
    } else {
        (None, false)
    };
    let projection = FogProjection::new(map, scene, grid_source, grid_dims);
    let visible_scene_coords = projection.visible_scene_coords(visibility_tiles.as_ref());
    if persist_visible_tiles {
        state.fog.record_visible_tiles(scene, &visible_scene_coords);
    }
    let explored = state.fog.scene_data(scene);
    let mut fog = vec![FOG_VISIBILITY_UNSEEN; len];
    let (scene_w, _) = scene.dimensions();

    for gy in 0..grid_dims.1 {
        for gx in 0..grid_dims.0 {
            if let Some((scene_x, scene_y)) = projection.scene_coord_for_grid_cell(gx, gy)
                && explored[scene_y * scene_w + scene_x] != 0
            {
                fog[gy * grid_dims.0 + gx] = FOG_VISIBILITY_EXPLORED;
            }
        }
    }

    for (scene_x, scene_y) in visible_scene_coords {
        if let Some((grid_x, grid_y)) = projection.grid_cell_for_scene_coord(scene_x, scene_y) {
            let idx = grid_y * grid_dims.0 + grid_x;
            fog[idx] = FOG_VISIBILITY_VISIBLE;
        }
    }

    if !fog_enabled {
        fog.fill(FOG_VISIBILITY_VISIBLE);
    }

    fog
}

#[derive(Debug, Clone, Copy)]
struct FogProjection<'a> {
    map: &'a MapState,
    scene: FogScene,
    grid_source: GridSource,
    grid_dims: (usize, usize),
}

impl<'a> FogProjection<'a> {
    fn new(
        map: &'a MapState,
        scene: FogScene,
        grid_source: GridSource,
        grid_dims: (usize, usize),
    ) -> Self {
        Self {
            map,
            scene,
            grid_source,
            grid_dims,
        }
    }

    fn visible_scene_coords(
        &self,
        visibility_tiles: Option<&[u8; VIEWPORT_VISIBILITY_LEN]>,
    ) -> Vec<(usize, usize)> {
        let Some(tiles) = visibility_tiles else {
            return Vec::new();
        };

        let mut coords = Vec::new();
        let (scene_w, scene_h) = self.scene.dimensions();
        let player_local = local_scene_player_tile(self.map);
        let player_local_x = player_local[0].round() as i32;
        let player_local_y = player_local[1].round() as i32;

        for vy in 0..VIEWPORT_VISIBILITY_HEIGHT {
            for vx in 0..VIEWPORT_VISIBILITY_WIDTH {
                let idx = vy * VIEWPORT_VISIBILITY_WIDTH + vx;
                if tiles[idx] == FOG_HIDDEN_TILE {
                    continue;
                }

                let dx = vx as i32 - VIEWPORT_VISIBILITY_WIDTH as i32 / 2;
                let dy = vy as i32 - VIEWPORT_VISIBILITY_HEIGHT as i32 / 2;

                match self.scene {
                    FogScene::Britannia | FogScene::Underworld => coords.push((
                        self.map.x.wrapping_add_signed(dx as i8) as usize,
                        self.map.y.wrapping_add_signed(dy as i8) as usize,
                    )),
                    FogScene::Local(_) => {
                        let x = player_local_x + dx;
                        let y = player_local_y + dy;
                        if (0..scene_w as i32).contains(&x) && (0..scene_h as i32).contains(&y) {
                            coords.push((x as usize, y as usize));
                        }
                    }
                }
            }
        }

        coords
    }

    fn scene_coord_for_grid_cell(&self, grid_x: usize, grid_y: usize) -> Option<(usize, usize)> {
        match self.scene {
            FogScene::Britannia | FogScene::Underworld => match self.grid_source {
                GridSource::Outdoor => {
                    let half_w = self.grid_dims.0 as i32 / 2;
                    let half_h = self.grid_dims.1 as i32 / 2;
                    Some((
                        (self.map.x as i32 - half_w + grid_x as i32).rem_euclid(256) as usize,
                        (self.map.y as i32 - half_h + grid_y as i32).rem_euclid(256) as usize,
                    ))
                }
                GridSource::Local => Some((
                    self.map.scroll_x.wrapping_add(grid_x as u8) as usize,
                    self.map.scroll_y.wrapping_add(grid_y as u8) as usize,
                )),
            },
            FogScene::Local(_) => Some((grid_x, grid_y)),
        }
    }

    fn grid_cell_for_scene_coord(&self, scene_x: usize, scene_y: usize) -> Option<(usize, usize)> {
        match self.scene {
            FogScene::Britannia | FogScene::Underworld => {
                let (top_left_x, top_left_y) = match self.grid_source {
                    GridSource::Outdoor => {
                        let half_w = self.grid_dims.0 as i32 / 2;
                        let half_h = self.grid_dims.1 as i32 / 2;
                        (
                            (self.map.x as i32 - half_w).rem_euclid(256) as usize,
                            (self.map.y as i32 - half_h).rem_euclid(256) as usize,
                        )
                    }
                    GridSource::Local => (self.map.scroll_x as usize, self.map.scroll_y as usize),
                };
                let grid_x = (scene_x + 256 - top_left_x) % 256;
                let grid_y = (scene_y + 256 - top_left_y) % 256;
                (grid_x < self.grid_dims.0 && grid_y < self.grid_dims.1).then_some((grid_x, grid_y))
            }
            FogScene::Local(_) => (scene_x < self.grid_dims.0 && scene_y < self.grid_dims.1)
                .then_some((scene_x, scene_y)),
        }
    }
}

fn show_dungeon_map(ui: &mut egui::Ui, state: &mut MinimapState, map: &MapState) {
    let rect = ui
        .available_rect_before_wrap()
        .round_to_pixels(ui.pixels_per_point());
    let _response = ui.allocate_rect(rect, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let grid = extract_dungeon_grid(&map.tiles);
    let panel_rounding = 10.0;
    let frame_margin = (rect.width().min(rect.height()) * 0.014).max(4.0);
    let map_rect = rect.shrink(frame_margin);
    let player_tile = local_scene_player_tile(map);

    painter.rect_filled(rect, panel_rounding, Color32::from_rgb(7, 10, 13));
    painter.rect_stroke(
        rect,
        panel_rounding,
        Stroke::new(1.0, Color32::from_rgb(32, 38, 46)),
        StrokeKind::Inside,
    );
    painter.rect_filled(map_rect, 7.0, Color32::from_rgb(12, 16, 20));
    painter.rect_stroke(
        map_rect,
        7.0,
        Stroke::new(1.0, Color32::from_rgb(18, 24, 30)),
        StrokeKind::Inside,
    );
    let map_painter = ui.painter_at(map_rect);
    match state.dungeon_primary_view() {
        DungeonPrimaryView::Centered => {
            paint_centered_dungeon_cells(&map_painter, map_rect, &grid, player_tile);
            if state.show_dungeon_corner_view() {
                paint_dungeon_overview_inset(
                    ui,
                    map_rect,
                    &grid,
                    player_tile,
                    map.dungeon_facing,
                    DungeonPrimaryView::Fixed,
                );
            }
            paint_centered_dungeon_player_marker(ui, map_rect, map.dungeon_facing);
        }
        DungeonPrimaryView::Fixed => {
            paint_fixed_dungeon_cells(&map_painter, map_rect, &grid);
            if state.show_dungeon_corner_view() {
                paint_dungeon_overview_inset(
                    ui,
                    map_rect,
                    &grid,
                    player_tile,
                    map.dungeon_facing,
                    DungeonPrimaryView::Centered,
                );
            }
            paint_player_marker(
                ui,
                map_rect,
                (DUNGEON_LEVEL_WIDTH as u32, DUNGEON_LEVEL_HEIGHT as u32),
                player_tile,
                map.dungeon_facing,
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DungeonCellKind {
    Corridor,
    Room,
    Wall,
    WallAlt,
    SecretDoor,
    Door,
    Ladder { up: bool, down: bool },
}

fn classify_dungeon_cell(code: u8) -> DungeonCellKind {
    match code >> 4 {
        0x1 => DungeonCellKind::Ladder {
            up: true,
            down: false,
        },
        0x2 => DungeonCellKind::Ladder {
            up: false,
            down: true,
        },
        0x3 => DungeonCellKind::Ladder {
            up: true,
            down: true,
        },
        0xA | 0xF => DungeonCellKind::Room,
        0xB => DungeonCellKind::Wall,
        0xC => DungeonCellKind::WallAlt,
        0xD => DungeonCellKind::SecretDoor,
        0xE => DungeonCellKind::Door,
        _ => DungeonCellKind::Corridor,
    }
}

fn paint_centered_dungeon_cells(
    painter: &egui::Painter,
    rect: Rect,
    grid: &[u8],
    player_tile: [f32; 2],
) {
    for repeat_y in -1..=1 {
        for repeat_x in -1..=1 {
            for y in 0..DUNGEON_LEVEL_HEIGHT {
                for x in 0..DUNGEON_LEVEL_WIDTH {
                    let idx = y * DUNGEON_LEVEL_WIDTH + x;
                    let world_tile = [
                        x as f32 + repeat_x as f32 * DUNGEON_LEVEL_WIDTH as f32,
                        y as f32 + repeat_y as f32 * DUNGEON_LEVEL_HEIGHT as f32,
                    ];
                    let cell = centered_dungeon_tile_rect(rect, player_tile, world_tile);
                    if !cell.intersects(rect) {
                        continue;
                    }
                    paint_dungeon_cell(painter, cell, classify_dungeon_cell(grid[idx]));
                }
            }
        }
    }
}

fn paint_fixed_dungeon_cells(painter: &egui::Painter, rect: Rect, grid: &[u8]) {
    for y in 0..DUNGEON_LEVEL_HEIGHT {
        for x in 0..DUNGEON_LEVEL_WIDTH {
            let idx = y * DUNGEON_LEVEL_WIDTH + x;
            let cell = fixed_dungeon_tile_rect(rect, x, y);
            paint_dungeon_cell(painter, cell, classify_dungeon_cell(grid[idx]));
        }
    }
}

fn fixed_dungeon_tile_rect(rect: Rect, x: usize, y: usize) -> Rect {
    let cell_w = rect.width() / DUNGEON_LEVEL_WIDTH as f32;
    let cell_h = rect.height() / DUNGEON_LEVEL_HEIGHT as f32;
    Rect::from_min_max(
        Pos2::new(
            rect.left() + x as f32 * cell_w,
            rect.top() + y as f32 * cell_h,
        ),
        Pos2::new(
            rect.left() + (x + 1) as f32 * cell_w,
            rect.top() + (y + 1) as f32 * cell_h,
        ),
    )
}

fn centered_dungeon_tile_rect(rect: Rect, player_tile: [f32; 2], world_tile: [f32; 2]) -> Rect {
    let cell_w = rect.width() / DUNGEON_LEVEL_WIDTH as f32;
    let cell_h = rect.height() / DUNGEON_LEVEL_HEIGHT as f32;
    let center = Pos2::new(
        rect.center().x + (world_tile[0] - player_tile[0]) * cell_w,
        rect.center().y + (world_tile[1] - player_tile[1]) * cell_h,
    );
    Rect::from_center_size(center, vec2(cell_w, cell_h))
}

fn paint_dungeon_overview_inset(
    ui: &egui::Ui,
    map_rect: Rect,
    grid: &[u8],
    player_tile: [f32; 2],
    facing: Option<CardinalDirection>,
    inset_view: DungeonPrimaryView,
) {
    let frame = dungeon_overview_rect(map_rect);
    let panel = ui.painter_at(frame);
    let shadow = frame.translate(vec2(0.0, 3.0));
    panel.rect_filled(shadow, 9.0, Color32::from_black_alpha(88));
    panel.rect_filled(frame, 9.0, Color32::from_rgb(11, 15, 20));
    panel.rect_stroke(
        frame,
        9.0,
        Stroke::new(1.0, Color32::from_rgb(38, 46, 56)),
        StrokeKind::Inside,
    );

    let grid_rect = frame.shrink(frame.width() * 0.075);
    panel.rect_filled(grid_rect, 6.0, Color32::from_rgb(9, 12, 16));
    panel.rect_stroke(
        grid_rect,
        6.0,
        Stroke::new(1.0, Color32::from_rgb(24, 30, 38)),
        StrokeKind::Inside,
    );

    match inset_view {
        DungeonPrimaryView::Fixed => {
            paint_fixed_dungeon_cells(&panel, grid_rect, grid);
            paint_player_marker(
                ui,
                grid_rect,
                (DUNGEON_LEVEL_WIDTH as u32, DUNGEON_LEVEL_HEIGHT as u32),
                player_tile,
                facing,
            );
        }
        DungeonPrimaryView::Centered => {
            paint_centered_dungeon_cells(&panel, grid_rect, grid, player_tile);
            paint_centered_dungeon_player_marker(ui, grid_rect, facing);
        }
    }
}

fn dungeon_overview_rect(map_rect: Rect) -> Rect {
    let margin = (map_rect.width() * 0.028).clamp(2.0, 8.0);
    let available_side = (map_rect.width() - 2.0 * margin)
        .min(map_rect.height() - 2.0 * margin)
        .max(8.0);
    let preferred_side = (map_rect.width() * 0.20).max(8.0);
    let side = preferred_side.min(available_side);
    Rect::from_min_size(
        Pos2::new(map_rect.right() - margin - side, map_rect.top() + margin),
        vec2(side, side),
    )
}

fn parse_dungeon_primary_view(value: &str) -> Option<DungeonPrimaryView> {
    match value.trim() {
        "centered" => Some(DungeonPrimaryView::Centered),
        "fixed" => Some(DungeonPrimaryView::Fixed),
        _ => None,
    }
}

fn load_dungeon_primary_view() -> DungeonPrimaryView {
    minimap_pref_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|value| parse_dungeon_primary_view(&value))
        .unwrap_or_default()
}

fn save_dungeon_primary_view(view: DungeonPrimaryView) {
    let Some(path) = minimap_pref_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, view.as_str());
}

fn load_dungeon_corner_view() -> bool {
    bool_pref_path(DUNGEON_CORNER_VIEW_PREF_FILE)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|value| parse_bool_pref(&value))
        .unwrap_or(true)
}

fn save_dungeon_corner_view(visible: bool) {
    let Some(path) = bool_pref_path(DUNGEON_CORNER_VIEW_PREF_FILE) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, if visible { "true" } else { "false" });
}

fn parse_bool_pref(value: &str) -> Option<bool> {
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn minimap_pref_path() -> Option<PathBuf> {
    bool_pref_path(DUNGEON_VIEW_PREF_FILE)
}

fn bool_pref_path(file_name: &str) -> Option<PathBuf> {
    crate::preferences::appdata_file_path(file_name)
}

fn paint_dungeon_cell(painter: &egui::Painter, rect: Rect, kind: DungeonCellKind) {
    let outer = rect.shrink(rect.width().min(rect.height()) * 0.045);
    let inner = outer.shrink(outer.width().min(outer.height()) * 0.11);

    match kind {
        DungeonCellKind::Corridor => paint_dungeon_floor_tile(
            painter,
            outer,
            inner,
            Color32::from_rgb(32, 39, 46),
            Color32::from_rgb(23, 29, 35),
        ),
        DungeonCellKind::Room => paint_dungeon_floor_tile(
            painter,
            outer,
            inner,
            Color32::from_rgb(46, 68, 83),
            Color32::from_rgb(34, 49, 60),
        ),
        DungeonCellKind::Wall => paint_dungeon_wall_tile(
            painter,
            outer,
            inner,
            Color32::from_rgb(182, 191, 202),
            Color32::from_rgb(112, 122, 134),
        ),
        DungeonCellKind::WallAlt => paint_dungeon_wall_tile(
            painter,
            outer,
            inner,
            Color32::from_rgb(191, 185, 176),
            Color32::from_rgb(121, 116, 110),
        ),
        DungeonCellKind::SecretDoor => {
            paint_dungeon_wall_tile(
                painter,
                outer,
                inner,
                Color32::from_rgb(182, 191, 202),
                Color32::from_rgb(112, 122, 134),
            );
            paint_secret_door_seam(painter, inner);
        }
        DungeonCellKind::Door => {
            paint_dungeon_floor_tile(
                painter,
                outer,
                inner,
                Color32::from_rgb(32, 39, 46),
                Color32::from_rgb(23, 29, 35),
            );
            paint_door_glyph(painter, inner);
        }
        DungeonCellKind::Ladder { up, down } => {
            paint_dungeon_floor_tile(
                painter,
                outer,
                inner,
                Color32::from_rgb(32, 39, 46),
                Color32::from_rgb(23, 29, 35),
            );
            paint_ladder_icon(painter, inner, up, down);
        }
    }
}

fn paint_dungeon_floor_tile(
    painter: &egui::Painter,
    outer: Rect,
    inner: Rect,
    rim: Color32,
    fill: Color32,
) {
    painter.rect_filled(outer, 4.0, rim);
    painter.rect_filled(inner, 3.0, fill);
    painter.line_segment(
        [
            Pos2::new(inner.left(), inner.top() + 1.0),
            Pos2::new(inner.right(), inner.top() + 1.0),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(12)),
    );
    painter.line_segment(
        [
            Pos2::new(inner.left(), inner.bottom() - 1.0),
            Pos2::new(inner.right(), inner.bottom() - 1.0),
        ],
        Stroke::new(1.0, Color32::from_black_alpha(70)),
    );
}

fn paint_dungeon_wall_tile(
    painter: &egui::Painter,
    outer: Rect,
    inner: Rect,
    fill: Color32,
    base: Color32,
) {
    painter.rect_filled(outer, 3.0, base);
    painter.rect_filled(inner, 2.0, fill);

    let mortar = Color32::from_black_alpha(48);
    for y in [
        inner.top() + inner.height() * 0.33,
        inner.top() + inner.height() * 0.66,
    ] {
        painter.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            Stroke::new(1.0, mortar),
        );
    }

    painter.line_segment(
        [
            Pos2::new(inner.left(), inner.top()),
            Pos2::new(inner.right(), inner.top()),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(44)),
    );
    painter.line_segment(
        [
            Pos2::new(inner.left(), inner.bottom()),
            Pos2::new(inner.right(), inner.bottom()),
        ],
        Stroke::new(1.0, Color32::from_black_alpha(88)),
    );
}

fn paint_ladder_icon(painter: &egui::Painter, rect: Rect, up: bool, down: bool) {
    let plaque = Rect::from_center_size(
        rect.center(),
        vec2(rect.width() * 0.52, rect.height() * 0.76),
    );
    painter.rect_filled(plaque, 5.0, Color32::from_rgb(22, 26, 31));
    painter.rect_stroke(
        plaque,
        5.0,
        Stroke::new(1.0, Color32::from_rgb(54, 60, 70)),
        StrokeKind::Inside,
    );

    let left = plaque.left() + plaque.width() * 0.28;
    let right = plaque.right() - plaque.width() * 0.28;
    let top = plaque.top() + plaque.height() * 0.18;
    let bottom = plaque.bottom() - plaque.height() * 0.18;
    let gold = Color32::from_rgb(233, 197, 88);
    let gold_shadow = Color32::from_rgb(122, 88, 24);

    for x in [left, right] {
        painter.line_segment(
            [Pos2::new(x, top), Pos2::new(x, bottom)],
            Stroke::new(3.0, gold_shadow),
        );
        painter.line_segment(
            [Pos2::new(x, top), Pos2::new(x, bottom)],
            Stroke::new(1.8, gold),
        );
    }

    for t in [0.18, 0.38, 0.58, 0.78] {
        let y = egui::lerp(top..=bottom, t);
        painter.line_segment(
            [Pos2::new(left, y), Pos2::new(right, y)],
            Stroke::new(1.6, gold),
        );
    }

    if up {
        paint_ladder_badge(
            painter,
            Pos2::new(plaque.center().x, plaque.top() + plaque.height() * 0.11),
            plaque.width() * 0.34,
            true,
        );
    }
    if down {
        paint_ladder_badge(
            painter,
            Pos2::new(plaque.center().x, plaque.bottom() - plaque.height() * 0.11),
            plaque.width() * 0.34,
            false,
        );
    }
}

fn paint_ladder_badge(painter: &egui::Painter, center: Pos2, size: f32, up: bool) {
    painter.circle_filled(center, size * 0.5, Color32::from_rgb(15, 20, 26));
    painter.circle_stroke(
        center,
        size * 0.5,
        Stroke::new(1.2, Color32::from_rgb(106, 198, 248)),
    );

    let half_w = size * 0.2;
    let tip_offset = size * 0.18;
    let base_offset = size * 0.12;
    let points = if up {
        [
            Pos2::new(center.x, center.y - tip_offset),
            Pos2::new(center.x - half_w, center.y + base_offset),
            Pos2::new(center.x + half_w, center.y + base_offset),
        ]
    } else {
        [
            Pos2::new(center.x, center.y + tip_offset),
            Pos2::new(center.x - half_w, center.y - base_offset),
            Pos2::new(center.x + half_w, center.y - base_offset),
        ]
    };
    paint_triangle(painter, points, Color32::from_rgb(124, 220, 255));
}

fn paint_secret_door_seam(painter: &egui::Painter, rect: Rect) {
    let x = rect.center().x;
    let dash = (rect.height() * 0.12).max(2.0);
    let gap = dash * 0.72;
    let mut y = rect.top() + rect.height() * 0.12;
    while y < rect.bottom() - dash {
        painter.line_segment(
            [Pos2::new(x, y), Pos2::new(x, y + dash)],
            Stroke::new(1.6, Color32::from_rgb(210, 186, 108)),
        );
        y += dash + gap;
    }
}

fn paint_door_glyph(painter: &egui::Painter, rect: Rect) {
    let door = Rect::from_center_size(
        rect.center(),
        vec2(rect.width() * 0.46, rect.height() * 0.66),
    );
    painter.rect_filled(door, 4.0, Color32::from_rgb(178, 141, 66));
    painter.rect_stroke(
        door,
        4.0,
        Stroke::new(1.2, Color32::from_rgb(78, 58, 20)),
        StrokeKind::Inside,
    );
    for t in [0.28, 0.52, 0.76] {
        let y = egui::lerp(door.top()..=door.bottom(), t);
        painter.line_segment(
            [
                Pos2::new(door.left() + 2.0, y),
                Pos2::new(door.right() - 2.0, y),
            ],
            Stroke::new(1.0, Color32::from_rgb(96, 72, 28)),
        );
    }
}

fn paint_triangle(painter: &egui::Painter, points: [Pos2; 3], fill: Color32) {
    painter.add(egui::Shape::convex_polygon(
        points.to_vec(),
        fill,
        Stroke::new(1.0, Color32::BLACK),
    ));
}

/// Decode the combat terrain scratch grid.
///
/// Combat stores an 11x11 active battlefield in the first 11 columns of a
/// 32-byte-stride buffer. The remaining bytes in each row are unrelated combat
/// tables, so they must be discarded.
fn extract_combat_grid(tiles: &[u8; COMBAT_TERRAIN_LEN]) -> Vec<u8> {
    let mut grid = vec![0u8; COMBAT_TERRAIN_WIDTH * COMBAT_TERRAIN_HEIGHT];
    for y in 0..COMBAT_TERRAIN_HEIGHT {
        let src = y * COMBAT_TERRAIN_STRIDE;
        let dst = y * COMBAT_TERRAIN_WIDTH;
        grid[dst..dst + COMBAT_TERRAIN_WIDTH]
            .copy_from_slice(&tiles[src..src + COMBAT_TERRAIN_WIDTH]);
    }
    grid
}

/// Build an object overlay grid (same dimensions as the tile grid).
///
/// Each cell is 0 (no object) or the object's tile byte. The shader adds 256
/// to get the animated-page sprite from the atlas.
fn build_objects_overlay(
    objects: &[ObjectEntry],
    grid_w: usize,
    grid_h: usize,
    map: &MapState,
) -> Vec<u8> {
    debug_assert!(
        !matches!(map.location, LocationType::Dungeon(_)),
        "dungeon walking maps use the abstract painter path instead of sprite overlays"
    );
    let mut overlay = vec![0u8; grid_w * grid_h];
    let half_w = grid_w as i32 / 2;
    let half_h = grid_h as i32 / 2;
    let outdoor = map.is_outdoor();
    let combat = matches!(map.location, LocationType::Combat(_));

    for obj in objects {
        if !outdoor && obj.floor != map.z {
            continue;
        }

        let (gx, gy) = if outdoor {
            let vx = (obj.x as i32 - map.x as i32 + half_w).rem_euclid(256);
            let vy = (obj.y as i32 - map.y as i32 + half_h).rem_euclid(256);
            (vx, vy)
        } else if combat {
            // Combat actors already use battlefield-local coordinates even when
            // scroll_x/scroll_y still contain overworld chunk state.
            (obj.x as i32, obj.y as i32)
        } else {
            (
                obj.x as i32 - map.scroll_x as i32,
                obj.y as i32 - map.scroll_y as i32,
            )
        };

        if gx >= 0 && gx < grid_w as i32 && gy >= 0 && gy < grid_h as i32 {
            overlay[gy as usize * grid_w + gx as usize] = obj.tile;
        }
    }

    overlay
}

/// Collapse each atlas tile to a representative color for overview rendering.
///
/// Palette-0 black in the Ultima V art is mostly outline/shadow detail. Ignore
/// it while building zoomed-out colors so repeated pixel-art patterns do not
/// darken the minimap during minification. Keep the surviving coverage for all
/// tiles so the shader can apply a perceptual brightness correction as those
/// outline/shadow texels disappear.
fn build_tile_lowpass_lut(atlas_rgba: &[u8]) -> Vec<LowpassTileSample> {
    assert_eq!(
        atlas_rgba.len(),
        TILE_COUNT * TILE_RGBA_BYTES,
        "tile low-pass LUT requires one 16x16 RGBA tile for each atlas entry"
    );

    let mut lut = Vec::with_capacity(TILE_COUNT);
    for tile_idx in 0..TILE_COUNT {
        let tile = &atlas_rgba[tile_idx * TILE_RGBA_BYTES..(tile_idx + 1) * TILE_RGBA_BYTES];
        let mut sum = [0u32; 3];
        let mut opaque_pixels = 0u32;

        for px in tile.chunks_exact(4) {
            let visible = !is_ega_black_rgba(px);
            if !visible {
                continue;
            }
            sum[0] += px[0] as u32;
            sum[1] += px[1] as u32;
            sum[2] += px[2] as u32;
            opaque_pixels += 1;
        }

        let coverage = ((opaque_pixels * 255 + (TILE_SIZE * TILE_SIZE / 2) as u32)
            / (TILE_SIZE * TILE_SIZE) as u32) as u8;

        let rgb = if opaque_pixels == 0 {
            [0, 0, 0]
        } else {
            [
                (sum[0] / opaque_pixels) as u8,
                (sum[1] / opaque_pixels) as u8,
                (sum[2] / opaque_pixels) as u8,
            ]
        };

        lut.push(LowpassTileSample { rgb, coverage });
    }

    lut
}

/// Build a premultiplied RGBA low-pass map that stays stable when multiple
/// tiles collapse into a single screen pixel.
///
/// RGB stores the visible-color average multiplied by non-black coverage, and
/// alpha stores that coverage directly. The shader uses the coverage channel
/// to put back the perceived darkness lost when palette-0 black texels are
/// dropped during minification.
fn build_lowpass_map(
    grid_data: &[u8],
    objects_data: &[u8],
    tile_lut: &[LowpassTileSample],
    grid_w: usize,
    grid_h: usize,
) -> Vec<u8> {
    let texels = grid_w * grid_h;
    assert_eq!(
        grid_data.len(),
        texels,
        "terrain grid length must match dimensions"
    );
    assert_eq!(
        objects_data.len(),
        texels,
        "object grid length must match dimensions"
    );
    assert_eq!(
        tile_lut.len(),
        TILE_COUNT,
        "tile LUT must cover the full atlas"
    );

    let mut lowpass = vec![0u8; texels * 4];
    for idx in 0..texels {
        let mut sample =
            PremultipliedCoverageSample::from_lowpass_tile(tile_lut[grid_data[idx] as usize]);

        let object_id = objects_data[idx];
        if object_id != 0 {
            sample = sample.composite_over(tile_lut[object_id as usize + 256]);
        }

        let out = &mut lowpass[idx * 4..idx * 4 + 4];
        out.copy_from_slice(&sample.to_rgba());
    }

    lowpass
}

#[derive(Clone, Copy)]
struct VisibleWorldLocation<'a> {
    location: &'a WorldLocation,
    point: Pos2,
    distance_sq: i32,
}

/// Paint markers and optional labels for visible overworld points of interest.
fn paint_overworld_overlay(
    ui: &egui::Ui,
    rect: Rect,
    world_map: &WorldMap,
    overlay: OverworldOverlayOptions,
    fog_visibility: Option<&[u8]>,
) {
    let mut visible = visible_world_locations(
        world_map.locations(),
        rect,
        overlay.cx,
        overlay.cy,
        overlay.grid_dims,
    );
    if visible.is_empty() {
        return;
    }

    visible.sort_by_key(|entry| {
        (
            world_label_priority(entry.location.category()),
            entry.distance_sq,
            entry.location.name().len(),
        )
    });

    let painter = ui.painter_at(rect);
    for entry in &visible {
        if !is_overworld_location_revealed(entry.location, overlay, fog_visibility) {
            continue;
        }
        let fill = world_marker_color(entry.location.category());
        painter.circle_filled(entry.point, 3.5, fill);
        painter.circle_stroke(entry.point, 4.5, Stroke::new(1.0, Color32::BLACK));
    }

    if !overlay.show_labels {
        return;
    }

    let font_size = match overlay.grid_dims.1 {
        0..=48 => 12.0,
        49..=96 => 11.0,
        _ => 10.0,
    };
    let font_id = FontId::proportional(font_size);
    let mut occupied = Vec::new();

    for entry in &visible {
        if !is_overworld_location_revealed(entry.location, overlay, fog_visibility) {
            continue;
        }
        if !overlay.label_filters.shows(entry.location.category()) {
            continue;
        }
        let text = entry.location.name();
        let galley = painter.layout_no_wrap(text.to_owned(), font_id.clone(), Color32::WHITE);
        let label_rect = world_label_rect(rect, entry.point, galley.rect.size());
        if occupied
            .iter()
            .any(|other: &Rect| other.intersects(label_rect))
        {
            continue;
        }

        painter.rect_filled(
            label_rect.expand2(vec2(4.0, 2.0)),
            4.0,
            Color32::from_black_alpha(180),
        );
        painter.galley(label_rect.min, galley, Color32::WHITE);
        occupied.push(label_rect.expand2(vec2(6.0, 4.0)));
    }
}

fn is_overworld_location_revealed(
    location: &WorldLocation,
    overlay: OverworldOverlayOptions,
    fog_visibility: Option<&[u8]>,
) -> bool {
    let Some(fog_visibility) = fog_visibility else {
        return true;
    };
    let half_w = overlay.grid_dims.0 as i32 / 2;
    let half_h = overlay.grid_dims.1 as i32 / 2;
    let top_left_x = (overlay.cx as i32 - half_w).rem_euclid(256) as usize;
    let top_left_y = (overlay.cy as i32 - half_h).rem_euclid(256) as usize;
    let grid_x = (location.x as usize + 256 - top_left_x) % 256;
    let grid_y = (location.y as usize + 256 - top_left_y) % 256;
    if grid_x >= overlay.grid_dims.0 || grid_y >= overlay.grid_dims.1 {
        return false;
    }
    fog_visibility[grid_y * overlay.grid_dims.0 + grid_x] != FOG_VISIBILITY_UNSEEN
}

/// Paint the current player location with a screen-space marker that keeps a
/// readable minimum size while still expanding to nearly fill the tile at
/// close zoom.
fn paint_player_marker(
    ui: &egui::Ui,
    rect: Rect,
    grid_dims: (u32, u32),
    player_tile: [f32; 2],
    facing: Option<CardinalDirection>,
) {
    if grid_dims.0 == 0 || grid_dims.1 == 0 {
        return;
    }

    let center = player_marker_center(rect, grid_dims, player_tile);
    let tile_span_px = (rect.width() / grid_dims.0 as f32).min(rect.height() / grid_dims.1 as f32);
    paint_player_marker_at(ui, rect, center, tile_span_px, facing);
}

fn paint_centered_dungeon_player_marker(
    ui: &egui::Ui,
    rect: Rect,
    facing: Option<CardinalDirection>,
) {
    let tile_span_px = (rect.width() / DUNGEON_LEVEL_WIDTH as f32)
        .min(rect.height() / DUNGEON_LEVEL_HEIGHT as f32);
    paint_player_marker_at(ui, rect, rect.center(), tile_span_px, facing);
}

fn paint_player_marker_at(
    ui: &egui::Ui,
    rect: Rect,
    center: Pos2,
    tile_span_px: f32,
    facing: Option<CardinalDirection>,
) {
    let style = player_marker_style(tile_span_px);
    let painter = ui.painter_at(rect);

    if style.filled {
        painter.circle_filled(center, style.radius, PLAYER_MARKER_COLOR);
        painter.circle_stroke(center, style.radius + 0.5, Stroke::new(1.5, Color32::BLACK));
    } else {
        painter.circle_stroke(center, style.radius + 0.5, Stroke::new(3.0, Color32::BLACK));
        painter.circle_stroke(center, style.radius, Stroke::new(1.75, PLAYER_MARKER_COLOR));
    }

    if let Some(facing) = facing {
        paint_player_facing_arrow(&painter, center, style, facing);
    }
}

fn paint_player_facing_arrow(
    painter: &egui::Painter,
    center: Pos2,
    style: PlayerMarkerStyle,
    facing: CardinalDirection,
) {
    let unit = facing_unit_vector(facing);
    let perp = vec2(-unit.y, unit.x);
    let tip_distance =
        (style.radius + PLAYER_FACING_ARROW_EXTRA_LEN).max(PLAYER_FACING_ARROW_MIN_LEN);
    let base_distance = (style.radius * 0.45).max(2.0);
    let half_width = PLAYER_FACING_ARROW_WIDTH.max(style.radius * 0.35);

    let tip = center + unit * tip_distance;
    let base_center = center + unit * base_distance;
    let left = base_center + perp * half_width;
    let right = base_center - perp * half_width;

    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        Color32::BLACK,
        Stroke::NONE,
    ));

    let inner_tip = center + unit * (tip_distance - 1.5);
    let inner_base_center = center + unit * (base_distance + 0.5);
    let inner_half_width = (half_width - 1.5).max(1.5);
    let inner_left = inner_base_center + perp * inner_half_width;
    let inner_right = inner_base_center - perp * inner_half_width;

    painter.add(egui::Shape::convex_polygon(
        vec![inner_tip, inner_left, inner_right],
        PLAYER_MARKER_COLOR,
        Stroke::NONE,
    ));
}

fn facing_unit_vector(facing: CardinalDirection) -> egui::Vec2 {
    match facing {
        CardinalDirection::North => vec2(0.0, -1.0),
        CardinalDirection::East => vec2(1.0, 0.0),
        CardinalDirection::South => vec2(0.0, 1.0),
        CardinalDirection::West => vec2(-1.0, 0.0),
    }
}

/// Map a tile-space player position to the pixel center of the corresponding
/// tile in the minimap rectangle.
fn player_marker_center(rect: Rect, grid_dims: (u32, u32), player_tile: [f32; 2]) -> Pos2 {
    let tile_x = player_tile[0].floor();
    let tile_y = player_tile[1].floor();
    Pos2::new(
        rect.left() + (tile_x + 0.5) / grid_dims.0 as f32 * rect.width(),
        rect.top() + (tile_y + 0.5) / grid_dims.1 as f32 * rect.height(),
    )
}

/// Pick a marker size with a readable minimum screen-space footprint, then let
/// it grow with the visible tile size until it nearly fills the tile. Switch
/// to a solid dot once tiles are too small to preserve an obvious hollow
/// center.
fn player_marker_style(tile_span_px: f32) -> PlayerMarkerStyle {
    PlayerMarkerStyle {
        radius: (tile_span_px * 0.5 - PLAYER_MARKER_TILE_MARGIN_PX).max(PLAYER_MARKER_MIN_RADIUS),
        filled: tile_span_px <= PLAYER_MARKER_FILL_TILE_THRESHOLD_PX,
    }
}

/// Return the visible overworld locations after applying world-wrap projection.
fn visible_world_locations<'a>(
    locations: &'a [WorldLocation],
    rect: Rect,
    cx: u8,
    cy: u8,
    grid_dims: (usize, usize),
) -> Vec<VisibleWorldLocation<'a>> {
    let half_w = grid_dims.0 as i32 / 2;
    let half_h = grid_dims.1 as i32 / 2;
    let mut visible = Vec::new();

    for location in locations {
        let dx = wrapped_delta(location.x, cx) as i32;
        let dy = wrapped_delta(location.y, cy) as i32;
        let tile_x = dx + half_w;
        let tile_y = dy + half_h;
        if tile_x < 0 || tile_y < 0 || tile_x >= grid_dims.0 as i32 || tile_y >= grid_dims.1 as i32
        {
            continue;
        }

        let point = Pos2::new(
            rect.left() + (tile_x as f32 + 0.5) / grid_dims.0 as f32 * rect.width(),
            rect.top() + (tile_y as f32 + 0.5) / grid_dims.1 as f32 * rect.height(),
        );
        visible.push(VisibleWorldLocation {
            location,
            point,
            distance_sq: dx * dx + dy * dy,
        });
    }

    visible
}

/// Return whether the cached tile grid must be rebuilt for the current frame.
fn needs_grid_refresh(
    last_grid_key: Option<GridCacheKey>,
    grid_key: GridCacheKey,
    has_objects: bool,
) -> bool {
    last_grid_key != Some(grid_key) || has_objects
}

/// Position a label near its marker while clamping it into the visible map area.
fn world_label_rect(bounds: Rect, point: Pos2, label_size: egui::Vec2) -> Rect {
    let center = bounds.center();
    let mut min = Pos2::new(
        if point.x < center.x {
            point.x - label_size.x - 8.0
        } else {
            point.x + 8.0
        },
        if point.y < center.y {
            point.y - label_size.y - 6.0
        } else {
            point.y + 6.0
        },
    );

    min.x = min.x.clamp(bounds.left(), bounds.right() - label_size.x);
    min.y = min.y.clamp(bounds.top(), bounds.bottom() - label_size.y);
    Rect::from_min_size(min, label_size)
}

/// Compute the shortest wrapped delta on the 256x256 overworld map.
fn wrapped_delta(coord: u8, center: u8) -> i16 {
    let mut delta = coord as i16 - center as i16;
    if delta > 127 {
        delta -= 256;
    } else if delta < -128 {
        delta += 256;
    }
    delta
}

/// Sort more important location categories ahead of less important ones.
fn world_label_priority(category: WorldLabelCategory) -> u8 {
    match category {
        WorldLabelCategory::Shrine => 0,
        WorldLabelCategory::Town => 1,
        WorldLabelCategory::Castle | WorldLabelCategory::Keep => 2,
        WorldLabelCategory::Dungeon => 3,
        WorldLabelCategory::Dwelling => 4,
    }
}

/// Pick a marker color for each overworld location category.
fn world_marker_color(category: WorldLabelCategory) -> Color32 {
    match category {
        WorldLabelCategory::Town => Color32::from_rgb(240, 212, 106),
        WorldLabelCategory::Dwelling => Color32::from_rgb(119, 201, 145),
        WorldLabelCategory::Castle | WorldLabelCategory::Keep => Color32::from_rgb(127, 169, 255),
        WorldLabelCategory::Dungeon => Color32::from_rgb(226, 110, 110),
        WorldLabelCategory::Shrine => Color32::from_rgb(120, 224, 224),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::LocationType;
    use crate::game::offsets::VIEWPORT_VISIBILITY_LEN;
    use crate::game::quest::Virtue;
    use crate::game::world_map::{WorldLabelKind, WorldLocation};

    #[test]
    fn wrapped_delta_prefers_shortest_world_distance() {
        assert_eq!(wrapped_delta(2, 250), 8);
        assert_eq!(wrapped_delta(250, 2), -8);
        assert_eq!(wrapped_delta(42, 40), 2);
    }

    #[test]
    fn visible_world_locations_wrap_at_map_edges() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(128.0, 128.0));
        let locations = [WorldLocation {
            kind: WorldLabelKind::Location(LocationType::Town(1)),
            x: 1,
            y: 250,
        }];

        let visible = visible_world_locations(&locations, rect, 250, 250, (16, 16));
        assert_eq!(visible.len(), 1);
        assert!(visible[0].point.x > rect.center().x);
        assert!((visible[0].point.y - rect.center().y).abs() <= rect.height() / 16.0);
    }

    #[test]
    fn outdoor_grid_dims_expand_to_match_wide_panel() {
        assert_eq!(
            outdoor_grid_dims_for_panel(vec2(400.0, 200.0), 48),
            (99, 49)
        );
    }

    #[test]
    fn outdoor_grid_dims_clamp_before_world_wrap_repeats() {
        assert_eq!(
            outdoor_grid_dims_for_panel(vec2(900.0, 180.0), 64),
            (255, 65)
        );
    }

    #[test]
    fn outdoor_grid_dims_prefer_odd_spans_for_true_centering() {
        assert_eq!(
            outdoor_grid_dims_for_panel(vec2(600.0, 200.0), 48),
            (147, 49)
        );
    }

    #[test]
    fn fit_rect_to_grid_adds_side_bars_when_grid_hits_wrap_limit() {
        let bounds = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(900.0, 180.0));
        let fitted = fit_rect_to_grid(bounds, (255, 63));

        assert_eq!(fitted.height(), bounds.height());
        assert!(fitted.width() < bounds.width());
        assert_eq!(fitted.center().y, bounds.center().y);
    }

    #[test]
    fn player_marker_turns_filled_once_tiles_get_too_small() {
        assert!(!player_marker_style(8.0).filled);
        assert!(player_marker_style(6.0).filled);
        assert!(player_marker_style(2.0).filled);
    }

    #[test]
    fn player_marker_radius_has_screen_space_floor_and_grows_with_tiles() {
        assert_eq!(player_marker_style(1.0).radius, PLAYER_MARKER_MIN_RADIUS);
        assert_eq!(player_marker_style(32.0).radius, 13.0);
    }

    #[test]
    fn player_marker_center_tracks_tile_center() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), vec2(320.0, 320.0));
        let point = player_marker_center(rect, (32, 32), [5.0, 6.0]);

        assert_eq!(point, Pos2::new(65.0, 85.0));
    }

    #[test]
    fn player_marker_center_matches_odd_zoom_tile_snap() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(110.0, 110.0));
        let point = player_marker_center(rect, (11, 11), [5.5, 5.5]);

        assert_eq!(point, Pos2::new(55.0, 55.0));
    }

    #[test]
    fn facing_unit_vector_matches_cardinal_axes() {
        assert_eq!(
            facing_unit_vector(CardinalDirection::North),
            vec2(0.0, -1.0)
        );
        assert_eq!(facing_unit_vector(CardinalDirection::East), vec2(1.0, 0.0));
        assert_eq!(facing_unit_vector(CardinalDirection::South), vec2(0.0, 1.0));
        assert_eq!(facing_unit_vector(CardinalDirection::West), vec2(-1.0, 0.0));
    }

    #[test]
    fn label_filters_can_hide_shrines_without_hiding_towns() {
        let filters = LabelFilters {
            shrines: false,
            ..LabelFilters::default()
        };

        assert!(filters.shows(WorldLabelCategory::Town));
        assert!(!filters.shows(WorldLabelCategory::Shrine));
    }

    #[test]
    fn shrine_points_report_shrine_category() {
        let shrine = WorldLocation {
            kind: WorldLabelKind::Shrine(Virtue::Honor),
            x: 0,
            y: 0,
        };

        assert_eq!(shrine.category(), WorldLabelCategory::Shrine);
        assert_eq!(shrine.name(), "Honor");
    }

    #[test]
    fn grid_source_change_invalidates_cached_grid() {
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (42, 43),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Local,
                local_map: Some((LocationType::Town(1), 0)),
                outdoor_z: None,
            }),
            GridCacheKey {
                center: (42, 43),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            },
            false,
        ));
        assert!(!needs_grid_refresh(
            Some(GridCacheKey {
                center: (42, 43),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            }),
            GridCacheKey {
                center: (42, 43),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            },
            false,
        ));
    }

    #[test]
    fn local_map_change_invalidates_cached_grid() {
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (12, 9),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Local,
                local_map: Some((LocationType::Town(1), 0)),
                outdoor_z: None,
            }),
            GridCacheKey {
                center: (12, 9),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Local,
                local_map: Some((LocationType::Town(2), 0)),
                outdoor_z: None,
            },
            false,
        ));
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (12, 9),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Local,
                local_map: Some((LocationType::Dungeon(33), 0)),
                outdoor_z: None,
            }),
            GridCacheKey {
                center: (12, 9),
                zoom: 32,
                grid_dims: (32, 32),
                source: GridSource::Local,
                local_map: Some((LocationType::Dungeon(33), 1)),
                outdoor_z: None,
            },
            false,
        ));
    }

    #[test]
    fn outdoor_layer_change_invalidates_cached_grid() {
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (99, 68),
                zoom: 48,
                grid_dims: (48, 48),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            }),
            GridCacheKey {
                center: (99, 68),
                zoom: 48,
                grid_dims: (48, 48),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0xFF),
            },
            false,
        ));
    }

    #[test]
    fn grid_dims_change_invalidates_cached_grid() {
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (99, 68),
                zoom: 48,
                grid_dims: (48, 48),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            }),
            GridCacheKey {
                center: (99, 68),
                zoom: 48,
                grid_dims: (96, 48),
                source: GridSource::Outdoor,
                local_map: None,
                outdoor_z: Some(0),
            },
            false,
        ));
    }

    #[test]
    fn clear_resets_cached_grid_state() {
        let mut state = MinimapState::new();
        state.map = Some(MapState {
            location: LocationType::Overworld,
            z: 0xFF,
            x: 1,
            y: 2,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        });
        state.last_grid_key = Some(GridCacheKey {
            center: (1, 2),
            zoom: 32,
            grid_dims: (32, 32),
            source: GridSource::Outdoor,
            local_map: None,
            outdoor_z: Some(0xFF),
        });
        state.raw_atlas = Some(Arc::new(vec![1, 2, 3]));
        state.fog_reset_error = Some("disk error".to_string());
        state.fog_reset_target_game_key = Some("game-key".to_string());

        {
            let mut gpu = state.gpu.lock().unwrap();
            gpu.grid_dirty = true;
            gpu.fog_dirty = true;
            gpu.grid_data = vec![1];
            gpu.objects_data = vec![2];
            gpu.lowpass_data = vec![3, 4, 5, 255];
            gpu.fog_data = vec![255; 12];
            gpu.grid_dims = (3, 4);
            gpu.player_tile = [5.0, 6.0];
        }

        state.clear();

        assert!(state.map.is_none());
        assert!(state.raw_atlas.is_none());
        assert!(state.last_grid_key.is_none());
        assert!(state.last_stable_visibility_scene.is_none());
        assert!(state.last_stable_visibility_tiles.is_none());
        assert!(state.fog_reset_error().is_none());
        assert!(state.fog_reset_target_game_key.is_none());

        let gpu = state.gpu.lock().unwrap();
        // clear() must drop the GL-backed renderer so the next paint callback rebuilds it.
        assert!(gpu.renderer.is_none());
        assert!(!gpu.grid_dirty);
        assert!(!gpu.fog_dirty);
        assert!(gpu.grid_data.is_empty());
        assert!(gpu.objects_data.is_empty());
        assert!(gpu.lowpass_data.is_empty());
        assert!(gpu.fog_data.is_empty());
        assert_eq!(gpu.grid_dims, (0, 0));
        assert_eq!(gpu.player_tile, [0.0, 0.0]);
    }

    #[test]
    fn lowpass_lut_treats_object_black_as_transparent() {
        let mut atlas = vec![0u8; TILE_COUNT * TILE_RGBA_BYTES];

        atlas[0..TILE_RGBA_BYTES]
            .chunks_exact_mut(4)
            .for_each(|px| px.copy_from_slice(&[10, 20, 30, 255]));

        let overlay_start = 256 * TILE_RGBA_BYTES;
        for (idx, px) in atlas[overlay_start..overlay_start + TILE_RGBA_BYTES]
            .chunks_exact_mut(4)
            .enumerate()
        {
            if idx < (TILE_SIZE * TILE_SIZE / 2) {
                px.copy_from_slice(&[200, 100, 50, 255]);
            }
        }

        let lut = build_tile_lowpass_lut(&atlas);
        assert_eq!(
            lut[0],
            LowpassTileSample {
                rgb: [10, 20, 30],
                coverage: 255
            }
        );
        assert_eq!(
            lut[256],
            LowpassTileSample {
                rgb: [200, 100, 50],
                coverage: 128
            }
        );
    }

    #[test]
    fn lowpass_lut_tracks_terrain_coverage_after_ignoring_black() {
        let mut atlas = vec![0u8; TILE_COUNT * TILE_RGBA_BYTES];
        let terrain = &mut atlas[0..TILE_RGBA_BYTES];
        for (idx, px) in terrain.chunks_exact_mut(4).enumerate() {
            if idx % 2 == 0 {
                px.copy_from_slice(&[0, 0, 0, 255]);
            } else {
                px.copy_from_slice(&[80, 120, 200, 255]);
            }
        }

        let lut = build_tile_lowpass_lut(&atlas);
        assert_eq!(
            lut[0],
            LowpassTileSample {
                rgb: [80, 120, 200],
                coverage: 128
            }
        );
    }

    #[test]
    fn lowpass_map_blends_object_average_over_terrain() {
        let mut lut = vec![
            LowpassTileSample {
                rgb: [0, 0, 0],
                coverage: 255,
            };
            TILE_COUNT
        ];
        lut[7] = LowpassTileSample {
            rgb: [20, 40, 60],
            coverage: 255,
        };
        lut[256 + 3] = LowpassTileSample {
            rgb: [220, 120, 20],
            coverage: 128,
        };

        let lowpass = build_lowpass_map(&[7], &[3], &lut, 1, 1);
        assert_eq!(lowpass, vec![120, 80, 40, 255]);
    }

    #[test]
    fn lowpass_map_preserves_terrain_coverage_for_brightness_compensation() {
        let mut lut = vec![
            LowpassTileSample {
                rgb: [0, 0, 0],
                coverage: 255,
            };
            TILE_COUNT
        ];
        lut[7] = LowpassTileSample {
            rgb: [80, 120, 200],
            coverage: 128,
        };

        let lowpass = build_lowpass_map(&[7], &[0], &lut, 1, 1);
        assert_eq!(lowpass, vec![40, 60, 100, 128]);
    }

    #[test]
    fn lowpass_map_composites_partial_overlay_over_partial_terrain() {
        let mut lut = vec![
            LowpassTileSample {
                rgb: [0, 0, 0],
                coverage: 255,
            };
            TILE_COUNT
        ];
        lut[7] = LowpassTileSample {
            rgb: [80, 120, 200],
            coverage: 128,
        };
        lut[256 + 3] = LowpassTileSample {
            rgb: [200, 80, 40],
            coverage: 64,
        };

        let lowpass = build_lowpass_map(&[7], &[3], &lut, 1, 1);
        assert_eq!(lowpass, vec![80, 65, 85, 160]);
    }

    #[test]
    fn local_object_overlay_ignores_other_floors() {
        let map = MapState {
            location: LocationType::Town(2),
            z: 0xFF,
            x: 12,
            y: 8,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let objects = vec![
            ObjectEntry {
                tile: 7,
                x: 4,
                y: 5,
                floor: 0xFF,
            },
            ObjectEntry {
                tile: 9,
                x: 6,
                y: 7,
                floor: 0,
            },
        ];

        let overlay = build_objects_overlay(&objects, 32, 32, &map);
        assert_eq!(overlay[5 * 32 + 4], 7);
        assert_eq!(overlay[7 * 32 + 6], 0);
    }

    #[test]
    fn combat_objects_use_battlefield_coordinates() {
        let map = MapState {
            location: LocationType::Combat(0x80),
            z: 0,
            x: 4,
            y: 10,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 192,
            scroll_y: 48,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let objects = vec![ObjectEntry {
            tile: 11,
            x: 6,
            y: 9,
            floor: 0,
        }];

        let overlay = build_objects_overlay(&objects, 11, 11, &map);
        assert_eq!(overlay[9 * 11 + 6], 11);
    }

    #[test]
    fn outdoor_objects_use_rectangular_grid_center() {
        let map = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 38,
            y: 222,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let objects = vec![ObjectEntry {
            tile: 17,
            x: 38,
            y: 222,
            floor: 0,
        }];

        let overlay = build_objects_overlay(&objects, 96, 48, &map);
        assert_eq!(overlay[24 * 96 + 48], 17);
    }

    #[test]
    fn dungeon_cell_classification_maps_structural_features() {
        assert_eq!(
            classify_dungeon_cell(0x10),
            DungeonCellKind::Ladder {
                up: true,
                down: false,
            }
        );
        assert_eq!(
            classify_dungeon_cell(0x20),
            DungeonCellKind::Ladder {
                up: false,
                down: true,
            }
        );
        assert_eq!(
            classify_dungeon_cell(0x30),
            DungeonCellKind::Ladder {
                up: true,
                down: true,
            }
        );
        assert_eq!(classify_dungeon_cell(0xB0), DungeonCellKind::Wall);
        assert_eq!(classify_dungeon_cell(0xC0), DungeonCellKind::WallAlt);
        assert_eq!(classify_dungeon_cell(0xD0), DungeonCellKind::SecretDoor);
        assert_eq!(classify_dungeon_cell(0xE0), DungeonCellKind::Door);
        assert_eq!(classify_dungeon_cell(0xA0), DungeonCellKind::Room);
        assert_eq!(classify_dungeon_cell(0xF0), DungeonCellKind::Room);
    }

    #[test]
    fn dungeon_cell_classification_collapses_nonstructural_codes_to_corridor() {
        assert_eq!(classify_dungeon_cell(0x00), DungeonCellKind::Corridor);
        assert_eq!(classify_dungeon_cell(0x40), DungeonCellKind::Corridor);
        assert_eq!(classify_dungeon_cell(0x50), DungeonCellKind::Corridor);
        assert_eq!(classify_dungeon_cell(0x60), DungeonCellKind::Corridor);
        assert_eq!(classify_dungeon_cell(0x70), DungeonCellKind::Corridor);
        assert_eq!(classify_dungeon_cell(0x80), DungeonCellKind::Corridor);
    }

    #[test]
    fn dungeon_primary_view_swaps_between_centered_and_fixed() {
        assert_eq!(
            DungeonPrimaryView::Centered.swapped(),
            DungeonPrimaryView::Fixed
        );
        assert_eq!(
            DungeonPrimaryView::Fixed.swapped(),
            DungeonPrimaryView::Centered
        );
    }

    #[test]
    fn parse_dungeon_primary_view_accepts_known_values() {
        assert_eq!(
            parse_dungeon_primary_view("centered"),
            Some(DungeonPrimaryView::Centered)
        );
        assert_eq!(
            parse_dungeon_primary_view(" fixed "),
            Some(DungeonPrimaryView::Fixed)
        );
        assert_eq!(parse_dungeon_primary_view("weird"), None);
    }

    #[test]
    fn parse_bool_pref_accepts_true_and_false() {
        assert_eq!(parse_bool_pref("true"), Some(true));
        assert_eq!(parse_bool_pref(" false "), Some(false));
        assert_eq!(parse_bool_pref("maybe"), None);
    }

    #[test]
    fn fog_reset_confirmation_requires_exact_reset_token() {
        let mut state = MinimapState::new();
        state.set_game_directory(Some(Path::new(r"C:\Games\Ultima 5")));
        state.begin_fog_reset_confirmation();
        assert!(state.fog_reset_confirmation_open());
        assert!(!state.fog_reset_confirmation_ready());
        assert_eq!(
            state.fog_reset_target_game_key.as_deref(),
            state.fog.current_game_key()
        );

        *state.fog_reset_confirmation_text() = "reset".to_string();
        assert!(!state.fog_reset_confirmation_ready());

        *state.fog_reset_confirmation_text() = " RESET ".to_string();
        assert!(!state.fog_reset_confirmation_ready());

        *state.fog_reset_confirmation_text() = "RESET".to_string();
        assert!(state.fog_reset_confirmation_ready());

        state.cancel_fog_reset_confirmation();
        assert!(!state.fog_reset_confirmation_open());
        assert!(state.fog_reset_confirmation_text().is_empty());
        assert!(state.fog_reset_target_game_key.is_none());
    }

    #[test]
    fn changing_game_directory_cancels_fog_reset_confirmation() {
        let mut state = MinimapState::new();
        state.set_game_directory(Some(Path::new(r"C:\Games\Ultima 5")));
        state.begin_fog_reset_confirmation();
        *state.fog_reset_confirmation_text() = "RESET".to_string();

        state.set_game_directory(Some(Path::new(r"C:\Games\Ultima 5 Test")));

        assert!(!state.fog_reset_confirmation_open());
        assert!(state.fog_reset_confirmation_text().is_empty());
        assert!(state.fog_reset_target_game_key.is_none());
    }

    #[test]
    fn fog_supported_for_map_excludes_combat() {
        let state = MinimapState::new();
        let overworld = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 0,
            y: 0,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let combat = MapState {
            location: LocationType::Combat(0x80),
            ..overworld.clone()
        };

        assert!(state.fog_supported_for_map(&overworld));
        assert!(!state.fog_supported_for_map(&combat));
    }

    #[test]
    fn fog_projection_wraps_outdoor_scene_coordinates_from_world_view() {
        let map = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 2,
            y: 250,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let projection =
            FogProjection::new(&map, FogScene::Britannia, GridSource::Outdoor, (16, 16));

        assert_eq!(projection.scene_coord_for_grid_cell(0, 0), Some((250, 242)));
        assert_eq!(projection.grid_cell_for_scene_coord(2, 250), Some((8, 8)));
        assert_eq!(projection.grid_cell_for_scene_coord(1, 249), Some((7, 7)));
    }

    #[test]
    fn fog_projection_uses_rectangular_outdoor_viewport_dimensions() {
        let map = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 2,
            y: 250,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };
        let projection =
            FogProjection::new(&map, FogScene::Britannia, GridSource::Outdoor, (32, 16));

        assert_eq!(projection.scene_coord_for_grid_cell(0, 0), Some((242, 242)));
        assert_eq!(projection.grid_cell_for_scene_coord(2, 250), Some((16, 8)));
    }

    #[test]
    fn fog_projection_clamps_local_visibility_to_scene_bounds() {
        let mut visibility = [FOG_HIDDEN_TILE; VIEWPORT_VISIBILITY_LEN];
        let center = (VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
            + VIEWPORT_VISIBILITY_WIDTH / 2;
        visibility[center] = 0x01;
        visibility[0] = 0x01;

        let map = MapState {
            location: LocationType::Town(2),
            z: 0,
            x: 0,
            y: 0,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: Some(visibility),
            objects: Vec::new(),
        };
        let projection = FogProjection::new(&map, FogScene::Local(2), GridSource::Local, (32, 32));

        assert_eq!(
            projection.visible_scene_coords(map.visibility_tiles.as_ref()),
            vec![(0, 0)]
        );
    }

    #[test]
    fn build_fog_texture_reuses_last_stable_visibility_without_persisting_it() {
        let mut state = MinimapState::new();
        let mut visibility = [FOG_HIDDEN_TILE; VIEWPORT_VISIBILITY_LEN];
        let center = (VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
            + VIEWPORT_VISIBILITY_WIDTH / 2;
        visibility[center] = 0x01;

        let stable_map = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 10,
            y: 12,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            visibility_tiles: Some(visibility),
            objects: Vec::new(),
        };
        let fog = build_fog_texture(&mut state, &stable_map, GridSource::Outdoor, (11, 11));
        assert_eq!(fog[center], FOG_VISIBILITY_VISIBLE);

        let scene = FogScene::Britannia;
        let scene_data = state.fog.scene_data(scene).to_vec();
        assert_eq!(scene_data[12 * 256 + 10], 1);

        let transitioning_map = MapState {
            x: 11,
            visibility_tiles: None,
            ..stable_map
        };
        let fog = build_fog_texture(
            &mut state,
            &transitioning_map,
            GridSource::Outdoor,
            (11, 11),
        );
        assert_eq!(fog[center], FOG_VISIBILITY_VISIBLE);

        let scene_data = state.fog.scene_data(scene).to_vec();
        assert_eq!(
            scene_data[12 * 256 + 11],
            0,
            "display-only fallback visibility must not permanently reveal tiles"
        );
    }

    #[test]
    fn centered_dungeon_tile_rect_keeps_player_tile_at_view_center() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(160.0, 160.0));
        let cell = centered_dungeon_tile_rect(rect, [3.0, 5.0], [3.0, 5.0]);

        assert_eq!(cell.center(), rect.center());
        assert_eq!(cell.size(), vec2(20.0, 20.0));
    }

    #[test]
    fn centered_dungeon_tile_rect_places_wrapped_repeat_adjacent_to_player() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(160.0, 160.0));
        let wrapped_copy = centered_dungeon_tile_rect(rect, [7.0, 2.0], [8.0, 2.0]);

        assert_eq!(
            wrapped_copy.center(),
            Pos2::new(rect.center().x + 20.0, rect.center().y)
        );
    }

    #[test]
    fn fixed_dungeon_tile_rect_uses_canonical_8x8_layout() {
        let rect = Rect::from_min_size(Pos2::new(16.0, 24.0), vec2(160.0, 160.0));
        let cell = fixed_dungeon_tile_rect(rect, 3, 5);

        assert_eq!(cell.min, Pos2::new(76.0, 124.0));
        assert_eq!(cell.max, Pos2::new(96.0, 144.0));
    }

    #[test]
    fn dungeon_overview_rect_stays_in_top_right_of_map() {
        let map_rect = Rect::from_min_size(Pos2::new(40.0, 60.0), vec2(400.0, 400.0));
        let inset = dungeon_overview_rect(map_rect);

        assert!(inset.right() <= map_rect.right());
        assert!(inset.top() >= map_rect.top());
        assert!(inset.center().x > map_rect.center().x);
        assert!(inset.center().y < map_rect.center().y);
    }

    #[test]
    fn dungeon_overview_rect_handles_narrow_map_without_panicking() {
        let map_rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(60.0, 160.0));
        let inset = dungeon_overview_rect(map_rect);

        assert!(inset.width() >= 8.0);
        assert!(inset.right() <= map_rect.right());
        assert!(inset.top() >= map_rect.top());
    }

    #[test]
    fn chunked_overworld_grid_linearizes_to_row_major() {
        let mut tiles = [0u8; 1024];
        for gy in 0..32usize {
            for gx in 0..32usize {
                let cx = gx / 16;
                let cy = gy / 16;
                let lx = gx % 16;
                let ly = gy % 16;
                tiles[(cy * 2 + cx) * 256 + ly * 16 + lx] = (gy * 32 + gx) as u8;
            }
        }

        let grid = linearize_chunked_grid(&tiles);
        for (idx, tile) in grid.into_iter().enumerate() {
            assert_eq!(tile, idx as u8);
        }
    }

    #[test]
    fn row_major_grid_is_read_verbatim() {
        let mut tiles = [0u8; 1024];
        for (idx, tile) in tiles.iter_mut().enumerate() {
            *tile = (idx % 251) as u8;
        }

        assert_eq!(extract_row_major_grid(&tiles), tiles.to_vec());
    }

    #[test]
    fn dungeon_grid_uses_first_floor_slice_only() {
        let mut tiles = [0u8; 1024];
        for (idx, tile) in tiles.iter_mut().take(DUNGEON_LEVEL_LEN).enumerate() {
            *tile = idx as u8;
        }
        tiles[DUNGEON_LEVEL_LEN] = 0xEE;

        let grid = extract_dungeon_grid(&tiles);
        assert_eq!(grid.len(), DUNGEON_LEVEL_LEN);
        assert_eq!(grid[0], 0);
        assert_eq!(grid[DUNGEON_LEVEL_LEN - 1], 63);
    }

    #[test]
    fn local_scene_grid_uses_combat_dimensions_and_marker() {
        let mut combat_tiles = [0u8; COMBAT_TERRAIN_LEN];
        combat_tiles[10 * COMBAT_TERRAIN_STRIDE + 4] = 0x44;
        let map = MapState {
            location: LocationType::Combat(0xFF),
            z: 0,
            x: 4,
            y: 10,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 192,
            scroll_y: 48,
            tiles: [0; 1024],
            combat_tiles: Some(combat_tiles),
            visibility_tiles: None,
            objects: Vec::new(),
        };

        let (grid, width, height, marker) = extract_local_scene_grid(&map);
        assert_eq!((width, height), (11, 11));
        assert_eq!(marker, [4.0, 10.0]);
        assert_eq!(grid[10 * 11 + 4], 0x44);
    }

    #[test]
    fn local_scene_grid_uses_dungeon_local_marker_coordinates() {
        let mut tiles = [0u8; 1024];
        tiles[2 * DUNGEON_LEVEL_WIDTH + 1] = 0x44;
        let map = MapState {
            location: LocationType::Dungeon(34),
            z: 0,
            x: 1,
            y: 2,
            dungeon_facing: Some(CardinalDirection::South),
            transport: 0,
            scroll_x: 80,
            scroll_y: 48,
            tiles,
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };

        let (grid, width, height, marker) = extract_local_scene_grid(&map);
        assert_eq!(
            (width, height),
            (DUNGEON_LEVEL_WIDTH as u32, DUNGEON_LEVEL_HEIGHT as u32)
        );
        assert_eq!(marker, [1.0, 2.0]);
        assert_eq!(grid[2 * DUNGEON_LEVEL_WIDTH + 1], 0x44);
    }

    #[test]
    fn combat_grid_discards_padding_columns() {
        let mut tiles = [0xEE; COMBAT_TERRAIN_LEN];
        for y in 0..COMBAT_TERRAIN_HEIGHT {
            for x in 0..COMBAT_TERRAIN_WIDTH {
                tiles[y * COMBAT_TERRAIN_STRIDE + x] = (y * COMBAT_TERRAIN_WIDTH + x) as u8;
            }
        }

        let grid = extract_combat_grid(&tiles);
        for (idx, tile) in grid.into_iter().enumerate() {
            assert_eq!(tile, idx as u8);
        }
    }

    #[test]
    fn combat_uses_dedicated_grid_encoding() {
        assert_eq!(
            LocationType::Combat(0x80).tile_grid_encoding(),
            TileGridEncoding::Combat11x11Stride32
        );
        assert_eq!(
            LocationType::Town(1).tile_grid_encoding(),
            TileGridEncoding::RowMajor32
        );
        assert_eq!(
            LocationType::Dungeon(34).tile_grid_encoding(),
            TileGridEncoding::Dungeon8x8
        );
    }
}
