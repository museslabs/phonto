use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::backend::{BackendOptions, PauseMode};
use crate::plan::{Request, Source, YtDlpOpts};
use crate::scale::ScaleMode;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
// Top-level args are only required for the "play a wallpaper" mode, not
// when a subcommand is invoked.
#[command(subcommand_negates_reqs = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub play: PlayArgs,

    // Kept outside `PlayArgs` because `dump` resolves YouTube URLs too.
    #[command(flatten)]
    pub yt_dlp: YtDlpArgs,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List the displays phonto detects on this system.
    Displays,

    /// Dump image from video input
    Dump(DumpArgs),

    /// Browse the wallpapers of the Pinakothiki gallery.
    #[command(subcommand)]
    Gallery(GalleryCommand),

    /// List the wallpapers that `phonto install` downloaded.
    List {
        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Download a wallpaper from the Pinakothiki gallery and keep it.
    Install {
        /// The wallpaper: its id, such as `koi`, `pinakothiki:koi`, or the
        /// URL of its page in the gallery.
        #[arg(value_name = "WALLPAPER")]
        wallpaper: String,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Remove a wallpaper that `phonto install` downloaded. Its settings
    /// stay, unless --remove-settings.
    Uninstall {
        /// The wallpaper: its id, such as `koi`, `pinakothiki:koi`, or the
        /// URL of its page in the gallery.
        #[arg(value_name = "WALLPAPER")]
        wallpaper: String,

        /// Remove the settings of the wallpaper too.
        #[arg(long)]
        remove_settings: bool,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Show or change the settings of a wallpaper from the gallery: the
    /// commands of its inputs and its actions. Without a command, it shows
    /// them.
    Settings(SettingsArgs),

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

impl Command {
    /// True if the command prints JSON, so its errors are JSON too.
    pub fn json(&self) -> bool {
        match self {
            Command::Gallery(command) => command.json(),
            Command::List { json } | Command::Install { json, .. } => *json,
            Command::Uninstall { json, .. } => *json,
            Command::Settings(args) => {
                args.json || args.command.as_ref().is_some_and(SettingsCommand::json)
            }
            _ => false,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum GalleryCommand {
    /// List the wallpapers of the gallery.
    List {
        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Find the wallpapers that have all the words in their id, title,
    /// author, summary, or tags. Case does not matter.
    Search {
        #[arg(required = true, value_name = "WORDS")]
        words: Vec<String>,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Show a wallpaper of the gallery, with its inputs and its actions.
    Show {
        /// The wallpaper: its id, such as `koi`, `pinakothiki:koi`, or the
        /// URL of its page in the gallery.
        #[arg(value_name = "WALLPAPER")]
        wallpaper: String,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },
}

impl GalleryCommand {
    fn json(&self) -> bool {
        match self {
            GalleryCommand::List { json }
            | GalleryCommand::Search { json, .. }
            | GalleryCommand::Show { json, .. } => *json,
        }
    }
}

#[derive(Args, Debug)]
pub struct SettingsArgs {
    /// The wallpaper: its id, such as `koi`, `pinakothiki:koi`, or the URL of
    /// its page in the gallery.
    #[arg(value_name = "WALLPAPER")]
    pub wallpaper: String,

    #[command(subcommand)]
    pub command: Option<SettingsCommand>,

    /// Print one JSON document.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum SettingsCommand {
    /// Open the settings in your editor: $VISUAL, else $EDITOR, else vi.
    Edit,

    /// Print the path of the settings file.
    Path {
        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Set the command of an input.
    SetInput {
        /// The name of the input.
        name: String,

        /// The shell command that gives the value.
        #[arg(
            long,
            required_unless_present = "suggested",
            conflicts_with = "suggested"
        )]
        command: Option<String>,

        /// Seconds between two runs. By default, the `every` of the suggested
        /// command, or 1.
        #[arg(long, requires = "command", conflicts_with = "suggested")]
        every: Option<f64>,

        /// Copy the command that the wallpaper suggests.
        #[arg(long)]
        suggested: bool,

        /// Which suggested command to copy, from 1, when there are several.
        #[arg(
            long,
            requires = "suggested",
            conflicts_with = "command",
            value_parser = clap::value_parser!(u32).range(1..)
        )]
        pick: Option<u32>,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Remove the command of an input from the settings.
    UnsetInput {
        /// The name of the input.
        name: String,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Set the label or the command of an action. What you leave out stays.
    SetAction {
        /// The name of the action.
        name: String,

        /// The text that the wallpaper shows for the action.
        #[arg(long)]
        label: Option<String>,

        /// The shell command that a click runs.
        #[arg(long)]
        command: Option<String>,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Remove an action from the settings.
    UnsetAction {
        /// The name of the action.
        name: String,

        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Delete the settings file.
    Reset {
        /// Print one JSON document.
        #[arg(long)]
        json: bool,
    },
}

impl SettingsCommand {
    pub fn json(&self) -> bool {
        match self {
            SettingsCommand::Edit => false,
            SettingsCommand::Path { json }
            | SettingsCommand::SetInput { json, .. }
            | SettingsCommand::UnsetInput { json, .. }
            | SettingsCommand::SetAction { json, .. }
            | SettingsCommand::UnsetAction { json, .. }
            | SettingsCommand::Reset { json } => *json,
        }
    }
}

// Flags for the default mode (no subcommand): playing a wallpaper.
#[derive(Args, Debug)]
pub struct PlayArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub render: RenderArgs,

    #[command(flatten)]
    pub power: PowerArgs,
}

// What to play. Converted into a `plan::Request`.
#[derive(Args, Debug)]
pub struct SourceArgs {
    /// Path to the video file, streaming URL (http, https, rtsp, rtmp),
    /// YouTube URL (resolved via yt-dlp), procedural wallpaper (a `.glsl`
    /// file, or a directory with a manifest.toml), or a wallpaper from the
    /// Pinakothiki gallery (`pinakothiki:<id>`, or the URL of its page).
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
}

impl From<SourceArgs> for Request {
    /// Precedence: per-display flags > positional path > `--rand` > config.
    fn from(args: SourceArgs) -> Self {
        if !args.display.is_empty() || !args.display_rand.is_empty() {
            return Request::PerDisplay {
                pinned: args
                    .display
                    .chunks(2)
                    .map(|c| (c[0].clone(), c[1].clone()))
                    .collect(),
                random: args.display_rand,
            };
        }
        match (args.path, args.rand) {
            (Some(path), _) => Request::Mirror(Source::Path(path)),
            (None, true) => Request::Mirror(Source::Random),
            (None, false) => Request::FromConfig,
        }
    }
}

// How and where frames are drawn.
#[derive(Args, Debug)]
pub struct RenderArgs {
    /// How to fit the video to the screen. Procedural wallpapers always fill it.
    #[arg(long, value_enum, default_value_t = ScaleMode::Fill)]
    pub scale: ScaleMode,

    /// Which layer shell layer to render on (Linux/Wayland only).
    #[cfg(target_os = "linux")]
    #[arg(long, value_enum, default_value_t = crate::backend::wayland::LayerMode::Background)]
    pub layer: crate::backend::wayland::LayerMode,

    /// Draw a procedural wallpaper across all displays as one scene, instead
    /// of one copy on each display.
    #[arg(long, conflicts_with_all = ["display", "display_rand"])]
    pub span: bool,

    /// Path to a GLSL fragment shader file to apply. The shader receives
    /// `u_tex` (sampler2D), `v_uv` (vec2), and optionally `u_resolution`
    /// (vec2, surface size in pixels).
    #[cfg(target_os = "linux")]
    #[arg(long, value_name = "PATH")]
    shader: Option<String>,
}

impl RenderArgs {
    pub fn backend_options(&self) -> BackendOptions {
        BackendOptions {
            #[cfg(target_os = "linux")]
            shader: self.shader.clone(),
        }
    }
}

// Battery-saving behaviour.
#[derive(Args, Debug)]
pub struct PowerArgs {
    /// Pause playback while the system is on battery
    #[arg(long, conflicts_with = "pause_below")]
    pause_on_battery: bool,

    /// Pause playback when on battery and charge drops below PERCENT (1-100)
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u8).range(1..=100))]
    pause_below: Option<u8>,
}

impl PowerArgs {
    pub fn pause_mode(&self) -> PauseMode {
        match (self.pause_on_battery, self.pause_below) {
            (true, _) => PauseMode::OnBattery,
            (false, Some(pct)) => PauseMode::BelowPercent(pct),
            (false, None) => PauseMode::Never,
        }
    }
}

#[derive(Args, Debug)]
pub struct YtDlpArgs {
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

impl From<YtDlpArgs> for YtDlpOpts {
    fn from(args: YtDlpArgs) -> Self {
        Self {
            format: args.yt_dlp_format,
            cookies_from_browser: args.cookies_from_browser,
            extra_args: args
                .yt_dlp_args
                .unwrap_or_default()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

#[derive(Args, Debug)]
pub struct DumpArgs {
    /// Video, URL, procedural wallpaper, or `pinakothiki:<id>` to take the
    /// frame from.
    #[arg()]
    pub path: String,

    #[arg(short, long, value_name = "OUT.png")]
    pub out: PathBuf,

    #[cfg(target_os = "linux")]
    #[arg(long, value_name = "PATH")]
    shader: Option<String>,

    #[arg(long, value_name = "SECONDS", default_value = "0.0", value_parser = parse_seek_seconds)]
    pub at: f64,

    /// Image size for procedural wallpapers (default 1920x1080). Videos keep
    /// their own size.
    #[arg(long, value_name = "WIDTHxHEIGHT", value_parser = parse_size)]
    pub size: Option<(u32, u32)>,
}

impl DumpArgs {
    /// `dump` reads its own `--shader`, not the top-level one.
    pub fn backend_options(&self) -> BackendOptions {
        BackendOptions {
            #[cfg(target_os = "linux")]
            shader: self.shader.clone(),
        }
    }
}

/// Parse a `--size` such as `2560x1440`.
fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("`{s}` is not WIDTHxHEIGHT"))?;
    let parse = |v: &str| match v.trim().parse::<u32>() {
        Ok(n) if (1..=16384).contains(&n) => Ok(n),
        _ => Err(format!("`{v}` is not a size between 1 and 16384")),
    };
    Ok((parse(w)?, parse(h)?))
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
