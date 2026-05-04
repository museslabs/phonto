use std::{num::NonZeroU32, ptr::NonNull};

use anyhow::Context;
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

pub struct GlRenderer {
    gl: glow::Context,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
}

impl GlRenderer {
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

        Ok(Self {
            gl,
            surface: gl_surface,
            context: gl_context,
        })
    }

    pub fn render(&self) -> anyhow::Result<()> {
        unsafe {
            self.gl.clear_color(255.0, 255.0, 255.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
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
}
