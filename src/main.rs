mod backend;
mod config;
mod scale;
mod wallpaper;

use backend::{Backend, PauseMode, RunOptions};
use clap::Parser;

use scale::ScaleMode;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file
    #[arg(required_unless_present = "rand", conflicts_with = "rand")]
    path: Option<String>,

    /// Play a random wallpaper from your playlist
    #[arg(long, conflicts_with = "path")]
    rand: bool,

    /// How to fit the video to the screen.
    #[arg(long, value_enum, default_value_t = ScaleMode::Fill)]
    scale: ScaleMode,

    /// Pause playback while the system is on battery (macOS only)
    #[arg(long, conflicts_with = "pause_below")]
    pause_on_battery: bool,

    /// Pause playback when on battery and charge drops below PERCENT (1-100, macOS only)
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u8).range(1..=100))]
    pause_below: Option<u8>,
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

    let pause = match (args.pause_on_battery, args.pause_below) {
        (true, _) => PauseMode::OnBattery,
        (false, Some(pct)) => PauseMode::BelowPercent(pct),
        (false, None) => PauseMode::Never,
    };
    let options = RunOptions {
        pause,
        scale: args.scale,
    };

    #[cfg(target_os = "linux")]
    return backend::wayland::WaylandBackend::new()?.run(path, options);

    #[cfg(target_os = "macos")]
    return backend::macos::MacosBackend::new()?.run(path, options);
}
