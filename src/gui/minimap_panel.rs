use std::sync::{Arc, Mutex};

use egui::epaint::PaintCallbackInfo;
use egui::{Color32, FontId, Pos2, Rect, Stroke, vec2};

use crate::game::map::{LocationType, MapState, ObjectEntry, TileGridEncoding};
use crate::game::offsets::{
    COMBAT_TERRAIN_HEIGHT, COMBAT_TERRAIN_LEN, COMBAT_TERRAIN_STRIDE, COMBAT_TERRAIN_WIDTH,
};
use crate::game::world_map::{WorldLabelCategory, WorldLocation, WorldMap};
use crate::tiles::atlas::{TILE_COUNT, TILE_SIZE, TileAtlas};
use crate::tiles::ega::is_ega_black_rgba;

use super::minimap_gl::MinimapGl;

const ZOOM_MIN: usize = 11;
const ZOOM_MAX: usize = 256;
const ZOOM_DEFAULT: usize = 48;
const TILE_RGBA_BYTES: usize = TILE_SIZE * TILE_SIZE * 4;
const PLAYER_MARKER_MIN_RADIUS: f32 = 4.0;
const PLAYER_MARKER_TILE_MARGIN_PX: f32 = 3.0;
const PLAYER_MARKER_FILL_TILE_THRESHOLD_PX: f32 = 6.0;
const PLAYER_MARKER_COLOR: Color32 = Color32::from_rgb(255, 230, 128);

