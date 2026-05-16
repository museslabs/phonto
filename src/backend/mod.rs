pub trait Backend {
    /// Take ownership of the runtime. Blocks for the lifetime of the wallpaper —
    /// returns only on error or graceful shutdown.
    fn run(self: Box<Self>, video_path: String) -> anyhow::Result<()>;
}

mod wayland;

pub fn init() -> anyhow::Result<Box<dyn Backend>> {
    Ok(Box::new(wayland::WaylandBackend::new()?))
}
