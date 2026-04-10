use glow::HasContext;

use crate::tiles::atlas::{TILE_COUNT, TILE_SIZE};

/// Atlas texture layout: 32 columns x 16 rows of 16x16 tiles.
const ATLAS_COLS: u32 = 32;
const ATLAS_ROWS: u32 = (TILE_COUNT as u32).div_ceil(ATLAS_COLS);
const ATLAS_WIDTH: u32 = ATLAS_COLS * TILE_SIZE as u32;
const ATLAS_HEIGHT: u32 = ATLAS_ROWS * TILE_SIZE as u32;

/// Filtered atlas layout: each tile gets a 1px gutter so mipmaps can low-pass
/// inside a tile without bleeding into the neighboring tile.
const FILTERED_ATLAS_PADDING: u32 = 1;
const FILTERED_ATLAS_CELL_SIZE: u32 = TILE_SIZE as u32 + FILTERED_ATLAS_PADDING * 2;
const FILTERED_ATLAS_WIDTH: u32 = ATLAS_COLS * FILTERED_ATLAS_CELL_SIZE;
const FILTERED_ATLAS_HEIGHT: u32 = ATLAS_ROWS * FILTERED_ATLAS_CELL_SIZE;

const VERTEX_SHADER: &str = r#"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_grid;
uniform sampler2D u_atlas;
uniform sampler2D u_filtered_atlas;
uniform sampler2D u_objects;
uniform sampler2D u_lowpass;
uniform vec2 u_grid_size;
uniform float u_atlas_cols;
uniform float u_atlas_rows;
uniform vec2 u_filtered_atlas_size;
uniform float u_filtered_atlas_cell_size;
uniform float u_filtered_atlas_padding;
uniform vec2 u_player_tile;

const float TILE_SIZE_PX = 16.0;

vec2 atlas_uv_for_tile(float tile_id, vec2 tile_frac) {
    float col = mod(tile_id, u_atlas_cols);
    float row = floor(tile_id / u_atlas_cols);
    return vec2(
        (col + tile_frac.x) / u_atlas_cols,
        (row + tile_frac.y) / u_atlas_rows
    );
}

vec2 filtered_atlas_uv_for_tile(float tile_id, vec2 tile_frac) {
    float col = mod(tile_id, u_atlas_cols);
    float row = floor(tile_id / u_atlas_cols);
    vec2 atlas_px = vec2(
        col * u_filtered_atlas_cell_size + u_filtered_atlas_padding + 0.5 +
            tile_frac.x * (TILE_SIZE_PX - 1.0),
        row * u_filtered_atlas_cell_size + u_filtered_atlas_padding + 0.5 +
            tile_frac.y * (TILE_SIZE_PX - 1.0)
    );
    return atlas_px / u_filtered_atlas_size;
}

vec2 filtered_atlas_grad(vec2 tile_coord_grad) {
    return tile_coord_grad * (TILE_SIZE_PX - 1.0) / u_filtered_atlas_size;
}

