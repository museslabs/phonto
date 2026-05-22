use std::collections::HashMap;

use anyhow::{Context, bail};

use crate::config::{Alias, Config, Display};

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
