mod backend;
mod config;
#[cfg(target_os = "macos")]
mod macos_live_lockscreen;
mod scale;
mod wallpaper;

#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use anyhow::Context;
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
    #[arg(
        required_unless_present_any = ["rand", "playlist"],
        conflicts_with_all = ["rand", "playlist"],
    )]
    path: Option<String>,

    /// Play from your `search_paths` pool
    #[arg(long, conflicts_with = "playlist")]
    rand: bool,

    /// Play from a named playlist defined in config
    #[arg(long, value_name = "NAME")]
    playlist: Option<String>,

    /// Swap wallpapers from the pool at this interval (e.g. "30s", "10m", "1h").
    /// Requires --rand or --playlist.
    #[arg(long, value_name = "DURATION")]
    shuffle_every: Option<String>,

    /// How to fit the video to the screen.
    #[arg(long, value_enum, default_value_t = ScaleMode::Fill)]
    scale: ScaleMode,

    /// Which layer shell layer to render on (Linux/Wayland only).
    #[cfg(target_os = "linux")]
    #[arg(long, value_enum, default_value_t = backend::wayland::LayerMode::Background)]
    layer: backend::wayland::LayerMode,

    /// Path to a GLSL fragment shader file to apply. The shader receives
    /// `u_tex` (sampler2D), `v_uv` (vec2), and optionally `u_resolution`
    /// (vec2, surface size in pixels).
    #[cfg(target_os = "linux")]
    #[arg(long, value_name = "PATH")]
    shader: Option<String>,

    /// Pause playback while the system is on battery
    #[arg(long, conflicts_with = "pause_below")]
    pause_on_battery: bool,

    /// Pause playback when on battery and charge drops below PERCENT (1-100)
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

    let selection = if args.rand {
        wallpaper::SourceSelection::SearchPaths
    } else if let Some(name) = args.playlist.as_deref() {
        wallpaper::SourceSelection::Playlist(name)
    } else {
        wallpaper::SourceSelection::Path(
            args.path
                .as_deref()
                .expect("clap requires a path when no pool is selected"),
        )
    };

    let source = wallpaper::resolve_source(&config, selection, args.shuffle_every.as_deref())?;
    wallpaper::write_current_if_single(&source);

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
    {
        let shader = args
            .shader
            .as_deref()
            .map(|p| {
                std::fs::read_to_string(p)
                    .with_context(|| format!("failed to read shader file: {p}"))
            })
            .transpose()?;
        backend::wayland::WaylandBackend::new(args.layer, shader)?.run(source, options)
    }

    #[cfg(target_os = "macos")]
    return backend::macos::MacosBackend::new()?.run(source, options);
}