void main() {
    vec2 tile_coord = v_uv * u_grid_size;
    vec2 tile_idx = floor(tile_coord);
    vec2 tile_frac = fract(tile_coord);

    // Clamp tile index to valid range
    tile_idx = clamp(tile_idx, vec2(0.0), u_grid_size - 1.0);

    // Sample grid texture at texel center to get tile ID
    vec2 grid_uv = (tile_idx + 0.5) / u_grid_size;
    float tile_id = floor(texture(u_grid, grid_uv).r * 255.0 + 0.5);
    vec2 filtered_grad_x = filtered_atlas_grad(dFdx(tile_coord));
    vec2 filtered_grad_y = filtered_atlas_grad(dFdy(tile_coord));

    vec4 detailed_color = texture(u_atlas, atlas_uv_for_tile(tile_id, tile_frac));
    vec4 filtered_color = textureGrad(
        u_filtered_atlas,
        filtered_atlas_uv_for_tile(tile_id, tile_frac),
        filtered_grad_x,
        filtered_grad_y
    );

    // Object overlay: if an object tile is present, render its sprite on top.
    // Object tile bytes are 0-255 in the R8 texture; the actual atlas sprite
    // is at tile_byte + 256 (the animated page of the 512-tile atlas).
    float obj_byte = floor(texture(u_objects, grid_uv).r * 255.0 + 0.5);
    if (obj_byte > 0.5) {
        float obj_tile = obj_byte + 256.0;
        vec4 obj_detailed = texture(u_atlas, atlas_uv_for_tile(obj_tile, tile_frac));
        if (obj_detailed.r > 0.0 || obj_detailed.g > 0.0 || obj_detailed.b > 0.0) {
            detailed_color = obj_detailed;
        }

        vec4 obj_filtered = textureGrad(
            u_filtered_atlas,
            filtered_atlas_uv_for_tile(obj_tile, tile_frac),
            filtered_grad_x,
            filtered_grad_y
        );
        filtered_color = vec4(
            filtered_color.rgb * (1.0 - obj_filtered.a) + obj_filtered.rgb * obj_filtered.a,
            1.0
        );
    }

    // Use the original nearest atlas while tiles are large on screen, then
    // blend to the mipmapped atlas once 16x16 tile art is actually minified.
    float tiles_per_pixel = max(fwidth(tile_coord).x, fwidth(tile_coord).y);
    float filtered_mix = smoothstep(0.20, 0.45, tiles_per_pixel);
    vec4 atlas_color = mix(detailed_color, filtered_color, filtered_mix);

    // Once a tile itself is near or below one screen pixel, atlas mipmaps are
    // no longer enough; blend to the whole-tile low-pass map to avoid
    // cross-tile moire.
    float lowpass_mix = smoothstep(0.85, 1.25, tiles_per_pixel);
    vec2 lowpass_uv = clamp(v_uv, 0.5 / u_grid_size, 1.0 - 0.5 / u_grid_size);
    frag_color = mix(atlas_color, texture(u_lowpass, lowpass_uv), lowpass_mix);

    // Player marker overlay (snap to tile boundary for odd zoom values)
    vec2 marker_tile = floor(u_player_tile);
    vec2 marker_min = marker_tile / u_grid_size;
    vec2 marker_max = (marker_tile + 1.0) / u_grid_size;

    if (v_uv.x >= marker_min.x && v_uv.x < marker_max.x &&
        v_uv.y >= marker_min.y && v_uv.y < marker_max.y) {

        // Local position within the marker tile [0, 1]
        vec2 local = (v_uv - marker_min) / (marker_max - marker_min);

        // Yellow border (2 pixels out of 16)
        float border = 2.0 / 16.0;
        bool on_border = local.x < border || local.x > (1.0 - border) ||
                         local.y < border || local.y > (1.0 - border);

        // Red cross (center, 4 pixels in each direction)
        float cross_half = 4.0 / 16.0;
        float pixel = 1.0 / 16.0;
        float cx = abs(local.x - 0.5);
        float cy = abs(local.y - 0.5);
        bool on_cross = (cx < pixel && cy < cross_half) ||
                        (cy < pixel && cx < cross_half);

        if (on_cross) {
            frag_color = vec4(1.0, 0.0, 0.0, 1.0);
        } else if (on_border) {
            frag_color = vec4(1.0, 1.0, 0.0, 1.0);
        }
    }
}
"#;

// Two triangles covering [-1,1] NDC with [0,1] UVs.
// UV y is flipped: top-left of screen = UV (0,0), bottom-left = UV (0,1).
#[rustfmt::skip]
const QUAD_VERTICES: [f32; 24] = [
    // pos.x, pos.y, uv.x, uv.y
    -1.0, -1.0,  0.0, 1.0,
     1.0, -1.0,  1.0, 1.0,
     1.0,  1.0,  1.0, 0.0,
    -1.0, -1.0,  0.0, 1.0,
     1.0,  1.0,  1.0, 0.0,
    -1.0,  1.0,  0.0, 0.0,
];

/// GPU-accelerated tilemap renderer using OpenGL.
pub struct MinimapGl {
    program: glow::Program,
    vao: glow::VertexArray,
    #[allow(dead_code)] // retained for destroy()
    vbo: glow::Buffer,
    atlas_texture: glow::Texture,
    filtered_atlas_texture: glow::Texture,
    grid_texture: glow::Texture,
    objects_texture: glow::Texture,
    lowpass_texture: glow::Texture,
    u_grid_size: glow::UniformLocation,
    u_atlas_cols: glow::UniformLocation,
    u_atlas_rows: glow::UniformLocation,
    u_filtered_atlas_size: glow::UniformLocation,
    u_filtered_atlas_cell_size: glow::UniformLocation,
    u_filtered_atlas_padding: glow::UniformLocation,
    u_player_tile: glow::UniformLocation,
}

