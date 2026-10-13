//! The WebGL2 viewport: draws a design's segment meshes and the editing
//! overlays (brush outline, mirror planes) from an orbiting camera.

use crate::camera::{multiply, Camera, Mat4};
use std::collections::HashMap;
use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, HtmlImageElement, WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlTexture, WebGlUniformLocation, WebGlVertexArrayObject,
};
use yard_core::mesh::{ChunkMesh, Mesh, Vertex, VERTEX_BYTES};

const VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec4 a_color;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in float a_tile;
layout(location = 4) in float a_light;
layout(location = 5) in vec2 a_fine;
uniform mat4 u_mvp;
out vec4 v_color;
out vec2 v_uv;
out float v_light;
// Column and row of the tile in its sheet, and the sheet; sheet -1 = no
// texture, column -1 = the whole sheet (the skin of a block's 3D model).
flat out vec3 v_tile;
void main() {
    v_color = a_color;
    v_uv = a_uv;
    v_light = a_light;
    if (a_tile > 65534.5) {
        v_tile = vec3(0.0, 0.0, -1.0);
    } else if (a_tile >= 61440.0) {
        // A position anywhere in the sheet, in sixteen bits: a_uv has the
        // high bytes, a_fine the low ones.
        v_tile = vec3(-1.0, 0.0, a_tile - 61440.0);
        v_uv = (floor(a_uv * 255.0 + 0.5) * 256.0 + a_fine) / 65535.0;
    } else {
        float page = floor(a_tile / 256.0);
        float cell = a_tile - page * 256.0;
        v_tile = vec3(mod(cell, 16.0), floor(cell / 16.0), page);
    }
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}"#;

// Each face shows its tile of the game's texture sheets, or its flat colour
// while the sheets are loading. A thin darker band along each block's edges
// keeps neighbouring blocks apart; it fades out once blocks are a few pixels wide.
const FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
precision mediump sampler2DArray;
in vec4 v_color;
in vec2 v_uv;
in float v_light;
flat in vec3 v_tile;
uniform float u_edges;
uniform float u_pages;
uniform sampler2DArray u_atlas;
out vec4 o_color;
void main() {
    vec4 base = v_color;
    float strength = 0.28;
    if (v_tile.x < 0.0) {
        // A 3D model: its own skin, no block edges.
        strength = 0.0;
        if (v_tile.z < u_pages) {
            base = vec4(texture(u_atlas, vec3(v_uv, v_tile.z)).rgb, v_color.a);
        }
    } else if (v_tile.z >= 0.0 && v_tile.z < u_pages) {
        // Stay half a texel inside the tile so its neighbours do not bleed in.
        vec2 inner = clamp(v_uv, 0.5 / 64.0, 1.0 - 0.5 / 64.0);
        vec4 t = texture(u_atlas, vec3((v_tile.xy + inner) / 16.0, v_tile.z));
        if (t.a < 0.25) discard; // the holes in grates and plants
        base = vec4(t.rgb, v_color.a < 0.99 ? clamp(t.a, 0.3, 0.85) : 1.0);
        strength = 0.12;
    }
    vec2 d = min(v_uv, 1.0 - v_uv);
    float edge = min(d.x, d.y);
    float w = max(fwidth(edge), 0.0005);
    float line = 1.0 - smoothstep(0.02, 0.02 + w * 1.5, edge);
    float fade = clamp(1.0 - w * 8.0, 0.0, 1.0);
    o_color = vec4(base.rgb * v_light * (1.0 - strength * line * fade * u_edges), base.a);
}"#;

/// Pixels along one edge of a texture sheet: sixteen 64-pixel tiles.
const SHEET_SIZE: i32 = 1024;

/// Fills the texture sheets in as their images arrive. Kept apart from the
/// renderer so image callbacks can hold it.
#[derive(Clone)]
pub struct AtlasLoader {
    gl: Gl,
    texture: WebGlTexture,
}

