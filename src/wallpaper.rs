use std::path::{Path, PathBuf};

use rand::seq::IndexedRandom;

use crate::{
    backend::PlaybackSource,
    config::{Config, Playlist, PlaylistEntry, SearchPath},
};

const WALLPAPER_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif", "ogv"];

pub enum SourceSelection<'a> {
    Path(&'a str),
    SearchPaths,
    Playlist(&'a str),
}

pub fn resolve_source(
    config: &Config,
    selection: SourceSelection<'_>,
) -> anyhow::Result<PlaybackSource> {
    let pick = match selection {
        SourceSelection::Path(path) => PathBuf::from(path),
        SourceSelection::SearchPaths => {
            let pool = collect(&config.search_paths);
            pick_random(&pool).ok_or_else(|| {
                anyhow::anyhow!("no wallpapers found in configured search paths")
            })?
        }
        SourceSelection::Playlist(name) => {
            let playlist = config
                .playlists
                .iter()
                .find(|p| p.name == name)
                .ok_or_else(|| anyhow::anyhow!("no playlist named '{name}' in config"))?;
            let pool = collect_playlist(playlist);
            pick_random(&pool)
                .ok_or_else(|| anyhow::anyhow!("playlist '{name}' has no playable entries"))?
        }
    };
    Ok(PlaybackSource::Single(pick))
}

pub fn collect(search_paths: &[SearchPath]) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    for sp in search_paths {
        walk(Path::new(&sp.path), sp.depth, 0, &mut wallpapers);
    }
    wallpapers
}

pub fn collect_playlist(playlist: &Playlist) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    for entry in &playlist.entries {
        match entry {
            PlaylistEntry::Dir { path, depth } => walk(Path::new(path), *depth, 0, &mut wallpapers),
            PlaylistEntry::File { file } => {
                let p = PathBuf::from(file);
                if p.is_file() {
                    wallpapers.push(p);
                } else {
                    log::warn!(
                        "playlist '{}' references missing file: {file}",
                        playlist.name
                    );
                }
            }
        }
    }
    wallpapers
}

pub fn pick_random(pool: &[PathBuf]) -> Option<PathBuf> {
    pool.choose(&mut rand::rng()).cloned()
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