impl MinimapGl {
    /// Create the renderer: compile shaders, upload atlas texture, create quad VBO.
    ///
    /// `atlas_rgba` is the raw sequential RGBA data from `TileAtlas::raw_data()`.
    pub fn new(gl: &glow::Context, atlas_rgba: &[u8]) -> Self {
        unsafe {
            let program = create_program(gl);

            // Uniform locations
            let u_grid_size = gl.get_uniform_location(program, "u_grid_size").unwrap();
            let u_atlas_cols = gl.get_uniform_location(program, "u_atlas_cols").unwrap();
            let u_atlas_rows = gl.get_uniform_location(program, "u_atlas_rows").unwrap();
            let u_filtered_atlas_size = gl
                .get_uniform_location(program, "u_filtered_atlas_size")
                .unwrap();
            let u_filtered_atlas_cell_size = gl
                .get_uniform_location(program, "u_filtered_atlas_cell_size")
                .unwrap();
            let u_filtered_atlas_padding = gl
                .get_uniform_location(program, "u_filtered_atlas_padding")
                .unwrap();
            let u_player_tile = gl.get_uniform_location(program, "u_player_tile").unwrap();

            // Set texture unit bindings (these are constant)
            gl.use_program(Some(program));
            let u_grid = gl.get_uniform_location(program, "u_grid").unwrap();
            let u_atlas = gl.get_uniform_location(program, "u_atlas").unwrap();
            let u_filtered_atlas = gl
                .get_uniform_location(program, "u_filtered_atlas")
                .unwrap();
            let u_objects = gl.get_uniform_location(program, "u_objects").unwrap();
            let u_lowpass = gl.get_uniform_location(program, "u_lowpass").unwrap();
            gl.uniform_1_i32(Some(&u_grid), 0);
            gl.uniform_1_i32(Some(&u_atlas), 1);
            gl.uniform_1_i32(Some(&u_filtered_atlas), 2);
            gl.uniform_1_i32(Some(&u_objects), 3);
            gl.uniform_1_i32(Some(&u_lowpass), 4);
            gl.use_program(None);

            // Fullscreen quad VBO
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&QUAD_VERTICES),
                glow::STATIC_DRAW,
            );

            // VAO
            let vao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            // a_pos (location 0): vec2 at offset 0
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            // a_uv (location 1): vec2 at offset 8
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
            gl.bind_vertex_array(None);

