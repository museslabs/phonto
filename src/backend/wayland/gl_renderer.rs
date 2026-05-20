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

pub struct GlRenderer {
    gl: glow::Context,
    egl_display: usize,
    egl_context: usize,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    scale_loc: glow::UniformLocation,
    resolution_loc: Option<glow::UniformLocation>,
    surface_dims: (u32, u32),
}

impl GlRenderer {
    const VERTEX_SHADER: &str = r#"#version 300 es
        uniform vec2 u_scale;
        in vec2 a_pos;
        out vec2 v_uv;
        void main() {
            v_uv = vec2(a_pos.x * 0.5 + 0.5, 0.5 - a_pos.y * 0.5);
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

        unsafe {
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

            let pos_loc = gl
                .get_attrib_location(program, "a_pos")
                .context("a_pos not found")?;

            gl.enable_vertex_attrib_array(pos_loc);
            gl.vertex_attrib_pointer_f32(pos_loc, 2, glow::FLOAT, false, 8, 0);
            gl.active_texture(glow::TEXTURE0);

            // Black clear so letterbox bars stay black under Fit/Center.
            gl.clear_color(0.0, 0.0, 0.0, 1.0);

            Ok(Self {
                gl,
                egl_display,
                egl_context,
                surface: gl_surface,
                context: gl_context,
                scale_loc,
                resolution_loc,
                surface_dims: (width, height),
            })
        }
    }

    pub fn surface_dims(&self) -> (u32, u32) {
        self.surface_dims
    }

    pub fn set_surface_size(&mut self, width: u32, height: u32) {
        self.surface_dims = (width, height);
        if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
            self.surface.resize(&self.context, w, h);
        }
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
            if let Some(ref loc) = self.resolution_loc {
                self.gl
                    .uniform_2_f32(Some(loc), width as f32, height as f32);
            }
        }
    }

    /// Recompute the on-screen quad scale for the given video pixel dimensions.
    /// Call this when the source dimensions are first known (or change).
    pub fn set_video_dimensions(&self, video_w: u32, video_h: u32, mode: ScaleMode) {
        let (screen_w, screen_h) = self.surface_dims;
        let scale = if screen_w == 0 || screen_h == 0 || video_w == 0 || video_h == 0 {
            (1.0, 1.0)
        } else {
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
        };
        unsafe {
            self.gl
                .uniform_2_f32(Some(&self.scale_loc), scale.0, scale.1);
        }
    }

    pub fn egl_display(&self) -> usize {
        self.egl_display
    }

    pub fn egl_context(&self) -> usize {
        self.egl_context
    }

    pub fn make_current(&self) -> anyhow::Result<()> {
        self.context
            .make_current(&self.surface)
            .context("make-current glutin context")
    }

    pub fn render(&self, frame: &decoder::Frame) -> anyhow::Result<()> {
        let texture =
            glow::NativeTexture(NonZeroU32::new(frame.texture_id).context("zero GL texture id")?);

        unsafe {
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }

        self.surface
            .swap_buffers(&self.context)
            .context("swap_buffers")?;

        Ok(())
    }
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

    fn compile_shader(gl: &glow::Context, ty: u32, source: &str) -> anyhow::Result<glow::Shader> {
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

    fn link_program(
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
