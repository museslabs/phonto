mod backend;
mod config;
mod displays;
#[cfg(target_os = "macos")]
mod macos_live_lockscreen;
mod plan;
mod scale;
mod wallpaper;

#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use anyhow::Context;

use backend::{Backend, PauseMode, RunOptions};
use clap::Parser;
use clap::Subcommand;

use scale::ScaleMode;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
// Top-level args are only required for the "play a wallpaper" mode, not
// when a subcommand is invoked.
#[command(subcommand_negates_reqs = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to the video file or a streaming URL (http, https, rtsp, rtmp)
    #[arg(conflicts_with_all = ["rand", "display", "display_rand"])]
    path: Option<String>,

    /// Play a random wallpaper from your playlist
    #[arg(long, conflicts_with_all = ["path", "display", "display_rand"])]
    rand: bool,

    /// Pin a video to a specific display. Repeatable.
    /// Pass the display ID exactly as `phonto displays` prints it, or an
    /// [[alias]].name from your config.
    #[arg(
        long,
        value_names = ["ID", "PATH"],
        num_args = 2,
        action = clap::ArgAction::Append,
        conflicts_with_all = ["path", "rand"],
    )]
    display: Vec<String>,

    /// Pick a random video for a specific display. Repeatable.
    #[arg(
        long = "display-rand",
        value_name = "ID",
        action = clap::ArgAction::Append,
        conflicts_with_all = ["path", "rand"],
    )]
    display_rand: Vec<String>,

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

#[derive(Subcommand, Debug)]
enum Command {
    /// List the displays phonto detects on this system.
    Displays,

    /// Transcode a video and register it as the macOS lock-screen
    /// wallpaper (HEVC Main10 + temporal sub-layers; survives multiple
    /// lock cycles).
    #[cfg(target_os = "macos")]
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

    if let Some(cmd) = args.command {
        match cmd {
            Command::Displays => {
                let detected = displays::list()?;
                displays::print(&detected);
                return Ok(());
            }
            #[cfg(target_os = "macos")]
            Command::InstallLiveLockscreen {
                video,
                name,
                remove,
            } => return macos_live_lockscreen::install::run(video, name, remove),
        }
    }

    let config = config::load()?;

    let cli_per_display = plan::CliPerDisplay {
        pinned: args
            .display
            .chunks(2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect(),
        random: args.display_rand.clone(),
    };

    let plan = plan::build(args.path, args.rand, cli_per_display, &config)?;
    let playback = plan::resolve(plan, &config.search_paths)?;

    // Persist the resolved path so other tools (e.g. hyprlock) can read it.
    // For per-display playback there's no single "current" path; skip the cache.
    if let plan::Playback::Mirror(ref path) = playback
        && let Ok(home) = std::env::var("HOME")
    {
        let cache_dir = std::path::Path::new(&home).join(".cache/phonto");
        if std::fs::create_dir_all(&cache_dir).is_ok() {
            let _ = std::fs::write(cache_dir.join("current"), path);
        }
    }

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
        backend::wayland::WaylandBackend::new(args.layer, shader)?.run(playback, options)
    }

    #[cfg(target_os = "macos")]
    return backend::macos::MacosBackend::new()?.run(playback, options);
}
