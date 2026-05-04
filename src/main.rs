use std::{num::NonZeroU32, path::Path, ptr::NonNull, sync::mpsc};

use anyhow::Context;
use glow::HasContext;
use glutin::context::ContextAttributesBuilder;
use glutin::{
    config::{Api, ConfigTemplateBuilder},
    context::{ContextApi, Version},
    display::{Display, DisplayApiPreference},
    prelude::{GlDisplay, NotCurrentGlContext},
    surface::{GlSurface, SurfaceAttributesBuilder, SwapInterval, WindowSurface},
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};

mod decoder;
mod wayland;

struct Phonto {
    state: wayland::State,
    eq: EventQueue<wayland::State>,
    qh: QueueHandle<wayland::State>,
}

impl Phonto {
    fn new() -> anyhow::Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();

        let mut state = wayland::State::new(conn);
        state.conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut state)
            .context("initial Wayland roundtrip")?;

        state.create_background_surface(&qh)?;
        eq.roundtrip(&mut state).context("create layer surface")?;
        state.wait_until_configured(&mut eq)?;

        Ok(Self { state, eq, qh })
    }

    fn play(
        &mut self,
        gl: glow::Context,
        gl_surface: glutin::surface::Surface<WindowSurface>,
        gl_context: glutin::context::PossiblyCurrentContext,
    ) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        let (width, height) = self.state.size();

        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || {
                if let Err(e) = decoder::run(Path::new(&WALLPAPER_PATH), tx, width, height) {
                    log::error!("decoder error: {e:#}");
                }
            })?;

        loop {
            self.state.wait_for_frame_callback(&mut self.eq)?;

            let _frame = rx.recv().context("receive decoded frame")?;
            // println!("{:?}", frame);

            unsafe {
                gl.clear_color(255.0, 255.0, 255.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl_surface.swap_buffers(&gl_context)?;
            }

            self.state.request_frame_callback(&self.qh);

            self.eq
                .dispatch_pending(&mut self.state)
                .context("dispatch pending Wayland events")?;

            self.state
                .conn
                .flush()
                .context("flush Wayland connection")?;
        }
    }
}

const WALLPAPER_PATH: &str = "/home/plo/dotfiles/wallpapers/animated/night-city.mp4";

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mut phonto = Phonto::new()?;

    let (width, height) = phonto.state.size();

    let raw_display_handle = WaylandDisplayHandle::new(
        NonNull::new(phonto.state.conn.backend().display_ptr().cast())
            .context("null wl_display")?,
    );
    let gl_display = unsafe {
        Display::new(
            RawDisplayHandle::Wayland(raw_display_handle),
            DisplayApiPreference::Egl,
        )
    }
    .context("Display::new")?;

    let template = ConfigTemplateBuilder::new()
        .with_api(Api::GLES2)
        .with_alpha_size(8)
        .build();

    let gl_config = unsafe { gl_display.find_configs(template) }
        .context("find_configs")?
        .next()
        .context("no EGL config")?;

    let gl_context_attrs = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::Gles(Some(Version::new(2, 0))))
        .build(None);

    let raw_window_handle = WaylandWindowHandle::new(
        NonNull::new(phonto.state.surface()?.id().as_ptr().cast()).context("null wl_surface")?,
    );
    let gl_surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        RawWindowHandle::Wayland(raw_window_handle),
        NonZeroU32::new(width).context("zero width")?,
        NonZeroU32::new(height).context("zero height")?,
    );
    let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &gl_surface_attrs) }
        .context("create_window_surface")?;

    let gl_context = unsafe { gl_display.create_context(&gl_config, &gl_context_attrs) }
        .context("create_context")?
        .make_current(&gl_surface)
        .context("eglMakeCurrent")?;

    gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN))?;

    let gl = unsafe {
        glow::Context::from_loader_function_cstr(|name| gl_display.get_proc_address(name).cast())
    };

    phonto.play(gl, gl_surface, gl_context)
}
