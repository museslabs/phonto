use std::{num::NonZeroU32, ptr::NonNull};

use anyhow::{Context, anyhow};
use glow::HasContext;
use glutin::{
    config::{Config, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext},
    display::{Display, DisplayApiPreference},
    prelude::{GlDisplay, NotCurrentGlContext},
    surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface},
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::{Proxy, protocol::wl_surface::WlSurface};

use crate::decoder;

pub struct GlRenderer {
    gl: glow::Context,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,

    program: glow::NativeProgram,
    vbo: glow::NativeBuffer,
    texture: glow::NativeTexture,
}

impl GlRenderer {
    const VERTEX_SHADER: &str = r#"
        attribute vec2 a_pos;
        varying vec2 v_uv;
        void main() {
            v_uv = vec2(a_pos.x * 0.5 + 0.5, 0.5 - a_pos.y * 0.5);
            gl_Position = vec4(a_pos, 0.0, 1.0);
        }
    "#;
    const FRAGMENT_SHADER: &str = r#"
        precision mediump float;
        uniform sampler2D u_tex;
        varying vec2 v_uv;
        void main() {
            gl_FragColor = texture2D(u_tex, v_uv);
        }
    "#;

    pub fn new(
        conn: &wayland_client::Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let gl_display = Self::create_display(conn)?;
        let gl_config = Self::create_config(&gl_display)?;
        let gl_surface = Self::create_surface(wl_surface, width, height, &gl_display, &gl_config)?;
        let gl_context = Self::create_context(&gl_display, &gl_config, &gl_surface)?;

        gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN))?;

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| {
                gl_display.get_proc_address(name).cast()
            })
        };

        unsafe {
            let vertex = Self::compile_shader(&gl, glow::VERTEX_SHADER, Self::VERTEX_SHADER)?;
            let fragment = Self::compile_shader(&gl, glow::FRAGMENT_SHADER, Self::FRAGMENT_SHADER)?;
            let program = Self::link_program(&gl, vertex, fragment)?;

            let vbo = gl.create_buffer().map_err(|e| anyhow::anyhow!(e))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            // Fullscreen quad as triangle strip: BL, BR, TL, TR
            let verts: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            let bytes = std::slice::from_raw_parts(verts.as_ptr().cast::<u8>(), verts.len() * 4);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);

            let texture = gl.create_texture().map_err(|e| anyhow::anyhow!(e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
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

            Ok(Self {
                gl,
                surface: gl_surface,
                context: gl_context,
                program,
                texture,
                vbo,
            })
        }
    }

    pub fn render(&self, frame: &decoder::Frame) -> anyhow::Result<()> {
        unsafe {
            self.gl.use_program(Some(self.program));

            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                frame.width as i32,
                frame.height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&frame.data)),
            );

            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let loc = self
                .gl
                .get_attrib_location(self.program, "a_pos")
                .context("a_pos not found")?;
            self.gl.enable_vertex_attrib_array(loc);
            self.gl
                .vertex_attrib_pointer_f32(loc, 2, glow::FLOAT, false, 8, 0);

            self.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            self.surface.swap_buffers(&self.context)?;
        }
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
            .with_api(glutin::config::Api::GLES2)
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

        unsafe { gl_display.create_window_surface(&gl_config, &gl_surface_attrs) }
            .context("create_window_surface")
    }

    fn create_context(
        gl_display: &Display,
        gl_config: &Config,
        gl_surface: &Surface<WindowSurface>,
    ) -> anyhow::Result<PossiblyCurrentContext> {
        let gl_context_attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(None);

        unsafe { gl_display.create_context(&gl_config, &gl_context_attrs) }
            .context("create_context")?
            .make_current(&gl_surface)
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
