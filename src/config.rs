use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

const DEFAULT_CONFIG: &str = "\
# phonto configuration
#
# `search_paths` is a list of directories scanned when running `phonto --rand`.
# Each entry has a `path` and a `depth` (0 = top-level only, 1 = one level of
# subdirectories, and so on). Uncomment and edit the examples below.
#
# [[search_paths]]
# path = \"/home/user/wallpapers\"
# depth = 1
#
# [[search_paths]]
# path = \"/mnt/media/videos\"
# depth = 2
#
# Named playlists let you group wallpapers by mood / context. Use them with
# `phonto --playlist <name>` (optionally combined with `--shuffle-every 10m`).
# Entries can mix directories (`path` + `depth`) and individual files (`file`).
#
# [[playlists]]
# name = \"chill\"
# entries = [
#   { path = \"/home/user/wallpapers/chill\", depth = 1 },
#   { file = \"/home/user/wallpapers/special.mp4\" },
# ]
";

#[derive(Debug, Deserialize)]
pub struct SearchPath {
    pub path: String,
    pub depth: u32,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PlaylistEntry {
    Dir { path: String, depth: u32 },
    File { file: String },
}

#[derive(Debug, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub search_paths: Vec<SearchPath>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

fn config_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from(".config"))
        })
        .join("phonto")
        .join("config.toml")
}

pub fn load() -> anyhow::Result<Config> {
    let path = config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        std::fs::write(&path, DEFAULT_CONFIG)
            .with_context(|| format!("failed to write default config to {}", path.display()))?;
        log::info!("created default config at {}", path.display());
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    toml::from_str(&contents).with_context(|| "failed to parse config")
}