#[derive(Debug, Clone, Copy, PartialEq)]
struct PlayerMarkerStyle {
    radius: f32,
    filled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LowpassTileSample {
    rgb: [u8; 3],
    alpha: u8,
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
    zoom: usize,
    show_labels: bool,
    label_filters: LabelFilters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Distinguishes overworld rendering from 32x32 local map rendering.
enum GridSource {
    Local,
    Overworld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Cache identity for the current minimap grid contents.
///
/// Local maps need their own discriminator so switching towns or dungeon floors
/// at the same coordinates still invalidates the cached terrain texture.
struct GridCacheKey {
    center: (u8, u8),
    zoom: usize,
    source: GridSource,
    local_map: Option<(LocationType, u8)>,
}

/// Shared state accessed by both the UI thread (for updates) and the paint
/// callback (for rendering). Protected by a mutex.
struct GpuState {
    renderer: Option<MinimapGl>,
    grid_dirty: bool,
    grid_data: Vec<u8>,
    objects_data: Vec<u8>,
    lowpass_data: Vec<u8>,
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
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            map: None,
            gpu: Arc::new(Mutex::new(GpuState {
                renderer: None,
                grid_dirty: false,
                grid_data: Vec::new(),
                objects_data: Vec::new(),
                lowpass_data: Vec::new(),
                grid_dims: (0, 0),
                player_tile: [0.0, 0.0],
            })),
            raw_atlas: None,
            tile_lowpass_lut: None,
            zoom: ZOOM_DEFAULT,
            show_labels: true,
            label_filters: LabelFilters::default(),
            last_grid_key: None,
        }
    }
}

impl MinimapState {
    /// Construct an empty minimap state with default zoom and label filters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear cached map data so the next loaded snapshot forces a fresh upload.
    pub fn clear(&mut self) {
        self.map = None;
        self.raw_atlas = None;
        self.tile_lowpass_lut = None;
        self.last_grid_key = None;

        let mut gpu = self.gpu.lock().unwrap();
        gpu.renderer = None;
        gpu.grid_dirty = false;
        gpu.grid_data.clear();
        gpu.objects_data.clear();
        gpu.lowpass_data.clear();
        gpu.grid_dims = (0, 0);
        gpu.player_tile = [0.0, 0.0];
    }
}

/// Render the minimap controls and GL-backed map view for the current snapshot.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut MinimapState,
    atlas: &TileAtlas,
    world_map: Option<&WorldMap>,
) {
    let Some(ref map) = state.map else {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for map data...");
        });
        return;
    };

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

    // Header
    let header = if map.z == 0xFF {
        format!("{} ({}, {})", map.location.name(), map.x, map.y)
    } else {
        format!("{} ({}, {}) Z:{}", map.location.name(), map.x, map.y, map.z)
    };
    let is_overworld = map.location.is_overworld();

    ui.vertical(|ui| {
        // Keep the changing coordinate text on its own line so movement cannot
        // reflow the zoom controls and resize the map panel.
        ui.add(egui::Label::new(&header).truncate());

        ui.horizontal(|ui| {
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
            ui.label(format!("{}x{}", state.zoom, state.zoom));
        });

        if is_overworld && world_map.is_some() {
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
    });

    let zoom = state.zoom;
    let cx = map.x;
    let cy = map.y;
    let grid_source = if is_overworld && world_map.is_some() {
        GridSource::Overworld
    } else {
        GridSource::Local
    };
    let grid_key = GridCacheKey {
        center: (cx, cy),
        zoom,
        source: grid_source,
        local_map: (!is_overworld).then_some((map.location, map.z)),
    };

    // Prepare tile grid data on CPU when map state changes.
    // Object positions change every game turn, so always update when objects are present.
    let has_objects = !map.objects.is_empty();
    let needs_update = needs_grid_refresh(state.last_grid_key, grid_key, has_objects);

    if needs_update {
        let (grid_data, grid_w, grid_h, player_tile) =
            if let Some(wm) = world_map.filter(|_| is_overworld) {
                let grid = extract_overworld_grid(wm, cx, cy, zoom);
                let half = zoom as f32 / 2.0;
                (grid, zoom as u32, zoom as u32, [half, half])
            } else {
                extract_local_scene_grid(map)
            };

        let objects_data =
            build_objects_overlay(&map.objects, grid_w as usize, grid_h as usize, map);
        let lowpass_data = build_lowpass_map(
            &grid_data,
            &objects_data,
            state.tile_lowpass_lut.as_ref().unwrap(),
            grid_w as usize,
            grid_h as usize,
        );

        let mut gpu = state.gpu.lock().unwrap();
        gpu.grid_data = grid_data;
        gpu.objects_data = objects_data;
        gpu.lowpass_data = lowpass_data;
        gpu.grid_dims = (grid_w, grid_h);
        gpu.player_tile = player_tile;
        gpu.grid_dirty = true;

        state.last_grid_key = Some(grid_key);
    }

    // Allocate a centered square region for the minimap
    let avail = ui.available_size();
    let side = avail.x.min(avail.y);

    // Issue paint callback inside centered layout
    let gpu = state.gpu.clone();
    let raw_atlas = state.raw_atlas.clone().unwrap();

    ui.vertical_centered(|ui| {
        let (rect, _response) =
            ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());

        let callback = egui_glow::CallbackFn::new(move |info: PaintCallbackInfo, painter| {
            let gl = painter.gl();
            let mut gpu = gpu.lock().unwrap();

            // Lazy-init: create renderer and upload atlas on first callback
            if gpu.renderer.is_none() {
                gpu.renderer = Some(MinimapGl::new(gl, &raw_atlas));
                // Force grid upload on first frame
                gpu.grid_dirty = true;
            }

            // Upload grid and objects textures if dirty
            if gpu.grid_dirty {
                let renderer = gpu.renderer.as_ref().unwrap();
                renderer.update_grid(gl, &gpu.grid_data, gpu.grid_dims.0, gpu.grid_dims.1);
                renderer.update_objects(gl, &gpu.objects_data, gpu.grid_dims.0, gpu.grid_dims.1);
                renderer.update_lowpass(gl, &gpu.lowpass_data, gpu.grid_dims.0, gpu.grid_dims.1);
                gpu.grid_dirty = false;
            }

            let grid_size = [gpu.grid_dims.0 as f32, gpu.grid_dims.1 as f32];
            gpu.renderer.as_ref().unwrap().paint(gl, &info, grid_size);
        });

        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });

        if let Some(wm) = world_map.filter(|_| is_overworld) {
            let overlay = OverworldOverlayOptions {
                cx,
                cy,
                zoom,
                show_labels: state.show_labels,
                label_filters: state.label_filters,
            };
            paint_overworld_overlay(ui, rect, wm, overlay);
        }

        let (grid_dims, player_tile) = {
            let gpu = state.gpu.lock().unwrap();
            (gpu.grid_dims, gpu.player_tile)
        };
        paint_player_marker(ui, rect, grid_dims, player_tile);
    });
}

