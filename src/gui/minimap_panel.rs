use std::sync::{Arc, Mutex};

use egui::epaint::PaintCallbackInfo;
use egui::{Color32, FontId, Pos2, Rect, Stroke, vec2};

use crate::game::map::{LocationType, MapState, ObjectEntry};
use crate::game::world_map::{WorldLabelCategory, WorldLocation, WorldMap};
use crate::tiles::atlas::TileAtlas;

use super::minimap_gl::MinimapGl;

const ZOOM_MIN: usize = 11;
const ZOOM_MAX: usize = 256;
const ZOOM_DEFAULT: usize = 48;

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
enum GridSource {
    Local,
    Overworld,
}

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
    show_labels: bool,
    label_filters: LabelFilters,
    last_center: Option<(u8, u8)>,
    last_zoom: Option<usize>,
    last_grid_source: Option<GridSource>,
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
            show_labels: true,
            label_filters: LabelFilters::default(),
            last_center: None,
            last_zoom: None,
            last_grid_source: None,
        }
    }
}

impl MinimapState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear cached map data so the next loaded snapshot forces a fresh upload.
    pub fn clear(&mut self) {
        self.map = None;
        self.raw_atlas = None;
        self.last_center = None;
        self.last_zoom = None;
        self.last_grid_source = None;

        let mut gpu = self.gpu.lock().unwrap();
        gpu.renderer = None;
        gpu.grid_dirty = false;
        gpu.grid_data.clear();
        gpu.objects_data.clear();
        gpu.grid_dims = (0, 0);
        gpu.player_tile = [0.0, 0.0];
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
    let is_overworld = map.location == LocationType::Overworld;

    // Zoom controls + header row
    ui.horizontal_wrapped(|ui| {
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
        if is_overworld && world_map.is_some() {
            ui.separator();
            ui.checkbox(&mut state.show_labels, "Labels");
            ui.add_enabled_ui(state.show_labels, |ui| {
                ui.checkbox(&mut state.label_filters.towns, "Towns");
                ui.checkbox(&mut state.label_filters.dwellings, "Dwellings");
                ui.checkbox(&mut state.label_filters.castles, "Castles");
                ui.checkbox(&mut state.label_filters.keeps, "Keeps");
                ui.checkbox(&mut state.label_filters.dungeons, "Dungeons");
                ui.checkbox(&mut state.label_filters.shrines, "Shrines");
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

    // Prepare tile grid data on CPU when map state changes.
    // Object positions change every game turn, so always update when objects are present.
    let has_objects = !map.objects.is_empty();
    let needs_update = state.last_center != Some((cx, cy))
        || state.last_zoom != Some(zoom)
        || state.last_grid_source != Some(grid_source)
        || has_objects;

    if needs_update {
        let (grid_data, grid_w, grid_h, player_tile) =
            if let Some(wm) = world_map.filter(|_| is_overworld) {
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
        state.last_grid_source = Some(grid_source);
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

#[derive(Clone, Copy)]
struct VisibleWorldLocation<'a> {
    location: &'a WorldLocation,
    point: Pos2,
    distance_sq: i32,
}

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

fn wrapped_delta(coord: u8, center: u8) -> i16 {
    let mut delta = coord as i16 - center as i16;
    if delta > 127 {
        delta -= 256;
    } else if delta < -128 {
        delta += 256;
    }
    delta
}

fn world_label_priority(category: WorldLabelCategory) -> u8 {
    match category {
        WorldLabelCategory::Shrine => 0,
        WorldLabelCategory::Town => 1,
        WorldLabelCategory::Castle | WorldLabelCategory::Keep => 2,
        WorldLabelCategory::Dungeon => 3,
        WorldLabelCategory::Dwelling => 4,
    }
}

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
    fn grid_source_distinguishes_local_and_overworld_textures() {
        assert_ne!(GridSource::Local, GridSource::Overworld);
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
            objects: Vec::new(),
        });
        state.last_center = Some((1, 2));
        state.last_zoom = Some(32);
        state.last_grid_source = Some(GridSource::Overworld);
        state.raw_atlas = Some(Arc::new(vec![1, 2, 3]));

        {
            let mut gpu = state.gpu.lock().unwrap();
            gpu.grid_dirty = true;
            gpu.grid_data = vec![1];
            gpu.objects_data = vec![2];
            gpu.grid_dims = (3, 4);
            gpu.player_tile = [5.0, 6.0];
        }

        state.clear();

        assert!(state.map.is_none());
        assert!(state.raw_atlas.is_none());
        assert!(state.last_center.is_none());
        assert!(state.last_zoom.is_none());
        assert!(state.last_grid_source.is_none());

        let gpu = state.gpu.lock().unwrap();
        // clear() must drop the GL-backed renderer so the next paint callback rebuilds it.
        assert!(gpu.renderer.is_none());
        assert!(!gpu.grid_dirty);
        assert!(gpu.grid_data.is_empty());
        assert!(gpu.objects_data.is_empty());
        assert_eq!(gpu.grid_dims, (0, 0));
        assert_eq!(gpu.player_tile, [0.0, 0.0]);
    }
}
