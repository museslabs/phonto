use std::path::{Path, PathBuf};

use rand::seq::IndexedRandom;

use crate::config::SearchPath;

const WALLPAPER_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif", "ogv"];

pub fn collect(search_paths: &[SearchPath]) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    for sp in search_paths {
        let expanded = crate::config::expand_tilde(&sp.path);
        walk(Path::new(&expanded), sp.depth, 0, &mut wallpapers);
    }
    wallpapers
}

pub fn pick_random(search_paths: &[SearchPath]) -> Option<PathBuf> {
    let wallpapers = collect(search_paths);
    wallpapers.choose(&mut rand::rng()).cloned()
}

fn walk(dir: &Path, max_depth: u32, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && depth < max_depth {
            walk(&path, max_depth, depth + 1, out);
        } else if path.is_file() && is_wallpaper(&path) {
            out.push(path);
        }
    }
}

fn is_wallpaper(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| WALLPAPER_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}
