use std::path::PathBuf;

#[cfg(target_os = "linux")]
use anyhow::Context;

use crate::config::{ActionCommand, InputCommand, Location};
use crate::displays::DisplayInfo;
use crate::plan::Playback;
use crate::scale::ScaleMode;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub pause: PauseMode,
    pub scale: ScaleMode,
    #[cfg(target_os = "linux")]
    pub layer: wayland::LayerMode,
    /// Draw one procedural wallpaper across all outputs, as one desktop.
    pub span: bool,
    /// The windows that pause the wallpaper of each display.
    #[cfg(target_os = "macos")]
    pub pause_behind: crate::plan::PauseBehindPlan,
    /// Where the user is, for the sun of procedural wallpapers.
    pub location: Option<Location>,
    /// The `[[input]]` commands of the configuration.
    pub inputs: Vec<InputCommand>,
    /// The `[[action]]` commands of the configuration.
    pub actions: Vec<ActionCommand>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum PauseMode {
    #[default]
    Never,
    OnBattery,
    BelowPercent(u8),
}

/// What `phonto dump` writes.
#[derive(Debug)]
pub struct DumpRequest {
    pub path: String,
    /// Seconds into the video, or seconds of procedural playback.
    pub at: f64,
    pub out: PathBuf,
    /// The image size for procedural wallpapers. Videos keep their own size.
    pub size: Option<(u32, u32)>,
    /// Where the user is, for the sun of procedural wallpapers.
    pub location: Option<Location>,
    /// The `[[input]]` commands of the configuration.
    pub inputs: Vec<InputCommand>,
    /// The `[[action]]` entries of the configuration, for their labels.
    pub actions: Vec<ActionCommand>,
}

pub trait Backend {
    /// Enumerate displays this backend can target. Cheap, does not start the runtime.
    fn list_displays() -> anyhow::Result<Vec<DisplayInfo>>;

    /// Take ownership of the runtime. Blocks for the lifetime of the wallpaper —
    /// returns only on error or graceful shutdown.
    fn run(self, playback: Playback, options: RunOptions) -> anyhow::Result<()>;

    fn dump(self, request: DumpRequest) -> anyhow::Result<()>;
}

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(target_os = "macos")]
pub mod macos;

/// The backend for the platform phonto was built for.
#[cfg(target_os = "linux")]
pub type Platform = wayland::WaylandBackend;
#[cfg(target_os = "macos")]
pub type Platform = macos::MacosBackend;

/// Settings needed to construct the [`Platform`] backend. Only Wayland has
/// any; on macOS this is empty.
#[derive(Debug)]
pub struct BackendOptions {
    /// Path to a GLSL fragment shader file.
    #[cfg(target_os = "linux")]
    pub shader: Option<String>,
}

#[cfg(target_os = "linux")]
pub fn create(options: BackendOptions) -> anyhow::Result<Platform> {
    let shader = options
        .shader
        .as_deref()
        .map(|p| {
            std::fs::read_to_string(p).with_context(|| format!("failed to read shader file: {p}"))
        })
        .transpose()?;
    wayland::WaylandBackend::new(shader)
}

#[cfg(target_os = "macos")]
pub fn create(_: BackendOptions) -> anyhow::Result<Platform> {
    macos::MacosBackend::new()
}