/// Render a placeholder when the tile atlas has not been loaded yet.
pub fn show_no_atlas(ui: &mut egui::Ui, status: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(status);
    });
}

/// Extract a zoom x zoom window from the overworld centered on (cx, cy), wrapping at 256.
fn extract_overworld_grid(world: &WorldMap, cx: u8, cy: u8, zoom: usize) -> Vec<u8> {
    let half = zoom as i32 / 2;
    let mut grid = vec![0u8; zoom * zoom];
    for vy in 0..zoom {
        for vx in 0..zoom {
            let wx = (cx as i32 - half + vx as i32).rem_euclid(256) as u8;
            let wy = (cy as i32 - half + vy as i32).rem_euclid(256) as u8;
            grid[vy * zoom + vx] = world.get_tile(wx, wy);
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

/// Decode the current non-overworld scene into a renderable grid plus player position.
fn extract_local_scene_grid(map: &MapState) -> (Vec<u8>, u32, u32, [f32; 2]) {
    match map.location.tile_grid_encoding() {
        TileGridEncoding::Chunked16x16 => {
            let pbx = map.x.wrapping_sub(map.scroll_x) as f32;
            let pby = map.y.wrapping_sub(map.scroll_y) as f32;
            (linearize_chunked_grid(&map.tiles), 32, 32, [pbx, pby])
        }
        TileGridEncoding::RowMajor32 => {
            let pbx = map.x.wrapping_sub(map.scroll_x) as f32;
            let pby = map.y.wrapping_sub(map.scroll_y) as f32;
            (extract_row_major_grid(&map.tiles), 32, 32, [pbx, pby])
        }
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
    let mut overlay = vec![0u8; grid_w * grid_h];
    let half = grid_w as i32 / 2;
    let overworld = map.location.is_overworld();
    let combat = matches!(map.location, LocationType::Combat(_));

    for obj in objects {
        if !overworld && obj.floor != map.z {
            continue;
        }

        let (gx, gy) = if overworld {
            let vx = (obj.x as i32 - map.x as i32 + half).rem_euclid(256);
            let vy = (obj.y as i32 - map.y as i32 + half).rem_euclid(256);
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
/// darken the minimap during minification. Animated-page tiles (256-511) also
/// use the surviving pixel coverage as overlay alpha.
fn build_tile_lowpass_lut(atlas_rgba: &[u8]) -> Vec<LowpassTileSample> {
    assert_eq!(
        atlas_rgba.len(),
        TILE_COUNT * TILE_RGBA_BYTES,
        "tile low-pass LUT requires one 16x16 RGBA tile for each atlas entry"
    );

    let mut lut = Vec::with_capacity(TILE_COUNT);
    for tile_idx in 0..TILE_COUNT {
        let tile = &atlas_rgba[tile_idx * TILE_RGBA_BYTES..(tile_idx + 1) * TILE_RGBA_BYTES];
        let overlay_tile = tile_idx >= 256;
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

        let alpha = if overlay_tile {
            ((opaque_pixels * 255 + (TILE_SIZE * TILE_SIZE / 2) as u32)
                / (TILE_SIZE * TILE_SIZE) as u32) as u8
        } else {
            u8::MAX
        };

        let rgb = if opaque_pixels == 0 {
            [0, 0, 0]
        } else {
            [
                (sum[0] / opaque_pixels) as u8,
                (sum[1] / opaque_pixels) as u8,
                (sum[2] / opaque_pixels) as u8,
            ]
        };

        lut.push(LowpassTileSample { rgb, alpha });
    }

    lut
}

/// Build an RGBA low-pass map that stays stable when multiple tiles collapse
/// into a single screen pixel.
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
        let terrain = tile_lut[grid_data[idx] as usize];
        let mut rgb = [
            terrain.rgb[0] as u32,
            terrain.rgb[1] as u32,
            terrain.rgb[2] as u32,
        ];

        let object_id = objects_data[idx];
        if object_id != 0 {
            let overlay = tile_lut[object_id as usize + 256];
            let alpha = overlay.alpha as u32;
            rgb[0] = (rgb[0] * (255 - alpha) + overlay.rgb[0] as u32 * alpha + 127) / 255;
            rgb[1] = (rgb[1] * (255 - alpha) + overlay.rgb[1] as u32 * alpha + 127) / 255;
            rgb[2] = (rgb[2] * (255 - alpha) + overlay.rgb[2] as u32 * alpha + 127) / 255;
        }

        let out = &mut lowpass[idx * 4..idx * 4 + 4];
        out[0] = rgb[0] as u8;
        out[1] = rgb[1] as u8;
        out[2] = rgb[2] as u8;
        out[3] = u8::MAX;
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
) {
    let mut visible = visible_world_locations(
        world_map.locations(),
        rect,
        overlay.cx,
        overlay.cy,
        overlay.zoom,
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
        let fill = world_marker_color(entry.location.category());
        painter.circle_filled(entry.point, 3.5, fill);
        painter.circle_stroke(entry.point, 4.5, Stroke::new(1.0, Color32::BLACK));
    }

    if !overlay.show_labels {
        return;
    }

    let font_size = match overlay.zoom {
        0..=48 => 12.0,
        49..=96 => 11.0,
        _ => 10.0,
    };
    let font_id = FontId::proportional(font_size);
    let mut occupied = Vec::new();

    for entry in &visible {
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

/// Paint the current player location with a screen-space marker that keeps a
/// readable minimum size while still expanding to nearly fill the tile at
/// close zoom.
fn paint_player_marker(ui: &egui::Ui, rect: Rect, grid_dims: (u32, u32), player_tile: [f32; 2]) {
    if grid_dims.0 == 0 || grid_dims.1 == 0 {
        return;
    }

    let center = player_marker_center(rect, grid_dims, player_tile);
    let tile_span_px = (rect.width() / grid_dims.0 as f32).min(rect.height() / grid_dims.1 as f32);
    let style = player_marker_style(tile_span_px);
    let painter = ui.painter_at(rect);

    if style.filled {
        painter.circle_filled(center, style.radius, PLAYER_MARKER_COLOR);
        painter.circle_stroke(center, style.radius + 0.5, Stroke::new(1.5, Color32::BLACK));
    } else {
        painter.circle_stroke(center, style.radius + 0.5, Stroke::new(3.0, Color32::BLACK));
        painter.circle_stroke(center, style.radius, Stroke::new(1.75, PLAYER_MARKER_COLOR));
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
    zoom: usize,
) -> Vec<VisibleWorldLocation<'a>> {
    let half = zoom as i32 / 2;
    let mut visible = Vec::new();

    for location in locations {
        let dx = wrapped_delta(location.x, cx) as i32;
        let dy = wrapped_delta(location.y, cy) as i32;
        let tile_x = dx + half;
        let tile_y = dy + half;
        if tile_x < 0 || tile_y < 0 || tile_x >= zoom as i32 || tile_y >= zoom as i32 {
            continue;
        }

        let point = Pos2::new(
            rect.left() + (tile_x as f32 + 0.5) / zoom as f32 * rect.width(),
            rect.top() + (tile_y as f32 + 0.5) / zoom as f32 * rect.height(),
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

        let visible = visible_world_locations(&locations, rect, 250, 250, 16);
        assert_eq!(visible.len(), 1);
        assert!(visible[0].point.x > rect.center().x);
        assert!((visible[0].point.y - rect.center().y).abs() <= rect.height() / 16.0);
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
                source: GridSource::Local,
                local_map: Some((LocationType::Town(1), 0)),
            }),
            GridCacheKey {
                center: (42, 43),
                zoom: 32,
                source: GridSource::Overworld,
                local_map: None,
            },
            false,
        ));
        assert!(!needs_grid_refresh(
            Some(GridCacheKey {
                center: (42, 43),
                zoom: 32,
                source: GridSource::Overworld,
                local_map: None,
            }),
            GridCacheKey {
                center: (42, 43),
                zoom: 32,
                source: GridSource::Overworld,
                local_map: None,
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
                source: GridSource::Local,
                local_map: Some((LocationType::Town(1), 0)),
            }),
            GridCacheKey {
                center: (12, 9),
                zoom: 32,
                source: GridSource::Local,
                local_map: Some((LocationType::Town(2), 0)),
            },
            false,
        ));
        assert!(needs_grid_refresh(
            Some(GridCacheKey {
                center: (12, 9),
                zoom: 32,
                source: GridSource::Local,
                local_map: Some((LocationType::Dungeon(33), 0)),
            }),
            GridCacheKey {
                center: (12, 9),
                zoom: 32,
                source: GridSource::Local,
                local_map: Some((LocationType::Dungeon(33), 1)),
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
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
            objects: Vec::new(),
        });
        state.last_grid_key = Some(GridCacheKey {
            center: (1, 2),
            zoom: 32,
            source: GridSource::Overworld,
            local_map: None,
        });
        state.raw_atlas = Some(Arc::new(vec![1, 2, 3]));

        {
            let mut gpu = state.gpu.lock().unwrap();
            gpu.grid_dirty = true;
            gpu.grid_data = vec![1];
            gpu.objects_data = vec![2];
            gpu.lowpass_data = vec![3, 4, 5, 255];
            gpu.grid_dims = (3, 4);
            gpu.player_tile = [5.0, 6.0];
        }

        state.clear();

        assert!(state.map.is_none());
        assert!(state.raw_atlas.is_none());
        assert!(state.last_grid_key.is_none());

        let gpu = state.gpu.lock().unwrap();
        // clear() must drop the GL-backed renderer so the next paint callback rebuilds it.
        assert!(gpu.renderer.is_none());
        assert!(!gpu.grid_dirty);
        assert!(gpu.grid_data.is_empty());
        assert!(gpu.objects_data.is_empty());
        assert!(gpu.lowpass_data.is_empty());
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
                alpha: 255
            }
        );
        assert_eq!(
            lut[256],
            LowpassTileSample {
                rgb: [200, 100, 50],
                alpha: 128
            }
        );
    }

    #[test]
    fn lowpass_lut_ignores_black_for_terrain_tiles() {
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
                alpha: 255
            }
        );
    }

    #[test]
    fn lowpass_map_blends_object_average_over_terrain() {
        let mut lut = vec![
            LowpassTileSample {
                rgb: [0, 0, 0],
                alpha: 255,
            };
            TILE_COUNT
        ];
        lut[7] = LowpassTileSample {
            rgb: [20, 40, 60],
            alpha: 255,
        };
        lut[256 + 3] = LowpassTileSample {
            rgb: [220, 120, 20],
            alpha: 128,
        };

        let lowpass = build_lowpass_map(&[7], &[3], &lut, 1, 1);
        assert_eq!(lowpass, vec![120, 80, 40, 255]);
    }

    #[test]
    fn local_object_overlay_ignores_other_floors() {
        let map = MapState {
            location: LocationType::Town(2),
            z: 0xFF,
            x: 12,
            y: 8,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; 1024],
            combat_tiles: None,
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
            transport: 0,
            scroll_x: 192,
            scroll_y: 48,
            tiles: [0; 1024],
            combat_tiles: None,
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
    fn local_scene_grid_uses_combat_dimensions_and_marker() {
        let mut combat_tiles = [0u8; COMBAT_TERRAIN_LEN];
        combat_tiles[10 * COMBAT_TERRAIN_STRIDE + 4] = 0x44;
        let map = MapState {
            location: LocationType::Combat(0xFF),
            z: 0,
            x: 4,
            y: 10,
            transport: 0,
            scroll_x: 192,
            scroll_y: 48,
            tiles: [0; 1024],
            combat_tiles: Some(combat_tiles),
            objects: Vec::new(),
        };

        let (grid, width, height, marker) = extract_local_scene_grid(&map);
        assert_eq!((width, height), (11, 11));
        assert_eq!(marker, [4.0, 10.0]);
        assert_eq!(grid[10 * 11 + 4], 0x44);
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
    }
}
