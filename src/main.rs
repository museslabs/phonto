mod backend;
mod config;
#[cfg(target_os = "macos")]
mod macos_live_lockscreen;
mod scale;
mod wallpaper;

#[cfg(target_os = "macos")]
use std::path::PathBuf;

use backend::{Backend, PauseMode, RunOptions};
use clap::Parser;
#[cfg(target_os = "macos")]
use clap::Subcommand;

use scale::ScaleMode;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
// Top-level args are only required for the "play a wallpaper" mode, not
// when a subcommand is invoked.
#[cfg_attr(target_os = "macos", command(subcommand_negates_reqs = true))]
struct Args {
    #[cfg(target_os = "macos")]
    #[command(subcommand)]
    command: Option<Command>,

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

#[cfg(target_os = "macos")]
#[derive(Subcommand, Debug)]
enum Command {
    /// Transcode a video and register it as the macOS lock-screen
    /// wallpaper (HEVC Main10 + temporal sub-layers; survives multiple
    /// lock cycles).
    InstallLiveLockscreen {
        /// Path to the video file (MP4/MOV).
        video: PathBuf,

        /// Display name for the picker (defaults to the file stem).
        #[arg(long)]
        name: Option<String>,

        /// Remove the previously-installed entry for this video and quit.
        #[arg(long)]
        remove: bool,
    },
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();

    #[cfg(target_os = "macos")]
    if let Some(cmd) = args.command {
        match cmd {
            Command::InstallLiveLockscreen {
                video,
                name,
                remove,
            } => return macos_live_lockscreen::install::run(video, name, remove),
        }
    }

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
