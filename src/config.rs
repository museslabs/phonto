use std::path::PathBuf;

use anyhow::{Context, bail};
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
# one of `path` or `random = true` per entry, unless it only sets
# `pause_behind`.
#
# [[display]]
# id = \"main\"
# path = \"/path/to/wallpaper.mp4\"
#
# [[display]]
# id = \"laptop\"
# random = true
#
# pause_behind: the windows that pause the wallpaper while they cover it, on
# battery and on power (macOS only). `any` pauses behind any windows over 95%
# of the screen, translucent ones too. `opaque` pauses behind opaque windows
# over the middle of the screen. `none` never pauses for windows. A hidden
# wallpaper, as on the lock screen, pauses in every mode. Both keys default
# to `any`.
#
# [pause_behind]
# on_battery = \"any\"
# on_power = \"opaque\"
#
# A display overrides it in its [[display]] entry, which holds everything
# about one display and needs no wallpaper. A key it leaves out comes from
# [pause_behind]:
#
# [[display]]
# id = \"LG HDR 4K\"
# pause_behind = { on_power = \"none\" }
#
# location: where you are, for the sunrise and sunset that procedural
# wallpapers get in iSun. Degrees, with north and east positive. phonto does
# not send it anywhere.
#
# [location]
# latitude = 37.98
# longitude = 23.73
#
# input: a command that gives procedural wallpapers a value. phonto runs it
# with sh every `every` seconds, while a wallpaper that reads the input plays.
# The command prints one to four numbers. The wallpaper reads iInput.<name>.
#
# [[input]]
# name = \"workspace\"
# command = \"hyprctl activeworkspace -j | jq .id\"
# every = 1
#
# action: a command that a click on a procedural wallpaper runs. The wallpaper
# names its actions. A click on an action runs the command with the same name,
# with sh. A wallpaper can also show the label, for example as a menu.
#
# [[action]]
# name = \"tape1\"
# label = \"Firefox\"
# command = \"firefox\"
";

#[derive(Debug, Deserialize)]
pub struct SearchPath {
    pub path: String,
    pub depth: u32,
}

#[derive(Debug, Deserialize)]
pub struct Alias {
    pub name: String,
    #[cfg(target_os = "linux")]
    #[serde(default)]
    pub wayland: Option<String>,
    #[cfg(target_os = "macos")]
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
    /// Overrides `[pause_behind]` for this display.
    #[serde(default)]
    pub pause_behind: Option<PauseBehind>,
}

impl Display {
    /// True if the entry only sets `pause_behind`. It plays nothing.
    pub fn only_pause_behind(&self) -> bool {
        self.path.is_none() && !self.random && self.pause_behind.is_some()
    }
}

/// `[pause_behind]`, or the `pause_behind` of a `[[display]]`: the windows
/// that pause the wallpaper while they cover it. Only macOS reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub struct PauseBehind {
    #[serde(default)]
    pub on_battery: Option<Behind>,
    #[serde(default)]
    pub on_power: Option<Behind>,
}

#[cfg(target_os = "macos")]
impl PauseBehind {
    /// These keys, with the ones that are left out taken from `table`.
    pub fn or(self, table: PauseBehind) -> PauseBehind {
        PauseBehind {
            on_battery: self.on_battery.or(table.on_battery),
            on_power: self.on_power.or(table.on_power),
        }
    }

    /// The mode for the power source. A key that is left out means `any`.
    pub fn mode(self, on_battery: bool) -> Behind {
        let mode = if on_battery {
            self.on_battery
        } else {
            self.on_power
        };
        mode.unwrap_or(Behind::Any)
    }
}

/// The windows that pause a wallpaper. A hidden wallpaper pauses in every
/// mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Behind {
    /// Any windows over 95% of the screen, translucent or not.
    Any,
    /// Opaque windows over the middle of the screen.
    Opaque,
    /// No windows.
    None,
}

/// A place on Earth, in degrees. North and east are positive.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
}

/// A command that gives procedural wallpapers the value of an input.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputCommand {
    pub name: String,
    /// A shell command that prints one to four numbers.
    pub command: String,
    /// Seconds between two runs.
    pub every: f64,
}

