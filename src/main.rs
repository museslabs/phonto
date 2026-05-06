use std::{path::Path, sync::mpsc};

use anyhow::Context;
use wayland_client::{Connection, EventQueue, QueueHandle};

mod decoder;
mod gl_renderer;
mod wayland;

struct Phonto {
    state: wayland::State,
    eq: EventQueue<wayland::State>,
    qh: QueueHandle<wayland::State>,
    renderer: gl_renderer::GlRenderer,
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

        let (width, height) = state.size();
        let renderer = gl_renderer::GlRenderer::new(&state.conn, state.surface()?, width, height)?;

        Ok(Self {
            state,
            eq,
            qh,
            renderer,
        })
    }

    fn play(&mut self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::sync_channel(2);
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
            let frame = rx.recv().context("receive decoded frame")?;
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

const WALLPAPER_PATH: &str = "/home/plo/dotfiles/wallpapers/animated/night-city.mp4";

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mut phonto = Phonto::new()?;
    phonto.play()
}
