use std::{path::Path, sync::mpsc};

use anyhow::Context;
use wayland_client::{Connection, EventQueue, QueueHandle};

use crate::{decoder, gl_renderer, wayland};

pub struct Phonto {
    state: wayland::State,
    eq: EventQueue<wayland::State>,
    qh: QueueHandle<wayland::State>,
    renderer: gl_renderer::GlRenderer,
}

impl Phonto {
    pub fn new() -> anyhow::Result<Self> {
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

        let (width, height) = state.size();
        let renderer = gl_renderer::GlRenderer::new(&state.conn, state.surface()?, width, height)?;

        Ok(Self {
            state,
            eq,
            qh,
            renderer,
        })
    }

    pub fn play(&mut self, wallpaper_path: String) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);

        let (gl_display, gl_context) =
            decoder::wrap_gl(self.renderer.egl_display(), self.renderer.egl_context())?;

        self.renderer.make_current()?;

        let decoder_gl_context = gl_context.clone();
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || {
                if let Err(e) = decoder::run(
                    Path::new(&wallpaper_path),
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