impl AtlasLoader {
    /// Copies one sheet into place. False if the image is not a sheet.
    pub fn put(&self, page: u32, image: &HtmlImageElement) -> bool {
        if image.natural_width() as i32 != SHEET_SIZE || image.natural_height() as i32 != SHEET_SIZE {
            return false;
        }
        self.gl.bind_texture(Gl::TEXTURE_2D_ARRAY, Some(&self.texture));
        self.gl
            .tex_sub_image_3d_with_html_image_element(
                Gl::TEXTURE_2D_ARRAY,
                0,
                0,
                0,
                page as i32,
                SHEET_SIZE,
                SHEET_SIZE,
                1,
                Gl::RGBA,
                Gl::UNSIGNED_BYTE,
                image,
            )
            .is_ok()
    }

    /// Builds the smaller copies used at a distance, once every sheet is in.
    pub fn finish(&self) {
        self.gl.bind_texture(Gl::TEXTURE_2D_ARRAY, Some(&self.texture));
        self.gl.generate_mipmap(Gl::TEXTURE_2D_ARRAY);
    }
}

/// One uploaded mesh.
struct Buffers {
    vao: WebGlVertexArrayObject,
    vertices: WebGlBuffer,
    indices: WebGlBuffer,
    count: i32,
}

#[derive(Default)]
struct Chunk {
    opaque: Option<Buffers>,
    translucent: Option<Buffers>,
}

/// The identity: a group drawn where its own coordinates say.
pub const IN_PLACE: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// The segments of one entity, and where the entity sits in the view: its
/// own place for an entity shown alone, its dock for one shown on its parent.
struct Group {
    placed: Mat4,
    chunks: HashMap<(i32, i32, i32), Chunk>,
}

/// Lines and see-through triangles drawn over the design. Positions are in
/// design space.
#[derive(Default)]
pub struct Overlay {
    pub lines: Mesh,
    pub fills: Mesh,
}

