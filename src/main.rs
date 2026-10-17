mod backend;
mod config;
mod displays;
mod plan;
mod scale;
mod wallpaper;

use std::path::PathBuf;

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

    /// Path to the video file, streaming URL (http, https, rtsp, rtmp),
    /// or YouTube URL (resolved via yt-dlp).
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

    /// Pass --cookies-from-browser <BROWSER> to yt-dlp for YouTube auth
    ///
    /// If YouTube is blocking you from accessing it, try using this option
    #[arg(long, value_name = "BROWSER")]
    cookies_from_browser: Option<String>,

    /// yt-dlp format selector (default: yt-dlp decides)
    #[arg(long, value_name = "FORMAT", default_value = None)]
    yt_dlp_format: Option<String>,

    /// Extra arguments forwarded to yt-dlp (e.g. `--yt-dlp-args "--no-check-certificate"`)
    #[arg(long, value_name = "ARGS")]
    yt_dlp_args: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the displays phonto detects on this system.
    Displays,

    /// Dump image from video input
    Dump {
        #[arg()]
        path: String,

        #[arg(short, long, value_name = "OUT.png")]
        out: PathBuf,

        #[cfg(target_os = "linux")]
        #[arg(long, value_name = "PATH")]
        shader: Option<String>,

        #[arg(long, value_name = "SECONDS", default_value = "0.0", value_parser = parse_seek_seconds)]
        at: f64,
    },

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

/// Parse a `--at` seek offset. Rejects negative and non-finite values, which
/// produce undefined/failed seeks in both the GStreamer and AVFoundation paths.
fn parse_seek_seconds(s: &str) -> Result<f64, String> {
    let secs: f64 = s
        .parse()
        .map_err(|_| format!("`{s}` is not a valid number"))?;
    if !secs.is_finite() {
        return Err("must be a finite number of seconds".to_string());
    }
    if secs < 0.0 {
        return Err("must be zero or positive".to_string());
    }
    Ok(secs)
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
            Command::Dump {
                path,
                at,
                out,
                #[cfg(target_os = "linux")]
                shader,
            } => {
                let config = config::load()?;
                let yt_dlp = plan::YtDlpOpts {
                    format: args.yt_dlp_format,
                    cookies_from_browser: args.cookies_from_browser,
                    extra_args: args
                        .yt_dlp_args
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect(),
                };

                let path =
                    plan::resolve_source(plan::Source::Path(path), &config.search_paths, &yt_dlp)?;

                let backend_options = backend::BackendOptions {
                    #[cfg(target_os = "linux")]
                    layer: args.layer,
                    #[cfg(target_os = "linux")]
                    shader,
                };
                return backend::create(backend_options)?.dump(path, at, out);
            }
            #[cfg(target_os = "macos")]
            Command::InstallLiveLockscreen {
                video,
                name,
                remove,
            } => return backend::macos::live_lockscreen::install::run(video, name, remove),
        }
    }

    let config = config::load()?;

    let request = if !args.display.is_empty() || !args.display_rand.is_empty() {
        plan::Request::PerDisplay {
            pinned: args
                .display
                .chunks(2)
                .map(|c| (c[0].clone(), c[1].clone()))
                .collect(),
            random: args.display_rand,
        }
    } else {
        match (args.path, args.rand) {
            (Some(path), _) => plan::Request::Mirror(plan::Source::Path(path)),
            (None, true) => plan::Request::Mirror(plan::Source::Random),
            (None, false) => plan::Request::FromConfig,
        }
    };

    let plan = plan::build(request, &config)?;

    let yt_dlp = plan::YtDlpOpts {
        format: args.yt_dlp_format,
        cookies_from_browser: args.cookies_from_browser,
        extra_args: args
            .yt_dlp_args
            .unwrap_or_default()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect(),
    };
    let playback = plan::resolve(plan, &config.search_paths, &yt_dlp)?;

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
    let backend_options = backend::BackendOptions {
        #[cfg(target_os = "linux")]
        layer: args.layer,
        #[cfg(target_os = "linux")]
        shader: args.shader,
    };
    backend::create(backend_options)?.run(playback, options)
}
