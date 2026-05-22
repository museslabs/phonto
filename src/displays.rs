#[derive(Debug, Default, Clone)]
pub struct DisplayInfo {
    pub id: String,
    pub description: String,
    pub width: u32,
    pub height: u32,
}

pub fn list() -> anyhow::Result<Vec<DisplayInfo>> {
    use crate::backend::Backend;
    #[cfg(target_os = "macos")]
    {
        <crate::backend::macos::MacosBackend as Backend>::list_displays()
    }
    #[cfg(target_os = "linux")]
    {
        <crate::backend::wayland::WaylandBackend as Backend>::list_displays()
    }
}

pub fn print(displays: &[DisplayInfo]) {
    if displays.is_empty() {
        println!("no displays detected");
        return;
    }

    let id_w = displays
        .iter()
        .map(|d| d.id.len())
        .max()
        .unwrap_or(0)
        .max("ID".len());
    let desc_w = displays
        .iter()
        .map(|d| d.description.len())
        .max()
        .unwrap_or(0)
        .max("DESCRIPTION".len());

    println!("{:<id_w$}  {:<desc_w$}  RESOLUTION", "ID", "DESCRIPTION");
    for d in displays {
        println!(
            "{:<id_w$}  {:<desc_w$}  {}x{}",
            d.id, d.description, d.width, d.height
        );
    }

    let key = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "wayland"
    };
    println!();
    println!(
        "Use an ID as `{key} = \"…\"` in an [[alias]] entry, or directly as `id` in a [[display]] block."
    );
}
