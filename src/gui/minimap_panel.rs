use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

use crate::game::map::MapState;
use crate::game::world_map::WorldMap;
use crate::tiles::atlas::{TILE_SIZE, TileAtlas};

const ZOOM_MIN: usize = 11;
const ZOOM_MAX: usize = 256;
const ZOOM_DEFAULT: usize = 48;

pub struct MinimapState {
    pub map: Option<MapState>,
    texture: Option<TextureHandle>,
    zoom: usize,
    last_center: Option<(u8, u8)>,
    last_zoom: Option<usize>,
    last_linear: bool,
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            map: None,
            texture: None,
            zoom: ZOOM_DEFAULT,
            last_center: None,
            last_zoom: None,
            last_linear: false,
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
            // zoom out: show more tiles
            state.zoom = (state.zoom * 4 / 3).min(ZOOM_MAX);
        }
        let mut zoom_f = state.zoom as f64;
        // Slider: left = zoomed out (many tiles), right = zoomed in (few tiles)
        // Invert the range so right = zoomed in
        let slider = egui::Slider::new(&mut zoom_f, ZOOM_MAX as f64..=ZOOM_MIN as f64)
            .logarithmic(true)
            .show_value(false)
            .clamping(egui::SliderClamping::Always);
        if ui.add(slider).changed() {
            state.zoom = zoom_f as usize;
        }
        if ui.small_button("\u{2795}").clicked() {
            // zoom in: show fewer tiles
            state.zoom = (state.zoom * 3 / 4).max(ZOOM_MIN);
        }
        ui.label(format!("{}x{}", state.zoom, state.zoom));
    });

    let zoom = state.zoom;
    let cx = map.x;
    let cy = map.y;

    // Determine display size to choose filtering mode
    let avail = ui.available_size();
    let display_side = avail.x.min(avail.y);

    // Choose filter: LINEAR when texture > display (downscaling), NEAREST otherwise
    let tex_pixels = zoom * TILE_SIZE;
    let use_linear = tex_pixels as f32 > display_side;

    // Rebuild texture when center, zoom, or filter mode changes
    let needs_rebuild = state.last_center != Some((cx, cy))
        || state.last_zoom != Some(zoom)
        || state.last_linear != use_linear;

    if needs_rebuild {
        let image = if let Some(wm) =
            world_map.filter(|_| map.location == crate::game::map::LocationType::Overworld)
        {
            build_detail_image(wm, atlas, cx, cy, zoom)
        } else {
            let scroll_x = map.scroll_x;
            let scroll_y = map.scroll_y;
            build_memory_image(&map.tiles, atlas, cx, cy, scroll_x, scroll_y)
        };

        let tex_opts = if use_linear {
            TextureOptions::LINEAR
        } else {
            TextureOptions::NEAREST
        };

        // Must recreate texture handle when filter mode changes
        if state.last_linear != use_linear {
            state.texture = Some(ui.ctx().load_texture("minimap", image, tex_opts));
        } else {
            match state.texture {
                Some(ref mut tex) => tex.set(image, tex_opts),
                None => {
                    state.texture = Some(ui.ctx().load_texture("minimap", image, tex_opts));
                }
            }
        }

        state.last_center = Some((cx, cy));
        state.last_zoom = Some(zoom);
        state.last_linear = use_linear;
    }

    // Display the texture, centered and square
    if let Some(ref texture) = state.texture {
        ui.vertical_centered(|ui| {
            ui.image(egui::load::SizedTexture::new(
                texture.id(),
                egui::vec2(display_side, display_side),
            ));
        });
    }
}

pub fn show_no_atlas(ui: &mut egui::Ui, status: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(status);
    });
}

