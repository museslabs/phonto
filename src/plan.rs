use std::collections::HashMap;

use anyhow::{Context, anyhow, bail};

#[cfg(target_os = "macos")]
use crate::config::PauseBehind;
use crate::config::{Alias, Config, Display, SearchPath};

#[derive(Debug, Clone, Default)]
pub struct YtDlpOpts {
    pub format: Option<String>,
    pub cookies_from_browser: Option<String>,
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Source {
    Path(String),
    Random,
}

#[derive(Debug, Clone)]
pub struct DisplayAssignment {
    /// Native display ID for the current OS (alias-resolved).
    pub native_id: String,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub enum Plan {
    Mirror(Source),
    PerDisplay(Vec<DisplayAssignment>),
}

#[derive(Debug, Clone)]
pub struct ResolvedAssignment {
    pub native_id: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub enum Playback {
    Mirror(String),
    PerDisplay(Vec<ResolvedAssignment>),
}

pub fn resolve(
    plan: Plan,
    search_paths: &[SearchPath],
    yt_dlp: &YtDlpOpts,
) -> anyhow::Result<Playback> {
    match plan {
        Plan::Mirror(source) => Ok(Playback::Mirror(resolve_source(
            source,
            search_paths,
            yt_dlp,
        )?)),
        Plan::PerDisplay(assignments) => {
            let resolved: Vec<ResolvedAssignment> = assignments
                .into_iter()
                .map(|a| {
                    let path = resolve_source(a.source, search_paths, yt_dlp)?;
                    Ok(ResolvedAssignment {
                        native_id: a.native_id,
                        path,
                    })
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Playback::PerDisplay(resolved))
        }
    }
}

pub fn resolve_source(
    source: Source,
    search_paths: &[SearchPath],
    yt_dlp: &YtDlpOpts,
) -> anyhow::Result<String> {
    match source {
        Source::Path(p) => {
            if let Some(yt_url) = crate::config::maybe_youtube_url(&p) {
                return resolve_with_ytdlp(&yt_url, yt_dlp);
            }
            if crate::config::is_url(&p) {
                Ok(p.trim().to_string())
            } else {
                Ok(crate::config::expand_tilde(&p))
            }
        }
        Source::Random => crate::wallpaper::pick_random(search_paths)
            .ok_or_else(|| anyhow!("no wallpapers found in configured search paths"))
            .map(|p| p.to_string_lossy().into_owned()),
    }
}

fn resolve_with_ytdlp(url: &str, opts: &YtDlpOpts) -> anyhow::Result<String> {
    let mut cmd = std::process::Command::new("yt-dlp");
    if let Some(format) = &opts.format {
        cmd.args(["-f", format]);
    }
    if let Some(browser) = &opts.cookies_from_browser {
        cmd.args(["--cookies-from-browser", browser]);
    }
    cmd.args(&opts.extra_args);
    cmd.args(["-g"]);
    cmd.arg(url);

    log::info!("Resolving `{url}` using yt-dlp");

    let output = cmd
        .output()
        .context("failed to run yt-dlp (is it installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp failed:\n{stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("yt-dlp output is not valid UTF-8")?;
    let stream_url = stdout
        .lines()
        .next()
        .ok_or_else(|| anyhow!("yt-dlp produced no output"))?;

    log::info!("yt-dlp resolved: {stream_url}");
    Ok(stream_url.to_string())
}

/// What the command line asked to play, before config is consulted.
#[derive(Debug, Clone)]
pub enum Request {
    /// One source on every display: a positional path or `--rand`.
    Mirror(Source),
    /// Per-display assignments from `--display` / `--display-rand`.
    PerDisplay {
        /// Pairs of (id, path) from repeated `--display ID PATH`.
        pinned: Vec<(String, String)>,
        /// IDs from repeated `--display-rand ID`.
        random: Vec<String>,
    },
    /// Nothing on the command line; fall back to config `[[display]]`.
    FromConfig,
}

/// Build a Plan from the CLI request, falling back to config `[[display]]`
/// when the CLI names no source. If neither produces a source, returns Err.
pub fn build(request: Request, config: &Config) -> anyhow::Result<Plan> {
    match request {
        Request::Mirror(source) => Ok(Plan::Mirror(source)),
        Request::PerDisplay { pinned, random } => {
            per_display_from_cli(pinned, random, &config.alias)
        }
        Request::FromConfig if config.display.iter().any(|d| !d.only_pause_behind()) => {
            per_display_from_config(&config.display, &config.alias)
        }
        Request::FromConfig => bail!(
            "no video source: pass a path, --rand, --display ID PATH, or configure [[display]] entries"
        ),
    }
}

fn per_display_from_cli(
    pinned: Vec<(String, String)>,
    random: Vec<String>,
    aliases: &[Alias],
) -> anyhow::Result<Plan> {
    let mut entries: Vec<DisplayAssignment> = Vec::new();
    for (id, path) in pinned {
        let native_id = resolve_id(&id, aliases)?;
        entries.push(DisplayAssignment {
            native_id,
            source: Source::Path(path),
        });
    }
    for id in random {
        let native_id = resolve_id(&id, aliases)?;
        entries.push(DisplayAssignment {
            native_id,
            source: Source::Random,
        });
    }
    dedupe(&mut entries)?;
    Ok(Plan::PerDisplay(entries))
}

fn per_display_from_config(displays: &[Display], aliases: &[Alias]) -> anyhow::Result<Plan> {
    let mut entries: Vec<DisplayAssignment> = Vec::new();
    for d in displays.iter().filter(|d| !d.only_pause_behind()) {
        let source = match (&d.path, d.random) {
            (Some(p), false) => Source::Path(p.clone()),
            (None, true) => Source::Random,
            (Some(_), true) => {
                bail!(
                    "[[display]] `{}`: set exactly one of `path` or `random`",
                    d.id
                );
            }
            (None, false) => {
                bail!(
                    "[[display]] `{}`: needs either `path` or `random = true`",
                    d.id
                );
            }
        };
        let native_id = resolve_id(&d.id, aliases)?;
        entries.push(DisplayAssignment { native_id, source });
    }
    dedupe(&mut entries)?;
    Ok(Plan::PerDisplay(entries))
}

/// The windows that pause the wallpaper of each display.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct PauseBehindPlan {
    /// `[pause_behind]`, for the displays without a `pause_behind` of their own.
    pub table: PauseBehind,
    /// The `pause_behind` of the `[[display]]` entries, by native ID. The keys
    /// that an entry leaves out come from the table.
    pub displays: HashMap<String, PauseBehind>,
}

#[cfg(target_os = "macos")]
impl PauseBehindPlan {
    /// True if a display pauses for other windows on battery than on power.
    pub fn follows_power(&self) -> bool {
        std::iter::once(&self.table)
            .chain(self.displays.values())
            .any(|p| p.mode(true) != p.mode(false))
    }
}

/// Reads the `pause_behind` of the `[[display]]` entries. It applies whatever
/// the command line plays.
#[cfg(target_os = "macos")]
pub fn pause_behind(config: &Config) -> anyhow::Result<PauseBehindPlan> {
    let mut displays = HashMap::new();
    for d in &config.display {
        let Some(own) = d.pause_behind else {
            continue;
        };
        let native_id = resolve_id(&d.id, &config.alias)?;
        if displays.contains_key(&native_id) {
            bail!("display `{native_id}` has `pause_behind` in two [[display]] entries");
        }
        displays.insert(native_id, own.or(config.pause_behind));
    }
    Ok(PauseBehindPlan {
        table: config.pause_behind,
        displays,
    })
}

fn resolve_id(id: &str, aliases: &[Alias]) -> anyhow::Result<String> {
    let Some(alias) = aliases.iter().find(|a| a.name == id) else {
        // Not an alias; treat as a raw native ID.
        return Ok(id.to_string());
    };

    #[cfg(target_os = "macos")]
    {
        return alias
            .macos
            .clone()
            .with_context(|| format!("alias `{id}` has no `macos = \"...\"` entry"));
    }
    #[cfg(target_os = "linux")]
    {
        return alias
            .wayland
            .clone()
            .with_context(|| format!("alias `{id}` has no `wayland = \"...\"` entry"));
    }
    #[allow(unreachable_code)]
    {
        let _ = alias;
        bail!("unsupported platform for alias resolution");
    }
}

fn dedupe(entries: &mut [DisplayAssignment]) -> anyhow::Result<()> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(prev) = seen.insert(e.native_id.as_str(), i) {
            bail!(
                "display `{}` assigned twice (entries {} and {})",
                e.native_id,
                prev,
                i
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    const ONLY_PAUSE_BEHIND: &str =
        "[[display]]\nid = \"b\"\npause_behind = { on_power = \"none\" }\n";

    #[test]
    fn an_entry_with_only_pause_behind_plays_nothing() {
        let config = config(&format!(
            "[[display]]\nid = \"a\"\npath = \"a.mp4\"\n{ONLY_PAUSE_BEHIND}"
        ));
        let Plan::PerDisplay(entries) = build(Request::FromConfig, &config).unwrap() else {
            panic!("expected per-display entries");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].native_id, "a");
    }

    #[test]
    fn entries_with_only_pause_behind_name_no_source() {
        let err = build(Request::FromConfig, &config(ONLY_PAUSE_BEHIND)).unwrap_err();
        assert!(format!("{err:#}").starts_with("no video source"), "{err:#}");
        let empty = build(Request::FromConfig, &config("[[display]]\nid = \"b\"\n")).unwrap_err();
        assert!(format!("{empty:#}").contains("needs either"), "{empty:#}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_display_falls_back_to_the_table_and_then_to_any() {
        use crate::config::Behind;

        let table = format!("[pause_behind]\non_battery = \"opaque\"\n{ONLY_PAUSE_BEHIND}");
        let plan = pause_behind(&config(&table)).unwrap();
        let b = plan.displays["b"];
        assert_eq!(b.mode(false), Behind::None);
        assert_eq!(b.mode(true), Behind::Opaque);
        assert_eq!(plan.table.mode(true), Behind::Opaque);
        assert_eq!(plan.table.mode(false), Behind::Any);
        assert!(plan.follows_power());

        let plan = pause_behind(&config(ONLY_PAUSE_BEHIND)).unwrap();
        assert_eq!(plan.displays["b"].mode(true), Behind::Any);
        assert_eq!(plan.table.mode(true), Behind::Any);
        assert_eq!(plan.table.mode(false), Behind::Any);
        assert!(plan.follows_power());
        assert!(!pause_behind(&config("")).unwrap().follows_power());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn two_entries_for_one_display_fail_after_alias_resolution() {
        let config = config(
            "[[alias]]\nname = \"main\"\nmacos = \"LG\"\n\n\
             [[display]]\nid = \"main\"\npath = \"a.mp4\"\npause_behind = { on_power = \"none\" }\n\n\
             [[display]]\nid = \"LG\"\n\n[display.pause_behind]\non_battery = \"any\"\n",
        );
        let err = pause_behind(&config).unwrap_err();
        assert!(format!("{err:#}").contains("display `LG`"), "{err:#}");
    }
}
