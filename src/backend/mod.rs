use crate::scale::ScaleMode;

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub pause: PauseMode,
    pub scale: ScaleMode,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum PauseMode {
    #[default]
    Never,
    OnBattery,
    BelowPercent(u8),
}

pub trait Backend {
    /// Take ownership of the runtime. Blocks for the lifetime of the wallpaper —
    /// returns only on error or graceful shutdown.
    fn run(self, video_path: String, options: RunOptions) -> anyhow::Result<()>;
}

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(target_os = "macos")]
pub mod macos;
