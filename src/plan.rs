use std::collections::HashMap;

use anyhow::{Context, anyhow, bail};

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

fn resolve_source(
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

#[derive(Debug, Default)]
pub struct CliPerDisplay {
    /// Pairs of (id, path) from repeated `--display ID PATH`.
    pub pinned: Vec<(String, String)>,
    /// IDs from repeated `--display-rand ID`.
    pub random: Vec<String>,
}

impl CliPerDisplay {
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.random.is_empty()
    }
}

/// Build a Plan from CLI args and config. Precedence: CLI per-display flags >
/// CLI positional/--rand > config `[[display]]`. If none of those produce a
/// source, returns Err.
pub fn build(
    positional_path: Option<String>,
    cli_rand: bool,
    cli_per_display: CliPerDisplay,
    config: &Config,
) -> anyhow::Result<Plan> {
    if !cli_per_display.is_empty() {
        return per_display_from_cli(cli_per_display, &config.alias);
    }
    if let Some(path) = positional_path {
        return Ok(Plan::Mirror(Source::Path(path)));
    }
    if cli_rand {
        return Ok(Plan::Mirror(Source::Random));
    }
    if !config.display.is_empty() {
        return per_display_from_config(&config.display, &config.alias);
    }
    bail!(
        "no video source: pass a path, --rand, --display ID PATH, or configure [[display]] entries"
    );
}

fn per_display_from_cli(cli: CliPerDisplay, aliases: &[Alias]) -> anyhow::Result<Plan> {
    let mut entries: Vec<DisplayAssignment> = Vec::new();
    for (id, path) in cli.pinned {
        let native_id = resolve_id(&id, aliases)?;
        entries.push(DisplayAssignment {
            native_id,
            source: Source::Path(path),
        });
    }
    for id in cli.random {
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
    for d in displays {
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
