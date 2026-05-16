mod backend;
mod config;
mod wallpaper;

use backend::Backend;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file
    #[arg(required_unless_present = "rand", conflicts_with = "rand")]
    path: Option<String>,

    /// Play a random wallpaper from your playlist
    #[arg(long, conflicts_with = "path")]
    rand: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();

    let config = config::load()?;

    let path = if args.rand {
        wallpaper::pick_random(&config.search_paths)
            .ok_or_else(|| anyhow::anyhow!("no wallpapers found in configured search paths"))?
            .to_string_lossy()
            .into_owned()
    } else {
        args.path
            .expect("clap ensures path is set when --rand is not used")
    };

    #[cfg(target_os = "linux")]
    return backend::wayland::WaylandBackend::new()?.run(path);

    #[cfg(target_os = "macos")]
    return backend::macos::MacosBackend::new()?.run(path);
}
