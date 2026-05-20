use std::path::{Path, PathBuf};
use std::time::Duration;

use rand::seq::IndexedRandom;

use crate::{
    backend::PlaybackSource,
    config::{Config, Playlist, PlaylistEntry, SearchPath},
};

const WALLPAPER_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif", "ogv"];
const MIN_SHUFFLE_INTERVAL: Duration = Duration::from_secs(2);

pub enum SourceSelection<'a> {
    Path(&'a str),
    SearchPaths,
    Playlist(&'a str),
}

pub fn resolve_source(
    config: &Config,
    selection: SourceSelection<'_>,
    shuffle_every: Option<&str>,
) -> anyhow::Result<PlaybackSource> {
    match selection {
        SourceSelection::Path(path) => {
            if shuffle_every.is_some() {
                anyhow::bail!("--shuffle-every requires --rand or --playlist");
            }
            Ok(PlaybackSource::Single(PathBuf::from(path)))
        }
        SourceSelection::SearchPaths => {
            let pool = collect(&config.search_paths);
            source_from_pool(pool, shuffle_every)
        }
        SourceSelection::Playlist(name) => {
            let playlist = config
                .playlists
                .iter()
                .find(|p| p.name == name)
                .ok_or_else(|| anyhow::anyhow!("no playlist named '{name}' in config"))?;
            let pool = collect_playlist(playlist);
            source_from_pool(pool, shuffle_every)
        }
    }
}

pub fn write_current_if_single(source: &PlaybackSource) {
    let PlaybackSource::Single(path) = source else {
        return;
    };

    if let Ok(home) = std::env::var("HOME") {
        let cache_dir = Path::new(&home).join(".cache/phonto");
        if std::fs::create_dir_all(&cache_dir).is_ok() {
            let _ = std::fs::write(cache_dir.join("current"), path.to_string_lossy().as_bytes());
        }
    }
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

fn source_from_pool(
    pool: Vec<PathBuf>,
    shuffle_every: Option<&str>,
) -> anyhow::Result<PlaybackSource> {
    match shuffle_every {
        Some(spec) => {
            let interval = parse_duration(spec)?;
            if pool.is_empty() {
                anyhow::bail!("playback pool is empty");
            }
            if interval < MIN_SHUFFLE_INTERVAL {
                anyhow::bail!(
                    "--shuffle-every must be at least {}s to allow the next video to pre-roll",
                    MIN_SHUFFLE_INTERVAL.as_secs()
                );
            }
            Ok(PlaybackSource::Shuffle { pool, interval })
        }
        None => {
            let pick =
                pick_random(&pool).ok_or_else(|| anyhow::anyhow!("playback pool is empty"))?;
            Ok(PlaybackSource::Single(pick))
        }
    }
}

fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num_str, unit) = s.split_at(split);
    let n: u64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration number in '{s}'"))?;
    let secs = match unit.trim() {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        other => anyhow::bail!("unknown duration unit '{other}' (use s, m, or h)"),
    };
    if secs == 0 {
        anyhow::bail!("duration must be greater than zero");
    }
    Ok(Duration::from_secs(secs))
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
