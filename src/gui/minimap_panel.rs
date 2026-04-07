use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

use crate::game::map::MapState;
use crate::game::world_map::WorldMap;
use crate::tiles::atlas::{TILE_SIZE, TileAtlas};

/// Zoom presets: (label, tiles per axis).
const ZOOM_PRESETS: &[(&str, usize)] =
    &[("Close", 16), ("Medium", 48), ("Far", 128), ("World", 256)];

/// Threshold: at this zoom level or below, render full tile sprites.
/// Above this, render 1 pixel per tile for performance.
const DETAIL_THRESHOLD: usize = 64;

pub struct MinimapState {
    pub map: Option<MapState>,
    texture: Option<TextureHandle>,
    zoom_idx: usize,
    last_center: Option<(u8, u8)>,
    last_zoom: Option<usize>,
    /// Cached color for each tile ID (0-255), sampled from atlas center pixel.
    overview_colors: Option<[Color32; 256]>,
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            map: None,
            texture: None,
            zoom_idx: 1, // Medium
            last_center: None,
            last_zoom: None,
            overview_colors: None,
        }
    }
}

impl MinimapState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Build the overview color table by sampling the center pixel of each tile.
fn build_overview_colors(atlas: &TileAtlas) -> [Color32; 256] {
    let mut colors = [Color32::BLACK; 256];
    for id in 0..256u16 {
        let rgba = atlas.tile_rgba(id);
        // Sample center pixel (row 8, col 8) of the 16x16 tile
        let offset = (8 * TILE_SIZE + 8) * 4;
        colors[id as usize] = Color32::from_rgb(rgba[offset], rgba[offset + 1], rgba[offset + 2]);
    }
    colors
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
        for (idx, &(label, _)) in ZOOM_PRESETS.iter().enumerate() {
            if ui.selectable_label(state.zoom_idx == idx, label).clicked() {
                state.zoom_idx = idx;
            }
        }
    });

    let zoom = ZOOM_PRESETS[state.zoom_idx].1;
    let cx = map.x;
    let cy = map.y;

    // Build overview colors lazily
    if state.overview_colors.is_none() {
        state.overview_colors = Some(build_overview_colors(atlas));
    }

    // Rebuild texture when center or zoom changes
    let needs_rebuild = state.last_center != Some((cx, cy)) || state.last_zoom != Some(zoom);

    if needs_rebuild {
        let image = if let Some(wm) =
            world_map.filter(|_| map.location == crate::game::map::LocationType::Overworld)
        {
            if zoom <= DETAIL_THRESHOLD {
                build_detail_image(wm, atlas, cx, cy, zoom)
            } else {
                build_overview_image(wm, state.overview_colors.as_ref().unwrap(), cx, cy, zoom)
            }
        } else {
            // Fallback: render from the 32x32 in-memory tile grid
            let scroll_x = map.scroll_x;
            let scroll_y = map.scroll_y;
            build_memory_image(&map.tiles, atlas, cx, cy, scroll_x, scroll_y)
        };

        match state.texture {
            Some(ref mut tex) => tex.set(image, TextureOptions::NEAREST),
            None => {
                state.texture = Some(ui.ctx().load_texture(
                    "minimap",
                    image,
                    TextureOptions::NEAREST,
                ));
            }
        }
        state.last_center = Some((cx, cy));
        state.last_zoom = Some(zoom);
    }

    // Display the texture, centered and square
    if let Some(ref texture) = state.texture {
        let avail = ui.available_size();
        let side = avail.x.min(avail.y);
        ui.vertical_centered(|ui| {
            ui.image(egui::load::SizedTexture::new(
                texture.id(),
                egui::vec2(side, side),
            ));
        });
    }
}

pub fn show_no_atlas(ui: &mut egui::Ui, status: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(status);
    });
}

/// Detailed rendering: each tile is 16x16 pixels. Used for zoom <= 64.
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
            let wx = center_x as i32 - half + vx as i32;
            let wy = center_y as i32 - half + vy as i32;
            if wx < 0 || wy < 0 || wx > 255 || wy > 255 {
                continue;
            }

            let tile_id = world.get_tile(wx as u8, wy as u8) as u16;
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

/// Overview rendering: each tile is 1 pixel. Used for zoom > 64.
fn build_overview_image(
    world: &WorldMap,
    colors: &[Color32; 256],
    center_x: u8,
    center_y: u8,
    zoom: usize,
) -> ColorImage {
    let img_side = zoom;
    let mut pixels = vec![Color32::BLACK; img_side * img_side];
    let half = zoom as i32 / 2;

    for vy in 0..zoom {
        for vx in 0..zoom {
            let wx = center_x as i32 - half + vx as i32;
            let wy = center_y as i32 - half + vy as i32;
            if wx < 0 || wy < 0 || wx > 255 || wy > 255 {
                continue;
            }

            let tile_id = world.get_tile(wx as u8, wy as u8);
            pixels[vy * img_side + vx] = colors[tile_id as usize];
        }
    }

    // Player marker: 3x3 bright cross at center
    let c = half as usize;
    let marker = Color32::from_rgb(255, 255, 0);
    for d in 0..3 {
        set_px(&mut pixels, img_side, c + d, c, marker);
        set_px(&mut pixels, img_side, c.wrapping_sub(d), c, marker);
        set_px(&mut pixels, img_side, c, c + d, marker);
        set_px(&mut pixels, img_side, c, c.wrapping_sub(d), marker);
    }

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

    let pbx = player_x.wrapping_sub(scroll_x) as i32;
    let pby = player_y.wrapping_sub(scroll_y) as i32;
    let half = grid_side as i32 / 2;
    let origin_x = (pbx - half).clamp(0, 0); // grid is only 32 wide
    let origin_y = (pby - half).clamp(0, 0);

    for vy in 0..grid_side {
        for vx in 0..grid_side {
            let gx = origin_x + vx as i32;
            let gy = origin_y + vy as i32;
            if gx < 0 || gy < 0 || gx >= grid_side as i32 || gy >= grid_side as i32 {
                continue;
            }

            let gxu = gx as usize;
            let gyu = gy as usize;
            let cx = gxu / 16;
            let cy = gyu / 16;
            let lx = gxu % 16;
            let ly = gyu % 16;
            let tile_id = tiles[(cy * 2 + cx) * 256 + ly * 16 + lx] as u16;
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

    // Player marker
    let marker_vx = pbx.clamp(0, grid_side as i32 - 1) as usize;
    let marker_vy = pby.clamp(0, grid_side as i32 - 1) as usize;
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

    // Yellow border (2px)
    for i in 0..tile_px {
        for t in 0..2 {
            set_px(pixels, img_side, tx + i, ty + t, yellow);
            set_px(pixels, img_side, tx + i, ty + tile_px - 1 - t, yellow);
            set_px(pixels, img_side, tx + t, ty + i, yellow);
            set_px(pixels, img_side, tx + tile_px - 1 - t, ty + i, yellow);
        }
    }
    // Red cross
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
