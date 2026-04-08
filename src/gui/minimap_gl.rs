use glow::HasContext;

use crate::tiles::atlas::{TILE_COUNT, TILE_SIZE};

/// Atlas texture layout: 32 columns x 16 rows of 16x16 tiles = 512x256 pixels.
const ATLAS_COLS: u32 = 32;
const ATLAS_ROWS: u32 = (TILE_COUNT as u32).div_ceil(ATLAS_COLS);
const ATLAS_WIDTH: u32 = ATLAS_COLS * TILE_SIZE as u32;
const ATLAS_HEIGHT: u32 = ATLAS_ROWS * TILE_SIZE as u32;

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
uniform vec2 u_grid_size;
uniform float u_atlas_cols;
uniform float u_atlas_rows;
uniform vec2 u_player_tile;

void main() {
    vec2 tile_coord = v_uv * u_grid_size;
    vec2 tile_idx = floor(tile_coord);
    vec2 tile_frac = fract(tile_coord);

    // Clamp tile index to valid range
    tile_idx = clamp(tile_idx, vec2(0.0), u_grid_size - 1.0);

    // Sample grid texture at texel center to get tile ID
    vec2 grid_uv = (tile_idx + 0.5) / u_grid_size;
    float tile_id = floor(texture(u_grid, grid_uv).r * 255.0 + 0.5);

    // Compute atlas UV from tile ID
    float col = mod(tile_id, u_atlas_cols);
    float row = floor(tile_id / u_atlas_cols);
    vec2 atlas_uv = vec2(
        (col + tile_frac.x) / u_atlas_cols,
        (row + tile_frac.y) / u_atlas_rows
    );

    frag_color = texture(u_atlas, atlas_uv);

    // Player marker overlay
    vec2 marker_min = u_player_tile / u_grid_size;
    vec2 marker_max = (u_player_tile + 1.0) / u_grid_size;

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
    grid_texture: glow::Texture,
    u_grid_size: glow::UniformLocation,
    u_atlas_cols: glow::UniformLocation,
    u_atlas_rows: glow::UniformLocation,
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
            let u_player_tile = gl.get_uniform_location(program, "u_player_tile").unwrap();

            // Set texture unit bindings (these are constant)
            gl.use_program(Some(program));
            let u_grid = gl.get_uniform_location(program, "u_grid").unwrap();
            let u_atlas = gl.get_uniform_location(program, "u_atlas").unwrap();
            gl.uniform_1_i32(Some(&u_grid), 0);
            gl.uniform_1_i32(Some(&u_atlas), 1);
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
            let atlas_buf = rearrange_atlas(atlas_rgba);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                ATLAS_WIDTH as i32,
                ATLAS_HEIGHT as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&atlas_buf)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            // Grid texture: created empty, filled later via update_grid
            let grid_texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(grid_texture));
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

            Self {
                program,
                vao,
                vbo,
                atlas_texture,
                grid_texture,
                u_grid_size,
                u_atlas_cols,
                u_atlas_rows,
                u_player_tile,
            }
        }
    }

    /// Upload a new tile grid as an R8 texture. `tile_ids` is row-major.
    pub fn update_grid(&self, gl: &glow::Context, tile_ids: &[u8], width: u32, height: u32) {
        assert_eq!(
            tile_ids.len(),
            width as usize * height as usize,
            "grid upload requires exactly width * height bytes"
        );
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.grid_texture));
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
                glow::PixelUnpackData::Slice(Some(tile_ids)),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
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

            // Set uniforms
            gl.uniform_2_f32(Some(&self.u_grid_size), grid_size[0], grid_size[1]);
            gl.uniform_1_f32(Some(&self.u_atlas_cols), ATLAS_COLS as f32);
            gl.uniform_1_f32(Some(&self.u_atlas_rows), ATLAS_ROWS as f32);
            gl.uniform_2_f32(Some(&self.u_player_tile), player_tile[0], player_tile[1]);

            // Draw fullscreen quad
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);

            // Clean up bindings
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
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
            gl.delete_texture(self.grid_texture);
        }
    }
}

/// Rearrange sequential tile RGBA data into a 2D atlas texture (32 cols x 16 rows).
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
