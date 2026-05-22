use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

const DEFAULT_CONFIG: &str = "\
# phonto configuration
#
# search_paths: directories scanned by --rand and by per-display `random = true`.
# Each entry has a path and a depth (0 = top-level only).
#
# [[search_paths]]
# path = \"/home/user/wallpapers\"
# depth = 1
#
# alias: portable names for displays across operating systems. Use the alias
# name in [[display]].id and `phonto displays` will tell you the per-OS strings
# to put here.
#
# [[alias]]
# name = \"main\"
# wayland = \"DP-1\"
# macos = \"DELL U2723QE\"
#
# display: pin a video (or a random pick) to a specific display. `id` matches
# an [[alias]].name OR a raw native ID as shown by `phonto displays`. Exactly
# one of `path` or `random = true` per entry.
#
# [[display]]
# id = \"main\"
# path = \"/path/to/wallpaper.mp4\"
#
# [[display]]
# id = \"laptop\"
# random = true
";

#[derive(Debug, Deserialize)]
pub struct SearchPath {
    pub path: String,
    pub depth: u32,
}

#[derive(Debug, Deserialize)]
pub struct Alias {
    pub name: String,
    #[serde(default)]
    pub wayland: Option<String>,
    #[serde(default)]
    pub macos: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Display {
    pub id: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub random: bool,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub search_paths: Vec<SearchPath>,
    #[serde(default)]
    pub alias: Vec<Alias>,
    #[serde(default)]
    pub display: Vec<Display>,
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
