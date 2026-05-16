mod decoder;
mod gl_renderer;

use std::{path::Path, sync::mpsc};

use anyhow::Context;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{wl_callback, wl_compositor, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use self::gl_renderer::GlRenderer;
use super::Backend;
use crate::scale::ScaleMode;

pub struct WaylandBackend {
    state: State,
    eq: EventQueue<State>,
    qh: QueueHandle<State>,
    renderer: GlRenderer,
}

impl WaylandBackend {
    pub fn new(_scale: ScaleMode) -> anyhow::Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();

        let mut state = State::new(conn);
        state.conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut state)
            .context("initial Wayland roundtrip")?;

        state.create_background_surface(&qh)?;
        eq.roundtrip(&mut state).context("create layer surface")?;
        state.wait_until_configured(&mut eq)?;

        let (width, height) = state.size();
        let renderer = GlRenderer::new(&state.conn, state.surface()?, width, height)?;

        Ok(Self {
            state,
            eq,
            qh,
            renderer,
        })
    }
}

impl Backend for WaylandBackend {
    fn run(mut self, video_path: String) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);

        let (gl_display, gl_context) =
            decoder::wrap_gl(self.renderer.egl_display(), self.renderer.egl_context())?;

        self.renderer.make_current()?;

        let decoder_gl_context = gl_context.clone();
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || {
                if let Err(e) = decoder::run(
                    Path::new(&video_path),
                    gl_display,
                    decoder_gl_context,
                    tx,
                ) {
                    log::error!("decoder error: {e:#}");
                }
            })?;

        loop {
            self.state.wait_for_frame_callback(&mut self.eq)?;

            let sample = rx.recv().context("receive decoded sample")?;
            let frame = decoder::sample_to_frame(sample, &gl_context)?;

            self.state.request_frame_callback(&self.qh);
            self.renderer.render(&frame)?;

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

struct State {
    conn: Connection,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    width: u32,
    height: u32,
    configured: bool,
    frame_callback_pending: bool,
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
}

impl State {
    fn new(conn: Connection) -> Self {
        Self {
            conn,
            compositor: None,
            layer_shell: None,
            width: 1,
            height: 1,
            configured: false,
            frame_callback_pending: false,
            surface: None,
            layer_surface: None,
        }
    }

    fn create_background_surface(&mut self, qh: &QueueHandle<Self>) -> anyhow::Result<()> {
        let compositor = self.compositor.as_ref().context("wl_compositor missing")?;
        let layer_shell = self
            .layer_shell
            .as_ref()
            .context("zwlr_layer_shell_v1 missing")?;

        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Background,
            "phonto".to_string(),
            qh,
            (),
        );

        layer_surface.set_size(0, 0);
        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        surface.commit();

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
        Ok(())
    }

    fn wait_until_configured(&mut self, event_queue: &mut EventQueue<Self>) -> anyhow::Result<()> {
        while !self.configured {
            event_queue
                .blocking_dispatch(self)
                .context("waiting for layer surface configure")?;
        }
        Ok(())
    }

    fn wait_for_frame_callback(
        &mut self,
        event_queue: &mut EventQueue<Self>,
    ) -> anyhow::Result<()> {
        while self.frame_callback_pending {
            event_queue
                .blocking_dispatch(self)
                .context("waiting for frame callback")?;
        }
        Ok(())
    }

    fn request_frame_callback(&mut self, qh: &QueueHandle<Self>) {
        self.surface
            .as_ref()
            .expect("wl_surface missing")
            .frame(qh, ());
        self.frame_callback_pending = true;
    }

    fn surface(&self) -> anyhow::Result<&wl_surface::WlSurface> {
        self.surface.as_ref().context("wl_surface missing")
    }

    fn size(&self) -> (u32, u32) {
        (self.width.max(1), self.height.max(1))
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer_surface.ack_configure(serial);
            if width > 0 && height > 0 {
                state.width = width;
                state.height = height;
            }
            state.configured = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_callback_pending = false;
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
