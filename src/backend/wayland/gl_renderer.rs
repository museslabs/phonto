use std::{num::NonZeroU32, ptr::NonNull};

use anyhow::{Context, anyhow};
use glow::HasContext;
use glutin::{
    config::{Config, ConfigTemplateBuilder},
    context::{
        AsRawContext, ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, RawContext,
        Version,
    },
    display::{AsRawDisplay, Display, DisplayApiPreference, RawDisplay as GlRawDisplay},
    prelude::{GlDisplay, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface},
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::{Proxy, protocol::wl_surface::WlSurface};

use super::decoder;
use crate::scale::ScaleMode;

/// The attribute location of the vertex positions of the fullscreen quad.
/// Every program binds `a_pos` here, so all programs share one quad.
const POSITION_ATTRIB: u32 = 0;

/// A crop that shows the whole texture.
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

pub struct GlRenderer {
    gl: glow::Context,
    egl_display: usize,
    egl_context: usize,
    display: Display,
    config: Config,
    context: PossiblyCurrentContext,
    program: glow::Program,
    scale_loc: glow::UniformLocation,
    /// `None` if a custom `--shader` does not read `v_uv`.
    flip_y_loc: Option<glow::UniformLocation>,
    /// `None` if a custom `--shader` does not read `v_uv`.
    crop_loc: Option<glow::UniformLocation>,
    resolution_loc: Option<glow::UniformLocation>,
    surfaces: Vec<Option<RenderSurface>>,
}

struct RenderSurface {
    surface: Surface<WindowSurface>,
    dims: (u32, u32),
}

/// A texture to draw to a surface, and its orientation.
#[derive(Debug, Clone, Copy)]
pub struct TextureRef {
    pub texture: glow::Texture,
    pub width: u32,
    pub height: u32,
    /// True for a texture that GL drew into (first row at the bottom). A
    /// decoded video frame has its first row at the top.
    pub flip_y: bool,
}

impl TextureRef {
    pub fn from_frame(frame: &decoder::Frame) -> anyhow::Result<Self> {
        Ok(Self {
            texture: glow::NativeTexture(
                NonZeroU32::new(frame.texture_id).context("zero GL texture id")?,
            ),
            width: frame.width,
            height: frame.height,
            flip_y: false,
        })
    }
}

impl GlRenderer {
    const VERTEX_SHADER: &str = r#"#version 300 es
        uniform vec2 u_scale;
        uniform float u_flip_y;
        // The part of the texture to draw: x, y, width, height, from the top left.
        uniform vec4 u_crop;
        layout(location = 0) in vec2 a_pos;
        out vec2 v_uv;
        void main() {
            vec2 uv = u_crop.xy + vec2(a_pos.x * 0.5 + 0.5, 0.5 - a_pos.y * 0.5) * u_crop.zw;
            v_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, u_flip_y));
            gl_Position = vec4(a_pos * u_scale, 0.0, 1.0);
        }
    "#;
    const FRAGMENT_SHADER: &str = r#"#version 300 es
        precision mediump float;
        uniform sampler2D u_tex;
        in vec2 v_uv;
        out vec4 frag_color;
        void main() {
            frag_color = texture(u_tex, v_uv);
        }
    "#;

    pub fn new(
        conn: &wayland_client::Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
        fragment_src: Option<&str>,
    ) -> anyhow::Result<Self> {
        let gl_display = Self::create_display(conn)?;
        let gl_config = Self::create_config(&gl_display)?;
        let gl_surface = Self::create_surface(wl_surface, width, height, &gl_display, &gl_config)?;
        let gl_context = Self::create_context(&gl_display, &gl_config, &gl_surface)?;

        let egl_display = match gl_display.raw_display() {
            GlRawDisplay::Egl(display) => display as usize,
            _ => return Err(anyhow!("expected EGL display")),
        };

        let egl_context = match gl_context.raw_context() {
            RawContext::Egl(ctx) => ctx as usize,
            _ => return Err(anyhow!("expected EGL context")),
        };

        gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN))?;

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| {
                gl_display.get_proc_address(name).cast()
            })
        };

        let frag_src = fragment_src.unwrap_or(Self::FRAGMENT_SHADER);

        let (program, scale_loc, flip_y_loc, crop_loc, resolution_loc) = unsafe {
            let vertex = Self::compile_shader(&gl, glow::VERTEX_SHADER, Self::VERTEX_SHADER)?;
            let fragment = Self::compile_shader(&gl, glow::FRAGMENT_SHADER, frag_src)?;
            let program = Self::link_program(&gl, vertex, fragment)?;

            gl.use_program(Some(program));

            let tex_loc = gl
                .get_uniform_location(program, "u_tex")
                .context("u_tex not found")?;
            gl.uniform_1_i32(Some(&tex_loc), 0);

            let scale_loc = gl
                .get_uniform_location(program, "u_scale")
                .context("u_scale not found")?;
            // Identity until the first frame tells us the source aspect.
            gl.uniform_2_f32(Some(&scale_loc), 1.0, 1.0);

            // It only changes v_uv. If a post-processing shader ignores v_uv,
            // the linker removes it.
            let flip_y_loc = gl.get_uniform_location(program, "u_flip_y");
            let crop_loc = gl.get_uniform_location(program, "u_crop");

            let resolution_loc = gl.get_uniform_location(program, "u_resolution");
            if let Some(ref loc) = resolution_loc {
                gl.uniform_2_f32(Some(loc), width as f32, height as f32);
            }

            let vbo = gl.create_buffer().map_err(|e| anyhow!(e))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            // Fullscreen quad as triangle strip: BL, BR, TL, TR
            let verts: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            let bytes = std::slice::from_raw_parts(verts.as_ptr().cast::<u8>(), verts.len() * 4);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);

            gl.enable_vertex_attrib_array(POSITION_ATTRIB);
            gl.vertex_attrib_pointer_f32(POSITION_ATTRIB, 2, glow::FLOAT, false, 8, 0);
            gl.active_texture(glow::TEXTURE0);

            // Black clear so letterbox bars stay black under Fit/Center.
            gl.clear_color(0.0, 0.0, 0.0, 1.0);

            (program, scale_loc, flip_y_loc, crop_loc, resolution_loc)
        };

        Ok(Self {
            gl,
            egl_display,
            egl_context,
            display: gl_display,
            config: gl_config,
            context: gl_context,
            program,
            scale_loc,
            flip_y_loc,
            crop_loc,
            resolution_loc,
            surfaces: vec![Some(RenderSurface {
                surface: gl_surface,
                dims: (width, height),
            })],
        })
    }

    pub fn add_surface(
        &mut self,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> anyhow::Result<usize> {
        let gl_surface =
            Self::create_surface(wl_surface, width, height, &self.display, &self.config)?;
        gl_surface.set_swap_interval(&self.context, SwapInterval::Wait(NonZeroU32::MIN))?;
        self.surfaces.push(Some(RenderSurface {
            surface: gl_surface,
            dims: (width, height),
        }));
        Ok(self.surfaces.len() - 1)
    }

    pub fn gl(&self) -> &glow::Context {
        &self.gl
    }

    pub fn egl_display(&self) -> usize {
        self.egl_display
    }

    pub fn egl_context(&self) -> usize {
        self.egl_context
    }

    pub fn surface_dims(&self, idx: usize) -> (u32, u32) {
        self.surfaces[idx]
            .as_ref()
            .expect("live render surface")
            .dims
    }

    pub fn set_surface_size(&mut self, idx: usize, width: u32, height: u32) {
        let s = self.surfaces[idx].as_mut().expect("live render surface");
        s.dims = (width, height);
        if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
            s.surface.resize(&self.context, w, h);
        }
    }

    pub fn remove_surface(&mut self, idx: usize) {
        if let Some(slot) = self.surfaces.get_mut(idx) {
            *slot = None;
        }
    }

    /// Makes the context current on surface `idx`, so GL calls draw into it.
    pub fn make_current(&self, idx: usize) -> anyhow::Result<()> {
        let s = self.surfaces[idx].as_ref().expect("live render surface");
        self.context
            .make_current(&s.surface)
            .context("make-current glutin context")
    }

    pub fn render(
        &self,
        idx: usize,
        frame: &decoder::Frame,
        mode: ScaleMode,
    ) -> anyhow::Result<()> {
        self.present(idx, TextureRef::from_frame(frame)?, mode)
    }

    /// Draws `texture` to surface `idx`, fitted by `mode`, through the
    /// post-processing shader if there is one. Then swaps the buffers.
    pub fn present(&self, idx: usize, texture: TextureRef, mode: ScaleMode) -> anyhow::Result<()> {
        let s = self.surfaces[idx].as_ref().expect("live render surface");
        let scale = Self::scale_for(mode, s.dims, (texture.width, texture.height));
        self.draw_to_surface(idx, texture, scale, WHOLE)
    }

    /// Draws the part `crop` of `texture` to fill surface `idx`. `crop` is x,
    /// y, width, and height, as fractions of the texture from the top left.
    /// A wallpaper that spans the desktop shows one part on each output.
    pub fn present_part(
        &self,
        idx: usize,
        texture: TextureRef,
        crop: [f32; 4],
    ) -> anyhow::Result<()> {
        self.draw_to_surface(idx, texture, (1.0, 1.0), crop)
    }

    fn draw_to_surface(
        &self,
        idx: usize,
        texture: TextureRef,
        scale: (f32, f32),
        crop: [f32; 4],
    ) -> anyhow::Result<()> {
        let s = self.surfaces[idx].as_ref().expect("live render surface");
        self.context
            .make_current(&s.surface)
            .context("make-current glutin context")?;
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.draw_texture(texture, s.dims, scale, crop);

        s.surface
            .swap_buffers(&self.context)
            .context("swap_buffers")?;

        Ok(())
    }

    /// Draws `texture` into an offscreen image of `width` x `height` pixels,
    /// stretched to fill it. Returns the RGBA rows from top to bottom.
    pub fn render_texture_to_rgba(
        &self,
        texture: TextureRef,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Vec<u8>> {
        if width == 0 || height == 0 {
            anyhow::bail!("cannot render a {width}x{height} image");
        }

        {
            let surface = self
                .surfaces
                .iter()
                .flatten()
                .next()
                .context("no live render surface")?;
            self.context
                .make_current(&surface.surface)
                .context("make-current glutin context")?;
        }

        let fbo = unsafe { self.gl.create_framebuffer().map_err(|e| anyhow!(e))? };
        let color = unsafe { self.gl.create_texture().map_err(|e| anyhow!(e))? };

        let result = {
            unsafe {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                self.gl.bind_texture(glow::TEXTURE_2D, Some(color));
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width as i32,
                    height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                self.gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(color),
                    0,
                );

                if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE
                {
                    anyhow::bail!("offscreen framebuffer is incomplete");
                }
            }

            self.draw_texture(texture, (width, height), (1.0, 1.0), WHOLE);

            let mut bottom_up = vec![0u8; width as usize * height as usize * 4];
            unsafe {
                self.gl.finish();
                self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                self.gl.read_pixels(
                    0,
                    0,
                    width as i32,
                    height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut bottom_up)),
                );
            }

            Ok(flip_vertical(&bottom_up, width as usize, height as usize))
        };

        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.delete_texture(color);
            self.gl.delete_framebuffer(fbo);
        }

        result
    }

    /// Draws `texture` into the bound framebuffer. It binds all the state that
    /// it uses, because the procedural passes share the context and change
    /// that state.
    fn draw_texture(
        &self,
        texture: TextureRef,
        (width, height): (u32, u32),
        scale: (f32, f32),
        [x, y, w, h]: [f32; 4],
    ) {
        unsafe {
            self.gl.use_program(Some(self.program));
            self.gl.viewport(0, 0, width as i32, height as i32);
            self.gl
                .uniform_2_f32(Some(&self.scale_loc), scale.0, scale.1);
            self.gl.uniform_4_f32(self.crop_loc.as_ref(), x, y, w, h);
            self.gl.uniform_1_f32(
                self.flip_y_loc.as_ref(),
                if texture.flip_y { 1.0 } else { 0.0 },
            );
            if let Some(ref loc) = self.resolution_loc {
                self.gl
                    .uniform_2_f32(Some(loc), width as f32, height as f32);
            }
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_sampler(0, None);
            self.gl
                .bind_texture(glow::TEXTURE_2D, Some(texture.texture));
            self.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    fn scale_for(mode: ScaleMode, screen_dims: (u32, u32), video_dims: (u32, u32)) -> (f32, f32) {
        let (screen_w, screen_h) = screen_dims;
        let (video_w, video_h) = video_dims;
        if screen_w == 0 || screen_h == 0 || video_w == 0 || video_h == 0 {
            return (1.0, 1.0);
        }

        let screen_aspect = screen_w as f32 / screen_h as f32;
        let video_aspect = video_w as f32 / video_h as f32;
        match mode {
            ScaleMode::Stretch => (1.0, 1.0),
            ScaleMode::Fit => {
                if video_aspect > screen_aspect {
                    (1.0, screen_aspect / video_aspect)
                } else {
                    (video_aspect / screen_aspect, 1.0)
                }
            }
            ScaleMode::Fill => {
                if video_aspect > screen_aspect {
                    (video_aspect / screen_aspect, 1.0)
                } else {
                    (1.0, screen_aspect / video_aspect)
                }
            }
            ScaleMode::Center => (
                video_w as f32 / screen_w as f32,
                video_h as f32 / screen_h as f32,
            ),
        }
    }
}

fn flip_vertical(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = width * 4;
    let mut out = vec![0u8; src.len()];

    for y in 0..height {
        let src_start = y * stride;
        let dst_start = (height - 1 - y) * stride;
        out[dst_start..dst_start + stride].copy_from_slice(&src[src_start..src_start + stride]);
    }

    out
}

impl GlRenderer {
    fn create_display(conn: &wayland_client::Connection) -> anyhow::Result<Display> {
        unsafe {
            Display::new(
                RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                    NonNull::new(conn.backend().display_ptr().cast()).context("null wl_display")?,
                )),
                DisplayApiPreference::Egl,
            )
        }
        .context("Display::new")
    }

    fn create_config(gl_display: &Display) -> anyhow::Result<Config> {
        let template = ConfigTemplateBuilder::new()
            .with_api(glutin::config::Api::GLES3)
            .with_alpha_size(8)
            .build();

        unsafe { gl_display.find_configs(template) }
            .context("find_configs")?
            .next()
            .context("no EGL config")
    }

    fn create_surface(
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
        gl_display: &Display,
        gl_config: &Config,
    ) -> anyhow::Result<Surface<WindowSurface>> {
        let raw_window_handle = WaylandWindowHandle::new(
            NonNull::new(wl_surface.id().as_ptr().cast()).context("null wl_surface")?,
        );

        let gl_surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            RawWindowHandle::Wayland(raw_window_handle),
            NonZeroU32::new(width).context("zero width")?,
            NonZeroU32::new(height).context("zero height")?,
        );

        unsafe { gl_display.create_window_surface(gl_config, &gl_surface_attrs) }
            .context("create_window_surface")
    }

    fn create_context(
        gl_display: &Display,
        gl_config: &Config,
        gl_surface: &Surface<WindowSurface>,
    ) -> anyhow::Result<PossiblyCurrentContext> {
        let gl_context_attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
            .build(None);

        unsafe { gl_display.create_context(gl_config, &gl_context_attrs) }
            .context("create_context")?
            .make_current(gl_surface)
            .context("eglMakeCurrent")
    }

    pub fn compile_shader(
        gl: &glow::Context,
        ty: u32,
        source: &str,
    ) -> anyhow::Result<glow::Shader> {
        unsafe {
            let shader = gl.create_shader(ty).map_err(|e| anyhow!(e))?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(anyhow!(
                    "shader compile failed: {}",
                    gl.get_shader_info_log(shader)
                ));
            }
            Ok(shader)
        }
    }

    pub fn link_program(
        gl: &glow::Context,
        vertex: glow::Shader,
        fragment: glow::Shader,
    ) -> anyhow::Result<glow::Program> {
        unsafe {
            let program = gl.create_program().map_err(|e| anyhow!(e))?;
            gl.attach_shader(program, vertex);
            gl.attach_shader(program, fragment);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(anyhow!(
                    "shader link failed: {}",
                    gl.get_program_info_log(program)
                ));
            }
            gl.delete_shader(vertex);
            gl.delete_shader(fragment);
            Ok(program)
        }
    }
}
