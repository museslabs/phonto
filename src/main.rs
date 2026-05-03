use std::{path::Path, sync::mpsc};

use anyhow::Context;
use wayland_client::Connection;

mod decoder;
mod wayland;

const WALLPAPER_PATH: &str = "/home/plo/dotfiles/wallpapers/animated/blue-porsche.mp4";

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let (tx, rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("decoder".into())
        .spawn(move || {
            if let Err(e) = decoder::run(Path::new(&WALLPAPER_PATH), tx) {
                log::error!("decoder error: {e:#}");
            }
        })?;

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

    loop {
        state.wait_for_frame_callback(&mut eq)?;

        let frame = rx.recv().context("receive decoded frame")?;
        println!("{:?}", frame);

        state.request_frame_callback(&qh);

        eq.dispatch_pending(&mut state)
            .context("dispatch pending Wayland events")?;

        state.conn.flush().context("flush Wayland connection")?;
    }
}