            // Atlas texture: rearrange from sequential into 2D grid layout
            let atlas_texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas_texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                ATLAS_WIDTH as i32,
                ATLAS_HEIGHT as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&rearrange_atlas(atlas_rgba))),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let filtered_atlas_texture = create_filtered_atlas_texture(gl);
            gl.bind_texture(glow::TEXTURE_2D, Some(filtered_atlas_texture));
            let filtered_atlas_buf = rearrange_filtered_atlas(atlas_rgba);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                FILTERED_ATLAS_WIDTH as i32,
                FILTERED_ATLAS_HEIGHT as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&filtered_atlas_buf)),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.bind_texture(glow::TEXTURE_2D, None);

            // Grid texture: created empty, filled later via update_grid
            let grid_texture = create_r8_texture(gl);

            // Object overlay texture: same dimensions as grid, filled via update_objects
            let objects_texture = create_r8_texture(gl);
            let lowpass_texture = create_lowpass_texture(gl);

            Self {
                program,
                vao,
                vbo,
                atlas_texture,
                filtered_atlas_texture,
                grid_texture,
                objects_texture,
                lowpass_texture,
                u_grid_size,
                u_atlas_cols,
                u_atlas_rows,
                u_filtered_atlas_size,
                u_filtered_atlas_cell_size,
                u_filtered_atlas_padding,
                u_player_tile,
            }
        }
    }

    /// Upload a new tile grid as an R8 texture. `tile_ids` is row-major.
    pub fn update_grid(&self, gl: &glow::Context, tile_ids: &[u8], width: u32, height: u32) {
        upload_r8(gl, self.grid_texture, tile_ids, width, height);
    }

    /// Upload the object overlay grid (same dimensions as the tile grid).
    /// Each byte is an object tile byte (0 = no object, non-zero = sprite at tile+256).
    pub fn update_objects(&self, gl: &glow::Context, object_ids: &[u8], width: u32, height: u32) {
        upload_r8(gl, self.objects_texture, object_ids, width, height);
    }

    /// Upload the low-pass RGBA map used for zoomed-out rendering.
    pub fn update_lowpass(&self, gl: &glow::Context, lowpass_rgba: &[u8], width: u32, height: u32) {
        upload_rgba(gl, self.lowpass_texture, lowpass_rgba, width, height);
    }

    /// Render the tilemap into the given viewport.
    pub fn paint(
        &self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        grid_size: [f32; 2],
        player_tile: [f32; 2],
    ) {
        let vp = info.viewport_in_pixels();
        let clip = info.clip_rect_in_pixels();

        unsafe {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);

            gl.use_program(Some(self.program));

            // Bind textures
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.grid_texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas_texture));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.filtered_atlas_texture));
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.objects_texture));
            gl.active_texture(glow::TEXTURE4);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.lowpass_texture));

            // Set uniforms
            gl.uniform_2_f32(Some(&self.u_grid_size), grid_size[0], grid_size[1]);
            gl.uniform_1_f32(Some(&self.u_atlas_cols), ATLAS_COLS as f32);
            gl.uniform_1_f32(Some(&self.u_atlas_rows), ATLAS_ROWS as f32);
            gl.uniform_2_f32(
                Some(&self.u_filtered_atlas_size),
                FILTERED_ATLAS_WIDTH as f32,
                FILTERED_ATLAS_HEIGHT as f32,
            );
            gl.uniform_1_f32(
                Some(&self.u_filtered_atlas_cell_size),
                FILTERED_ATLAS_CELL_SIZE as f32,
            );
            gl.uniform_1_f32(
                Some(&self.u_filtered_atlas_padding),
                FILTERED_ATLAS_PADDING as f32,
            );
            gl.uniform_2_f32(Some(&self.u_player_tile), player_tile[0], player_tile[1]);

            // Draw fullscreen quad
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);

            // Clean up bindings
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE4);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }

    /// Delete all GL resources.
    #[allow(dead_code)]
    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_texture(self.atlas_texture);
            gl.delete_texture(self.filtered_atlas_texture);
            gl.delete_texture(self.grid_texture);
            gl.delete_texture(self.objects_texture);
            gl.delete_texture(self.lowpass_texture);
        }
    }
}

/// Create an empty R8 texture with NEAREST filtering and CLAMP_TO_EDGE wrapping.
fn create_r8_texture(gl: &glow::Context) -> glow::Texture {
    unsafe {
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        tex
    }
}

/// Create an RGBA texture configured for tile-safe mipmapped atlas sampling.
fn create_filtered_atlas_texture(gl: &glow::Context) -> glow::Texture {
    unsafe {
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        tex
    }
}

/// Create an RGBA texture configured for mipmapped overview sampling.
fn create_lowpass_texture(gl: &glow::Context) -> glow::Texture {
    unsafe {
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        tex
    }
}

/// Upload row-major u8 data to an R8 texture.
fn upload_r8(gl: &glow::Context, texture: glow::Texture, data: &[u8], width: u32, height: u32) {
    assert_eq!(
        data.len(),
        width as usize * height as usize,
        "R8 upload requires exactly width * height bytes"
    );
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(data)),
        );
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

/// Upload row-major RGBA data and rebuild mipmaps for zoomed-out sampling.
fn upload_rgba(gl: &glow::Context, texture: glow::Texture, data: &[u8], width: u32, height: u32) {
    assert_eq!(
        data.len(),
        width as usize * height as usize * 4,
        "RGBA upload requires exactly width * height * 4 bytes"
    );
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(data)),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

