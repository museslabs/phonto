pub trait Backend {
    /// Take ownership of the runtime. Blocks for the lifetime of the wallpaper —
    /// returns only on error or graceful shutdown.
    fn run(self, video_path: String) -> anyhow::Result<()>;
}

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(target_os = "macos")]
pub mod macos;
