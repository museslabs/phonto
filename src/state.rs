//! Files phonto keeps under `~/.cache/phonto`.

use std::path::Path;

/// Persist the path of the wallpaper being played to `~/.cache/phonto/current`
/// so other tools (e.g. hyprlock) can read it. Best effort: errors are ignored.
pub fn write_current(path: &str) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let cache_dir = Path::new(&home).join(".cache/phonto");
    if std::fs::create_dir_all(&cache_dir).is_ok() {
        let _ = std::fs::write(cache_dir.join("current"), path);
    }
}