/// A command that a click on a procedural wallpaper runs, and the label that
/// the wallpaper can show for it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionCommand {
    /// The name of an action of the wallpaper.
    pub name: String,
    /// A shell command. Without it, a click does nothing.
    #[serde(default)]
    pub command: Option<String>,
    /// The text that the wallpaper shows for the action.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub search_paths: Vec<SearchPath>,
    #[serde(default)]
    pub alias: Vec<Alias>,
    #[serde(default)]
    pub display: Vec<Display>,
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default, rename = "input")]
    pub inputs: Vec<InputCommand>,
    #[serde(default, rename = "action")]
    pub actions: Vec<ActionCommand>,
    #[serde(default)]
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub pause_behind: PauseBehind,
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

pub fn is_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("rtmp://")
        || s.starts_with("rtsp://")
        || s.starts_with("file://")
}

/// Returns `Some("https://...")` if `s` looks like a YouTube URL (with or
/// without a protocol prefix). Returns `None` for non-YouTube inputs.
pub fn maybe_youtube_url(s: &str) -> Option<String> {
    let s = s.trim();
    let stripped = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);

    let is_yt = stripped.starts_with("www.youtube.com/")
        || stripped.starts_with("youtube.com/")
        || stripped.starts_with("youtu.be/")
        || stripped.starts_with("m.youtube.com/");

    if is_yt {
        if s.starts_with("http") {
            Some(s.to_string())
        } else {
            Some(format!("https://{stripped}"))
        }
    } else {
        None
    }
}

/// Expands a leading `~/` or `~` to `$HOME`. Leaves all other paths untouched.
/// Lets the same `~/Downloads/wall.mp4` work on macOS and Linux despite the
/// different home prefixes.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    if path == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return home;
    }
    path.to_string()
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
    parse(&contents)
}

fn parse(contents: &str) -> anyhow::Result<Config> {
    let config: Config = toml::from_str(contents).with_context(|| "failed to parse config")?;
    for input in &config.inputs {
        if !(input.every.is_finite() && input.every > 0.0) {
            bail!(
                "[[input]] `{}`: `every` must be a number of seconds above 0 (got {})",
                input.name,
                input.every
            );
        }
    }
    for (i, action) in config.actions.iter().enumerate() {
        if config.actions[..i].iter().any(|a| a.name == action.name) {
            bail!("two [[action]] entries are named `{}`", action.name);
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_intervals_must_be_finite_and_positive() {
        let config = |every: &str| {
            parse(&format!(
                "[[input]]\nname = \"cpu\"\ncommand = \"echo 1\"\nevery = {every}\n"
            ))
        };
        assert_eq!(config("2.5").unwrap().inputs[0].every, 2.5);
        for bad in ["inf", "nan", "0", "-1"] {
            let err = format!("{:#}", config(bad).unwrap_err());
            assert!(err.contains("`every` must be"), "{bad}: {err}");
        }
    }

    #[test]
    fn actions_have_unique_names() {
        let one = "[[action]]\nname = \"tape1\"\ncommand = \"firefox\"\n";
        let config = parse(one).unwrap();
        assert_eq!(config.actions[0].name, "tape1");
        assert_eq!(config.actions[0].command.as_deref(), Some("firefox"));
        assert_eq!(config.actions[0].label, None);
        let labelled = parse("[[action]]\nname = \"tape2\"\nlabel = \"Soon\"\n").unwrap();
        assert_eq!(labelled.actions[0].label.as_deref(), Some("Soon"));
        assert_eq!(labelled.actions[0].command, None);
        let err = format!("{:#}", parse(&format!("{one}{one}")).unwrap_err());
        assert!(err.contains("two [[action]] entries"), "{err}");
    }

    #[test]
    fn pause_behind_names_its_modes_and_keys() {
        let err = format!(
            "{:#}",
            parse("[pause_behind]\non_power = \"translucent\"\n").unwrap_err()
        );
        assert!(err.contains("on_power = \"translucent\""), "{err}");
        assert!(
            err.contains("expected one of `any`, `opaque`, `none`"),
            "{err}"
        );
        let err = format!(
            "{:#}",
            parse("[pause_behind]\non_ac = \"any\"\n").unwrap_err()
        );
        assert!(err.contains("expected `on_battery` or `on_power`"), "{err}");
    }

    #[test]
    fn a_display_takes_pause_behind_inline_or_as_a_table() {
        let inline = parse("[[display]]\nid = \"a\"\npause_behind = { on_power = \"none\" }\n");
        let table = parse("[[display]]\nid = \"a\"\n[display.pause_behind]\non_power = \"none\"\n");
        let expected = Some(PauseBehind {
            on_battery: None,
            on_power: Some(Behind::None),
        });
        assert_eq!(inline.unwrap().display[0].pause_behind, expected);
        assert_eq!(table.unwrap().display[0].pause_behind, expected);
    }
}