impl Overlay {
    fn vertex(pos: [f32; 3], color: [u8; 4]) -> Vertex {
        // UV in the middle of a face keeps the edge shading off overlay geometry.
        Vertex { pos, color, uv: [128, 128], tile: yard_core::mesh::NO_TILE, light: 255, fine: [0, 0] }
    }

    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], color: [u8; 4]) {
        let base = self.lines.vertices.len() as u32;
        self.lines.vertices.push(Self::vertex(a, color));
        self.lines.vertices.push(Self::vertex(b, color));
        self.lines.indices.extend_from_slice(&[base, base + 1]);
    }

    /// The twelve edges of a box.
    pub fn wire_box(&mut self, lo: [f32; 3], hi: [f32; 3], color: [u8; 4]) {
        let corner = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
            self.line(corner(a), corner(b), color);
        }
    }

    /// The twelve edges of a box that sits in a docked entity: the box is
    /// given in that entity's coordinates and drawn where `placed` puts it.
    pub fn wire_box_placed(&mut self, lo: [f32; 3], hi: [f32; 3], placed: &yard_core::dock::Placement, color: [u8; 4]) {
        let corner = |i: usize| placed.at([if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }]);
        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
            self.line(corner(a), corner(b), color);
        }
    }

    /// The six faces of a box, filled.
    pub fn solid_box(&mut self, lo: [f32; 3], hi: [f32; 3], color: [u8; 4]) {
        let corner = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        for face in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
            self.quad(face.map(corner), color);
        }
    }

    /// A flat four-cornered patch, visible from both sides.
    pub fn quad(&mut self, corners: [[f32; 3]; 4], color: [u8; 4]) {
        let base = self.fills.vertices.len() as u32;
        self.fills.vertices.extend(corners.iter().map(|&c| Self::vertex(c, color)));
        self.fills.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

pub struct Renderer {
    canvas: HtmlCanvasElement,
    gl: Gl,
    mvp: WebGlUniformLocation,
    edges: WebGlUniformLocation,
    pages: WebGlUniformLocation,
    /// Texture sheets that are loaded and may be drawn from; 0 until all are in.
    atlas_pages: u32,
    groups: Vec<Group>,
    overlay_lines: Option<Buffers>,
    overlay_fills: Option<Buffers>,
    /// Marks that show through the design: the selected block and its links.
    mark_lines: Option<Buffers>,
    mark_fills: Option<Buffers>,
    pub camera: Camera,
}

impl Renderer {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let gl: Gl = canvas
            .get_context("webgl2")
            .ok()
            .flatten()
            .ok_or("This browser does not support WebGL2, which the shipyard needs.")?
            .unchecked_into();
        let program = link(&gl, VERTEX_SHADER, FRAGMENT_SHADER)?;
        gl.use_program(Some(&program));
        let mvp = gl.get_uniform_location(&program, "u_mvp").ok_or("shader has no u_mvp")?;
        let edges = gl.get_uniform_location(&program, "u_edges").ok_or("shader has no u_edges")?;
        let pages = gl.get_uniform_location(&program, "u_pages").ok_or("shader has no u_pages")?;
        if let Some(atlas) = gl.get_uniform_location(&program, "u_atlas") {
            gl.uniform1i(Some(&atlas), 0);
        }
        gl.enable(Gl::DEPTH_TEST);
        gl.depth_func(Gl::LEQUAL);
        gl.blend_func(Gl::SRC_ALPHA, Gl::ONE_MINUS_SRC_ALPHA);
        gl.clear_color(0.051, 0.063, 0.078, 1.0);
        Ok(Self {
            canvas,
            gl,
            mvp,
            edges,
            pages,
            atlas_pages: 0,
            groups: Vec::new(),
            overlay_lines: None,
            overlay_fills: None,
            mark_lines: None,
            mark_fills: None,
            camera: Camera::default(),
        })
    }

    fn upload(&self, mesh: &Mesh) -> Option<Buffers> {
        if mesh.is_empty() {
            return None;
        }
        let gl = &self.gl;
        let vao = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(&vao));
        let vertices = gl.create_buffer()?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&vertices));
        gl.buffer_data_with_u8_array(Gl::ARRAY_BUFFER, &mesh.vertex_bytes(), Gl::STATIC_DRAW);
        let stride = VERTEX_BYTES as i32;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, Gl::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 4, Gl::UNSIGNED_BYTE, true, stride, 12);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_with_i32(2, 2, Gl::UNSIGNED_BYTE, true, stride, 16);
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_with_i32(3, 1, Gl::UNSIGNED_SHORT, false, stride, 18);
        gl.enable_vertex_attrib_array(4);
        gl.vertex_attrib_pointer_with_i32(4, 1, Gl::UNSIGNED_BYTE, true, stride, 20);
        gl.enable_vertex_attrib_array(5);
        gl.vertex_attrib_pointer_with_i32(5, 2, Gl::UNSIGNED_BYTE, false, stride, 21);
        let indices = gl.create_buffer()?;
        gl.bind_buffer(Gl::ELEMENT_ARRAY_BUFFER, Some(&indices));
        gl.buffer_data_with_u8_array(Gl::ELEMENT_ARRAY_BUFFER, &mesh.index_bytes(), Gl::STATIC_DRAW);
        gl.bind_vertex_array(None);
        Some(Buffers { vao, vertices, indices, count: mesh.indices.len() as i32 })
    }

    fn free(&self, buffers: Option<Buffers>) {
        if let Some(b) = buffers {
            self.gl.delete_vertex_array(Some(&b.vao));
            self.gl.delete_buffer(Some(&b.vertices));
            self.gl.delete_buffer(Some(&b.indices));
        }
    }

    /// Makes room for `pages` texture sheets and returns what fills them in.
    /// Until [`Renderer::atlas_ready`] is called, blocks keep their flat colours.
    pub fn start_atlas(&mut self, pages: u32) -> Option<AtlasLoader> {
        if pages == 0 {
            return None;
        }
        let gl = &self.gl;
        let texture = gl.create_texture()?;
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D_ARRAY, Some(&texture));
        // Five levels: down to 64-pixel sheets, where a tile is still 4 pixels.
        gl.tex_storage_3d(Gl::TEXTURE_2D_ARRAY, 5, Gl::RGBA8, SHEET_SIZE, SHEET_SIZE, pages as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D_ARRAY, Gl::TEXTURE_MIN_FILTER, Gl::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D_ARRAY, Gl::TEXTURE_MAG_FILTER, Gl::NEAREST as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D_ARRAY, Gl::TEXTURE_WRAP_S, Gl::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D_ARRAY, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);
        Some(AtlasLoader { gl: gl.clone(), texture })
    }

    /// Switches from flat colours to textures: every sheet has arrived.
    pub fn atlas_ready(&mut self, pages: u32) {
        self.atlas_pages = pages;
    }

    /// Replaces the mesh of one segment of one entity. An empty mesh removes it.
    pub fn set_chunk(&mut self, group: usize, key: (i32, i32, i32), mesh: &ChunkMesh) {
        while self.groups.len() <= group {
            self.groups.push(Group { placed: IN_PLACE, chunks: HashMap::new() });
        }
        if let Some(old) = self.groups[group].chunks.remove(&key) {
            self.free(old.opaque);
            self.free(old.translucent);
        }
        let chunk = Chunk { opaque: self.upload(&mesh.opaque), translucent: self.upload(&mesh.translucent) };
        if chunk.opaque.is_some() || chunk.translucent.is_some() {
            self.groups[group].chunks.insert(key, chunk);
        }
    }

    /// Says where an entity sits in the view. Cheap: nothing is uploaded, so
    /// it can be called every frame to move a docked entity.
    pub fn place_group(&mut self, group: usize, placed: Mat4) {
        if let Some(g) = self.groups.get_mut(group) {
            g.placed = placed;
        }
    }

    pub fn clear_chunks(&mut self) {
        for group in std::mem::take(&mut self.groups) {
            for (_, chunk) in group.chunks {
                self.free(chunk.opaque);
                self.free(chunk.translucent);
            }
        }
    }

    pub fn set_overlay(&mut self, overlay: &Overlay) {
        let (lines, fills) = (self.overlay_lines.take(), self.overlay_fills.take());
        self.free(lines);
        self.free(fills);
        self.overlay_lines = self.upload(&overlay.lines);
        self.overlay_fills = self.upload(&overlay.fills);
    }

    /// Replaces the marks drawn through the design. Systems sit inside hulls,
    /// so what a computer is linked to has to show through the blocks around it.
    pub fn set_marks(&mut self, marks: &Overlay) {
        let (lines, fills) = (self.mark_lines.take(), self.mark_fills.take());
        self.free(lines);
        self.free(fills);
        self.mark_lines = self.upload(&marks.lines);
        self.mark_fills = self.upload(&marks.fills);
    }

    /// Width over height of the canvas as shown.
    pub fn aspect(&self) -> f32 {
        self.canvas.client_width().max(1) as f32 / self.canvas.client_height().max(1) as f32
    }

    /// The design-space ray under a pointer position given in page coordinates.
    /// The middle of the view, in page coordinates.
    pub fn middle(&self) -> (i32, i32) {
        let rect = self.canvas.get_bounding_client_rect();
        ((rect.left() + rect.width() / 2.0) as i32, (rect.top() + rect.height() / 2.0) as i32)
    }

    pub fn pointer_ray(&self, client_x: i32, client_y: i32) -> ([f32; 3], [f32; 3]) {
        let rect = self.canvas.get_bounding_client_rect();
        let x = (client_x as f64 - rect.left()) / rect.width().max(1.0) * 2.0 - 1.0;
        let y = 1.0 - (client_y as f64 - rect.top()) / rect.height().max(1.0) * 2.0;
        self.camera.ray(x as f32, y as f32, self.aspect())
    }

    pub fn draw(&self) {
        // Match the drawing buffer to the element's size on screen.
        let ratio = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
        let width = ((self.canvas.client_width() as f64 * ratio) as u32).max(1);
        let height = ((self.canvas.client_height() as f64 * ratio) as u32).max(1);
        if self.canvas.width() != width || self.canvas.height() != height {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
        }
        let gl = &self.gl;
        gl.viewport(0, 0, width as i32, height as i32);
        gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        let view = self.camera.matrix(width as f32 / height as f32);
        let place = |placed: &Mat4| gl.uniform_matrix4fv_with_f32_array(Some(&self.mvp), false, &multiply(&view, placed));
        gl.uniform1f(Some(&self.pages), self.atlas_pages as f32);

        let draw = |b: &Buffers, mode: u32| {
            gl.bind_vertex_array(Some(&b.vao));
            gl.draw_elements_with_i32(mode, b.count, Gl::UNSIGNED_INT, 0);
        };

        // Solid blocks first.
        gl.enable(Gl::CULL_FACE);
        gl.disable(Gl::BLEND);
        gl.depth_mask(true);
        gl.uniform1f(Some(&self.edges), 1.0);
        for group in &self.groups {
            place(&group.placed);
            for chunk in group.chunks.values() {
                if let Some(b) = &chunk.opaque {
                    draw(b, Gl::TRIANGLES);
                }
            }
        }
        // Then everything see-through, blended over them without writing depth.
        gl.enable(Gl::BLEND);
        gl.depth_mask(false);
        for group in &self.groups {
            place(&group.placed);
            for chunk in group.chunks.values() {
                if let Some(b) = &chunk.translucent {
                    draw(b, Gl::TRIANGLES);
                }
            }
        }
        // Outlines and marks are given in the view's own coordinates.
        place(&IN_PLACE);
        gl.uniform1f(Some(&self.edges), 0.0);
        gl.disable(Gl::CULL_FACE);
        if let Some(b) = &self.overlay_fills {
            draw(b, Gl::TRIANGLES);
        }
        if let Some(b) = &self.overlay_lines {
            draw(b, Gl::LINES);
        }
        if self.mark_lines.is_some() || self.mark_fills.is_some() {
            gl.disable(Gl::DEPTH_TEST);
            if let Some(b) = &self.mark_fills {
                draw(b, Gl::TRIANGLES);
            }
            if let Some(b) = &self.mark_lines {
                draw(b, Gl::LINES);
            }
            gl.enable(Gl::DEPTH_TEST);
        }
        gl.depth_mask(true);
        gl.bind_vertex_array(None);
    }
}

fn compile(gl: &Gl, kind: u32, source: &str) -> Result<WebGlShader, String> {
    let shader = gl.create_shader(kind).ok_or("cannot create shader")?;
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);
    if gl.get_shader_parameter(&shader, Gl::COMPILE_STATUS).as_bool().unwrap_or(false) {
        Ok(shader)
    } else {
        Err(gl.get_shader_info_log(&shader).unwrap_or_else(|| "shader failed to compile".into()))
    }
}

fn link(gl: &Gl, vertex: &str, fragment: &str) -> Result<WebGlProgram, String> {
    let program = gl.create_program().ok_or("cannot create shader program")?;
    gl.attach_shader(&program, &compile(gl, Gl::VERTEX_SHADER, vertex)?);
    gl.attach_shader(&program, &compile(gl, Gl::FRAGMENT_SHADER, fragment)?);
    gl.link_program(&program);
    if gl.get_program_parameter(&program, Gl::LINK_STATUS).as_bool().unwrap_or(false) {
        Ok(program)
    } else {
        Err(gl.get_program_info_log(&program).unwrap_or_else(|| "shader program failed to link".into()))
    }
}
