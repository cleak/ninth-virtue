use std::sync::{Arc, Mutex};

use egui::epaint::PaintCallbackInfo;

use crate::game::map::{LocationType, MapState, ObjectEntry};
use crate::game::world_map::WorldMap;
use crate::tiles::atlas::TileAtlas;

use super::minimap_gl::MinimapGl;

const ZOOM_MIN: usize = 11;
const ZOOM_MAX: usize = 256;
const ZOOM_DEFAULT: usize = 48;

/// Shared state accessed by both the UI thread (for updates) and the paint
/// callback (for rendering). Protected by a mutex.
struct GpuState {
    renderer: Option<MinimapGl>,
    grid_dirty: bool,
    grid_data: Vec<u8>,
    objects_data: Vec<u8>,
    grid_dims: (u32, u32),
    player_tile: [f32; 2],
}

pub struct MinimapState {
    pub map: Option<MapState>,
    gpu: Arc<Mutex<GpuState>>,
    /// Raw sequential atlas RGBA, captured once from TileAtlas for lazy GPU upload.
    raw_atlas: Option<Arc<Vec<u8>>>,
    zoom: usize,
    last_center: Option<(u8, u8)>,
    last_zoom: Option<usize>,
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
                grid_dims: (0, 0),
                player_tile: [0.0, 0.0],
            })),
            raw_atlas: None,
            zoom: ZOOM_DEFAULT,
            last_center: None,
            last_zoom: None,
        }
    }
}

impl MinimapState {
    pub fn new() -> Self {
        Self::default()
    }
}

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

    // Header
    let header = if map.z == 0xFF {
        format!("{} ({}, {})", map.location.name(), map.x, map.y)
    } else {
        format!("{} ({}, {}) Z:{}", map.location.name(), map.x, map.y, map.z)
    };

    // Zoom controls + header row
    ui.horizontal(|ui| {
        ui.label(&header);
        ui.separator();
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

    let zoom = state.zoom;
    let cx = map.x;
    let cy = map.y;

    // Prepare tile grid data on CPU when map state changes.
    // Object positions change every game turn, so always update when objects are present.
    let has_objects = !map.objects.is_empty();
    let needs_update =
        state.last_center != Some((cx, cy)) || state.last_zoom != Some(zoom) || has_objects;

    if needs_update {
        let (grid_data, grid_w, grid_h, player_tile) =
            if let Some(wm) = world_map.filter(|_| map.location == LocationType::Overworld) {
                let grid = extract_overworld_grid(wm, cx, cy, zoom);
                let half = zoom as f32 / 2.0;
                (grid, zoom as u32, zoom as u32, [half, half])
            } else {
                let grid = linearize_town_grid(&map.tiles);
                let pbx = map.x.wrapping_sub(map.scroll_x) as f32;
                let pby = map.y.wrapping_sub(map.scroll_y) as f32;
                (grid, 32, 32, [pbx, pby])
            };

        let objects_data =
            build_objects_overlay(&map.objects, grid_w as usize, grid_h as usize, map);

        let mut gpu = state.gpu.lock().unwrap();
        gpu.grid_data = grid_data;
        gpu.objects_data = objects_data;
        gpu.grid_dims = (grid_w, grid_h);
        gpu.player_tile = player_tile;
        gpu.grid_dirty = true;

        state.last_center = Some((cx, cy));
        state.last_zoom = Some(zoom);
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
                gpu.grid_dirty = false;
            }

            let grid_size = [gpu.grid_dims.0 as f32, gpu.grid_dims.1 as f32];
            let player_tile = gpu.player_tile;
            gpu.renderer
                .as_ref()
                .unwrap()
                .paint(gl, &info, grid_size, player_tile);
        });

        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });
    });
}

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

/// Linearize the chunked 32x32 town/dungeon tile grid to row-major order.
fn linearize_town_grid(tiles: &[u8; 1024]) -> Vec<u8> {
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
    let overworld = map.location == LocationType::Overworld;

    for obj in objects {
        let (gx, gy) = if overworld {
            let vx = (obj.x as i32 - map.x as i32 + half).rem_euclid(256);
            let vy = (obj.y as i32 - map.y as i32 + half).rem_euclid(256);
            (vx, vy)
        } else {
            (
                obj.x.wrapping_sub(map.scroll_x) as i32,
                obj.y.wrapping_sub(map.scroll_y) as i32,
            )
        };

        if gx >= 0 && gx < grid_w as i32 && gy >= 0 && gy < grid_h as i32 {
            overlay[gy as usize * grid_w + gx as usize] = obj.tile;
        }
    }

    overlay
}