/// Rearrange sequential tile RGBA data into the exact 32x16 atlas layout.
fn rearrange_atlas(sequential_rgba: &[u8]) -> Vec<u8> {
    const EXPECTED_LEN: usize = TILE_COUNT * TILE_SIZE * TILE_SIZE * 4;
    assert_eq!(
        sequential_rgba.len(),
        EXPECTED_LEN,
        "atlas RGBA data must be exactly {EXPECTED_LEN} bytes"
    );
    let stride = ATLAS_WIDTH as usize * 4;
    let mut buf = vec![0u8; stride * ATLAS_HEIGHT as usize];

    for tile_id in 0..TILE_COUNT {
        let col = tile_id % ATLAS_COLS as usize;
        let row = tile_id / ATLAS_COLS as usize;
        let src_base = tile_id * TILE_SIZE * TILE_SIZE * 4;

        for py in 0..TILE_SIZE {
            let src_off = src_base + py * TILE_SIZE * 4;
            let dst_y = row * TILE_SIZE + py;
            let dst_x = col * TILE_SIZE;
            let dst_off = dst_y * stride + dst_x * 4;
            let row_bytes = TILE_SIZE * 4; // 64 bytes per tile row
            buf[dst_off..dst_off + row_bytes]
                .copy_from_slice(&sequential_rgba[src_off..src_off + row_bytes]);
        }
    }

    buf
}

/// Rearrange sequential tile RGBA data into the padded mipmapped atlas layout.
fn rearrange_filtered_atlas(sequential_rgba: &[u8]) -> Vec<u8> {
    const EXPECTED_LEN: usize = TILE_COUNT * TILE_SIZE * TILE_SIZE * 4;
    assert_eq!(
        sequential_rgba.len(),
        EXPECTED_LEN,
        "atlas RGBA data must be exactly {EXPECTED_LEN} bytes"
    );
    let stride = FILTERED_ATLAS_WIDTH as usize * 4;
    let mut buf = vec![0u8; stride * FILTERED_ATLAS_HEIGHT as usize];

    for tile_id in 0..TILE_COUNT {
        let col = tile_id % ATLAS_COLS as usize;
        let row = tile_id / ATLAS_COLS as usize;
        let src_base = tile_id * TILE_SIZE * TILE_SIZE * 4;
        let cell_x = col * FILTERED_ATLAS_CELL_SIZE as usize;
        let cell_y = row * FILTERED_ATLAS_CELL_SIZE as usize;

        for py in 0..TILE_SIZE {
            let src_off = src_base + py * TILE_SIZE * 4;
            let dst_y = cell_y + FILTERED_ATLAS_PADDING as usize + py;
            let dst_x = cell_x + FILTERED_ATLAS_PADDING as usize;
            let dst_off = dst_y * stride + dst_x * 4;
            let row_bytes = TILE_SIZE * 4; // 64 bytes per tile row
            if tile_id >= 256 {
                for px in 0..TILE_SIZE {
                    let src_px = src_off + px * 4;
                    let dst_px = dst_off + px * 4;
                    let rgba = &sequential_rgba[src_px..src_px + 4];
                    buf[dst_px] = rgba[0];
                    buf[dst_px + 1] = rgba[1];
                    buf[dst_px + 2] = rgba[2];
                    buf[dst_px + 3] = if rgba[0] == 0 && rgba[1] == 0 && rgba[2] == 0 {
                        0
                    } else {
                        255
                    };
                }
            } else {
                buf[dst_off..dst_off + row_bytes]
                    .copy_from_slice(&sequential_rgba[src_off..src_off + row_bytes]);
            }
        }

        for py in 0..TILE_SIZE {
            let src_y = cell_y + FILTERED_ATLAS_PADDING as usize + py;
            let row_off = src_y * stride;
            let first_px = row_off + (cell_x + FILTERED_ATLAS_PADDING as usize) * 4;
            let last_px = row_off + (cell_x + FILTERED_ATLAS_PADDING as usize + TILE_SIZE - 1) * 4;
            let left_gutter = row_off + cell_x * 4;
            let right_gutter = row_off + (cell_x + FILTERED_ATLAS_PADDING as usize + TILE_SIZE) * 4;
            let first_rgba = [
                buf[first_px],
                buf[first_px + 1],
                buf[first_px + 2],
                buf[first_px + 3],
            ];
            let last_rgba = [
                buf[last_px],
                buf[last_px + 1],
                buf[last_px + 2],
                buf[last_px + 3],
            ];
            buf[left_gutter..left_gutter + 4].copy_from_slice(&first_rgba);
            buf[right_gutter..right_gutter + 4].copy_from_slice(&last_rgba);
        }

        let interior_y = cell_y + FILTERED_ATLAS_PADDING as usize;
        let top_y = cell_y;
        let bottom_y = cell_y + FILTERED_ATLAS_PADDING as usize + TILE_SIZE;
        let row_bytes = FILTERED_ATLAS_CELL_SIZE as usize * 4;
        let interior_off = interior_y * stride + cell_x * 4;
        let bottom_src_off =
            (cell_y + FILTERED_ATLAS_PADDING as usize + TILE_SIZE - 1) * stride + cell_x * 4;
        let top_off = top_y * stride + cell_x * 4;
        let bottom_off = bottom_y * stride + cell_x * 4;
        let top_row = buf[interior_off..interior_off + row_bytes].to_vec();
        let bottom_row = buf[bottom_src_off..bottom_src_off + row_bytes].to_vec();
        buf[top_off..top_off + row_bytes].copy_from_slice(&top_row);
        buf[bottom_off..bottom_off + row_bytes].copy_from_slice(&bottom_row);
    }

    buf
}