/// Render the overworld from the WorldMap: each tile is 16x16 pixels.
fn build_detail_image(
    world: &WorldMap,
    atlas: &TileAtlas,
    center_x: u8,
    center_y: u8,
    zoom: usize,
) -> ColorImage {
    let img_side = zoom * TILE_SIZE;
    let mut pixels = vec![Color32::BLACK; img_side * img_side];
    let half = zoom as i32 / 2;

    for vy in 0..zoom {
        for vx in 0..zoom {
            let wx = (center_x as i32 - half + vx as i32).rem_euclid(256) as u8;
            let wy = (center_y as i32 - half + vy as i32).rem_euclid(256) as u8;

            let tile_id = world.get_tile(wx, wy) as u16;
            let rgba = atlas.tile_rgba(tile_id);

            for py in 0..TILE_SIZE {
                for px in 0..TILE_SIZE {
                    let src = (py * TILE_SIZE + px) * 4;
                    let dst = (vy * TILE_SIZE + py) * img_side + (vx * TILE_SIZE + px);
                    pixels[dst] = Color32::from_rgb(rgba[src], rgba[src + 1], rgba[src + 2]);
                }
            }
        }
    }

    draw_marker(
        &mut pixels,
        img_side,
        half as usize,
        half as usize,
        TILE_SIZE,
    );

    ColorImage {
        size: [img_side, img_side],
        pixels,
    }
}

/// Fallback rendering from the 32x32 in-memory tile grid (for towns/dungeons).
fn build_memory_image(
    tiles: &[u8; 1024],
    atlas: &TileAtlas,
    player_x: u8,
    player_y: u8,
    scroll_x: u8,
    scroll_y: u8,
) -> ColorImage {
    let grid_side: usize = 32;
    let img_side = grid_side * TILE_SIZE;
    let mut pixels = vec![Color32::BLACK; img_side * img_side];

    // Render all 32x32 tiles from the chunked memory grid
    for gy in 0..grid_side {
        for gx in 0..grid_side {
            let cx = gx / 16;
            let cy = gy / 16;
            let lx = gx % 16;
            let ly = gy % 16;
            let tile_id = tiles[(cy * 2 + cx) * 256 + ly * 16 + lx] as u16;
            let rgba = atlas.tile_rgba(tile_id);

            for py in 0..TILE_SIZE {
                for px in 0..TILE_SIZE {
                    let src = (py * TILE_SIZE + px) * 4;
                    let dst = (gy * TILE_SIZE + py) * img_side + (gx * TILE_SIZE + px);
                    pixels[dst] = Color32::from_rgb(rgba[src], rgba[src + 1], rgba[src + 2]);
                }
            }
        }
    }

    // Player marker at their position within the buffer
    let pbx = player_x.wrapping_sub(scroll_x) as usize;
    let pby = player_y.wrapping_sub(scroll_y) as usize;
    let marker_vx = pbx.min(grid_side - 1);
    let marker_vy = pby.min(grid_side - 1);
    draw_marker(&mut pixels, img_side, marker_vx, marker_vy, TILE_SIZE);

    ColorImage {
        size: [img_side, img_side],
        pixels,
    }
}

/// Draw a yellow border + red cross marker on a tile at view position (vx, vy).
fn draw_marker(
    pixels: &mut [Color32],
    img_side: usize,
    tile_vx: usize,
    tile_vy: usize,
    tile_px: usize,
) {
    let tx = tile_vx * tile_px;
    let ty = tile_vy * tile_px;
    let yellow = Color32::from_rgb(255, 255, 0);
    let red = Color32::from_rgb(255, 0, 0);

    for i in 0..tile_px {
        for t in 0..2 {
            set_px(pixels, img_side, tx + i, ty + t, yellow);
            set_px(pixels, img_side, tx + i, ty + tile_px - 1 - t, yellow);
            set_px(pixels, img_side, tx + t, ty + i, yellow);
            set_px(pixels, img_side, tx + tile_px - 1 - t, ty + i, yellow);
        }
    }
    let cx = tx + tile_px / 2;
    let cy = ty + tile_px / 2;
    for d in 0..4 {
        set_px(pixels, img_side, cx + d, cy, red);
        set_px(pixels, img_side, cx.wrapping_sub(d), cy, red);
        set_px(pixels, img_side, cx, cy + d, red);
        set_px(pixels, img_side, cx, cy.wrapping_sub(d), red);
    }
}

fn set_px(pixels: &mut [Color32], stride: usize, x: usize, y: usize, c: Color32) {
    if x < stride && y < stride {
        pixels[y * stride + x] = c;
    }
}
