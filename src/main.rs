mod backend;

use backend::Backend;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file
    path: String,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();

    #[cfg(target_os = "linux")]
    return backend::wayland::WaylandBackend::new()?.run(args.path);

    #[cfg(target_os = "macos")]
    return backend::macos::MacosBackend::new()?.run(args.path);
}