/// Compile and link the tilemap shader program.
fn create_program(gl: &glow::Context) -> glow::Program {
    unsafe {
        let program = gl.create_program().expect("create program");

        let vert = gl.create_shader(glow::VERTEX_SHADER).expect("create vs");
        gl.shader_source(vert, VERTEX_SHADER);
        gl.compile_shader(vert);
        assert!(
            gl.get_shader_compile_status(vert),
            "Vertex shader compile failed: {}",
            gl.get_shader_info_log(vert)
        );

        let frag = gl.create_shader(glow::FRAGMENT_SHADER).expect("create fs");
        gl.shader_source(frag, FRAGMENT_SHADER);
        gl.compile_shader(frag);
        assert!(
            gl.get_shader_compile_status(frag),
            "Fragment shader compile failed: {}",
            gl.get_shader_info_log(frag)
        );

        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);
        assert!(
            gl.get_program_link_status(program),
            "Shader link failed: {}",
            gl.get_program_info_log(program)
        );

        gl.detach_shader(program, vert);
        gl.detach_shader(program, frag);
        gl.delete_shader(vert);
        gl.delete_shader(frag);

        program
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rearranged_filtered_atlas_adds_tile_gutters() {
        let mut sequential = vec![0u8; TILE_COUNT * TILE_SIZE * TILE_SIZE * 4];
        let tile0 = &mut sequential[..TILE_SIZE * TILE_SIZE * 4];
        for px in tile0.chunks_exact_mut(4) {
            px.copy_from_slice(&[10, 20, 30, 255]);
        }

        let atlas = rearrange_filtered_atlas(&sequential);
        let stride = FILTERED_ATLAS_WIDTH as usize * 4;
        let top_left = &atlas[(FILTERED_ATLAS_PADDING as usize * stride)
            + FILTERED_ATLAS_PADDING as usize * 4
            ..(FILTERED_ATLAS_PADDING as usize * stride) + FILTERED_ATLAS_PADDING as usize * 4 + 4];
        let left_gutter = &atlas[FILTERED_ATLAS_PADDING as usize * stride
            ..FILTERED_ATLAS_PADDING as usize * stride + 4];
        let top_gutter =
            &atlas[FILTERED_ATLAS_PADDING as usize * 4..FILTERED_ATLAS_PADDING as usize * 4 + 4];

        assert_eq!(top_left, [10, 20, 30, 255]);
        assert_eq!(left_gutter, [10, 20, 30, 255]);
        assert_eq!(top_gutter, [10, 20, 30, 255]);
    }

    #[test]
    fn filtered_atlas_marks_black_object_pixels_transparent() {
        let mut sequential = vec![0u8; TILE_COUNT * TILE_SIZE * TILE_SIZE * 4];
        let object_tile =
            &mut sequential[256 * TILE_SIZE * TILE_SIZE * 4..257 * TILE_SIZE * TILE_SIZE * 4];
        object_tile[0..4].copy_from_slice(&[0, 0, 0, 255]);
        object_tile[4..8].copy_from_slice(&[200, 100, 50, 255]);

        let atlas = rearrange_filtered_atlas(&sequential);
        let stride = FILTERED_ATLAS_WIDTH as usize * 4;
        let object_row = (256 / ATLAS_COLS as usize) * FILTERED_ATLAS_CELL_SIZE as usize;
        let row_start = (object_row + FILTERED_ATLAS_PADDING as usize) * stride
            + FILTERED_ATLAS_PADDING as usize * 4;

        assert_eq!(&atlas[row_start..row_start + 4], [0, 0, 0, 0]);
        assert_eq!(&atlas[row_start + 4..row_start + 8], [200, 100, 50, 255]);
    }
}
