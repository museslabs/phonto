//! The settings of a wallpaper from the gallery: `[[input]]` and `[[action]]`
//! entries in `~/.config/phonto/wallpapers/<id>.toml`. While the wallpaper
//! plays, an entry there wins over the entry with the same name in
//! config.toml.
//!
//! phonto never turns on a command by itself. The template that `edit`
//! writes has every command commented out, and `set-input --suggested`
//! copies a command of the wallpaper only when the user asks for it.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::cli::SettingsCommand;
use crate::config::{self, ActionCommand, Config, InputCommand};
use crate::gallery::{self, print_json};
use crate::procedural::{self, NamedInput};

/// The entries of a settings file. They have the keys of config.toml.
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default, rename = "input", skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputCommand>,
    #[serde(default, rename = "action", skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionCommand>,
}

impl Settings {
    fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.actions.is_empty()
    }
}

/// The settings file of the wallpaper `id`.
pub fn path(id: &str) -> PathBuf {
    path_in(&config::config_dir(), id)
}

fn path_in(config_dir: &Path, id: &str) -> PathBuf {
    config_dir.join("wallpapers").join(format!("{id}.toml"))
}

/// Removes the settings of `id`. True if it had settings.
pub fn remove(id: &str) -> anyhow::Result<bool> {
    remove_file(&path(id))
}

fn remove_file(file: &Path) -> anyhow::Result<bool> {
    match fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", file.display())),
    }
}

/// Parses and checks settings. The checks are the ones of config.toml, and
/// an input can have only one entry, so it is clear which one plays.
fn parse(text: &str) -> anyhow::Result<Settings> {
    let settings: Settings = toml::from_str(text)?;
    config::check_commands(&settings.inputs, &settings.actions)?;
    for (i, input) in settings.inputs.iter().enumerate() {
        if settings.inputs[..i].iter().any(|o| o.name == input.name) {
            bail!("two [[input]] entries are named `{}`", input.name);
        }
    }
    Ok(settings)
}

/// The settings in `file`, or `None` without the file.
fn load(file: &Path) -> anyhow::Result<Option<Settings>> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", file.display()));
        }
    };
    let settings =
        parse(&text).with_context(|| format!("invalid settings in {}", file.display()))?;
    Ok(Some(settings))
}

/// The `[[input]]` and `[[action]]` entries for the wallpapers at `paths`:
/// the ones of config.toml, and over them the settings of each wallpaper
/// from the gallery.
pub fn merge(
    inputs: Vec<InputCommand>,
    actions: Vec<ActionCommand>,
    paths: &[&str],
) -> anyhow::Result<(Vec<InputCommand>, Vec<ActionCommand>)> {
    merge_in(&config::config_dir(), inputs, actions, paths)
}

fn merge_in(
    config_dir: &Path,
    inputs: Vec<InputCommand>,
    actions: Vec<ActionCommand>,
    paths: &[&str],
) -> anyhow::Result<(Vec<InputCommand>, Vec<ActionCommand>)> {
    let mut layers: Vec<(String, Settings)> = Vec::new();
    for path in paths {
        let Some(id) = gallery::identify(path) else {
            continue;
        };
        if layers.iter().any(|(other, _)| *other == id) {
            continue;
        }
        if let Some(settings) = load(&path_in(config_dir, &id))? {
            log::info!("using the settings of `{id}`");
            layers.push((id, settings));
        }
    }
    Ok(apply(inputs, actions, &layers))
}

/// Puts the settings of each wallpaper, in order, over the entries of
/// config.toml.
fn apply(
    mut inputs: Vec<InputCommand>,
    mut actions: Vec<ActionCommand>,
    layers: &[(String, Settings)],
) -> (Vec<InputCommand>, Vec<ActionCommand>) {
    let ids = || layers.iter().map(|(id, settings)| (id.as_str(), settings));
    overlay(
        &mut inputs,
        ids().map(|(id, s)| (id, &s.inputs[..])),
        "input",
    );
    overlay(
        &mut actions,
        ids().map(|(id, s)| (id, &s.actions[..])),
        "action",
    );
    (inputs, actions)
}

/// An `[[input]]` or an `[[action]]` entry.
trait Named: Clone + PartialEq {
    fn name(&self) -> &str;
}

impl Named for InputCommand {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for ActionCommand {
    fn name(&self) -> &str {
        &self.name
    }
}

/// An entry of a layer replaces the entries of `base` with its name, or
/// comes after them. If two wallpapers set a name in different ways, the
/// later one wins.
fn overlay<'a, T: Named + 'a>(
    base: &mut Vec<T>,
    layers: impl Iterator<Item = (&'a str, &'a [T])>,
    kind: &str,
) {
    let mut set: HashMap<&'a str, (&'a str, &'a T)> = HashMap::new();
    for (id, entries) in layers {
        for entry in entries {
            let name = entry.name();
            if let Some((other, earlier)) = set.insert(name, (id, entry))
                && earlier != entry
            {
                log::warn!(
                    "the settings of `{other}` and of `{id}` set the {kind} `{name}` in \
                     different ways. phonto uses the ones of `{id}`"
                );
            }
            base.retain(|e| e.name() != name);
            base.push(entry.clone());
        }
    }
}

/// What a wallpaper reads: its inputs, the names of its actions, and the
/// commands that its inputs file suggests.
#[derive(Debug, Default)]
pub struct Reads {
    pub inputs: Vec<NamedInput>,
    pub actions: Vec<String>,
    /// The `[[input]]` entries of the inputs file that are not commented out.
    pub suggested: Vec<InputCommand>,
    /// The text of the inputs file, for the template of `edit`.
    inputs_text: Option<String>,
}

impl Reads {
    /// From the text of the manifest (`None` for a single `.glsl` file), and
    /// of the inputs file.
    pub fn parse(manifest: Option<&str>, inputs_text: Option<&str>) -> anyhow::Result<Self> {
        let (inputs, actions) = match manifest {
            Some(text) => procedural::declared(text)
                .context("the manifest.toml of the wallpaper is not valid")?,
            None => Default::default(),
        };
        Ok(Reads {
            inputs,
            actions,
            suggested: inputs_text.map(suggestions).unwrap_or_default(),
            inputs_text: inputs_text.map(str::to_string),
        })
    }

    /// From the folder of a wallpaper, with the `run` and the `inputs` of its
    /// record.
    pub fn from_folder(dir: &Path, run: &str, inputs: Option<&str>) -> anyhow::Result<Self> {
        let manifest = manifest_path(run)
            .map(|name| {
                let path = dir.join(name);
                fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))
            })
            .transpose()?;
        // The wallpaper plays without its inputs file, so that is only a
        // warning.
        let inputs_text = inputs.and_then(|name| {
            fs::read_to_string(dir.join(name))
                .inspect_err(|err| log::warn!("failed to read the inputs file `{name}`: {err}"))
                .ok()
        });
        Self::parse(manifest.as_deref(), inputs_text.as_deref())
    }

    /// True if the wallpaper reads inputs or has actions.
    pub fn has_commands(&self) -> bool {
        !self.inputs.is_empty() || !self.actions.is_empty()
    }

    /// The inputs and the actions, for the output. With `set`, each one also
    /// says what sets it: the settings of the wallpaper, or config.toml.
    pub fn views(&self, set: Option<(&Settings, &Config)>) -> (Vec<InputView>, Vec<ActionView>) {
        let inputs = self.inputs.iter().map(|input| {
            let configured = set.map(|(settings, config)| {
                let own = settings.inputs.iter().find(|i| i.name == input.name);
                let from_config = || config.inputs.iter().find(|i| i.name == input.name);
                let (entry, from) = match own {
                    Some(entry) => (entry, SetBy::Wallpaper),
                    None => (from_config()?, SetBy::Config),
                };
                Some(ConfiguredInput {
                    command: entry.command.clone(),
                    every: entry.every,
                    from,
                })
            });
            InputView {
                name: input.name.clone(),
                size: input.size,
                default: input.default[..input.size].to_vec(),
                suggested: (self.suggested.iter())
                    .filter(|s| s.name == input.name)
                    .map(|s| Suggested {
                        command: s.command.clone(),
                        every: s.every,
                    })
                    .collect(),
                configured,
            }
        });
        let actions = self.actions.iter().map(|name| {
            let configured = set.map(|(settings, config)| {
                let own = settings.actions.iter().find(|a| a.name == *name);
                let from_config = || config.actions.iter().find(|a| a.name == *name);
                let (entry, from) = match own {
                    Some(entry) => (entry, SetBy::Wallpaper),
                    None => (from_config()?, SetBy::Config),
                };
                Some(ConfiguredAction {
                    label: entry.label.clone(),
                    command: entry.command.clone(),
                    from,
                })
            });
            ActionView {
                name: name.clone(),
                configured,
            }
        });
        (inputs.collect(), actions.collect())
    }
}

/// The manifest of a wallpaper with this `run`: `manifest.toml` for a pack,
/// and none for a single `.glsl` file.
pub fn manifest_path(run: &str) -> Option<&str> {
    if run.is_empty() {
        Some(procedural::MANIFEST)
    } else if Path::new(run)
        .file_name()
        .is_some_and(|n| n == procedural::MANIFEST)
    {
        Some(run)
    } else {
        None
    }
}

/// The `[[input]]` entries of an inputs file that phonto can use. It skips
/// the others.
fn suggestions(text: &str) -> Vec<InputCommand> {
    #[derive(Deserialize)]
    struct InputsFile {
        #[serde(default)]
        input: Vec<toml::Value>,
    }
    let file: InputsFile = match toml::from_str(text) {
        Ok(file) => file,
        Err(err) => {
            log::warn!("the inputs file of the wallpaper is not valid TOML: {err}");
            return Vec::new();
        }
    };
    (file.input.into_iter())
        .filter_map(|value| value.try_into::<InputCommand>().ok())
        .filter(|input| config::check_commands(std::slice::from_ref(input), &[]).is_ok())
        .collect()
}

/// An input of a wallpaper, in the JSON output.
#[derive(Debug, Serialize)]
pub struct InputView {
    name: String,
    size: usize,
    default: Vec<f32>,
    suggested: Vec<Suggested>,
    /// Only `phonto settings` has this key. It is `null` when nothing sets
    /// the input.
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<Option<ConfiguredInput>>,
}

#[derive(Debug, Serialize)]
struct Suggested {
    command: String,
    every: f64,
}

#[derive(Debug, Serialize)]
struct ConfiguredInput {
    command: String,
    every: f64,
    from: SetBy,
}

/// An action of a wallpaper, in the JSON output.
#[derive(Debug, Serialize)]
pub struct ActionView {
    name: String,
    /// As for `InputView`.
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<Option<ConfiguredAction>>,
}

#[derive(Debug, Serialize)]
struct ConfiguredAction {
    label: Option<String>,
    command: Option<String>,
    from: SetBy,
}

/// Where an entry comes from.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SetBy {
    /// The settings of the wallpaper.
    Wallpaper,
    Config,
}

impl SetBy {
    fn place(self) -> &'static str {
        match self {
            SetBy::Wallpaper => "this wallpaper",
            SetBy::Config => "config.toml",
        }
    }
}

/// Prints the inputs and the actions of a wallpaper.
pub fn print_views(inputs: &[InputView], actions: &[ActionView]) {
    if inputs.is_empty() && actions.is_empty() {
        println!("It reads no inputs and has no actions.");
        return;
    }
    if !inputs.is_empty() {
        println!("Inputs:");
    }
    for input in inputs {
        println!(
            "  {}: {}",
            input.name,
            describe_default(input.size, &input.default)
        );
        match &input.configured {
            Some(Some(set)) => println!(
                "    from {}: {}, every {} s",
                set.from.place(),
                gallery::clean(&set.command),
                set.every
            ),
            Some(None) => println!("    not set, so it keeps its default"),
            None => {}
        }
        // A suggestion helps only while nothing sets the input.
        if !matches!(input.configured, Some(Some(_))) {
            let several = input.suggested.len() > 1;
            for (i, s) in input.suggested.iter().enumerate() {
                let n = if several {
                    format!(" {}", i + 1)
                } else {
                    String::new()
                };
                let command = gallery::clean(&s.command);
                println!("    suggested{n}: {command}, every {} s", s.every);
            }
        }
    }
    if !actions.is_empty() {
        println!("Actions:");
    }
    for action in actions {
        match &action.configured {
            Some(Some(set)) => {
                let label = set.label.as_deref().map(gallery::clean);
                let command = set.command.as_deref().map(gallery::clean);
                let what = match (label, command) {
                    (Some(label), Some(command)) => format!("\"{label}\" runs {command}"),
                    (None, Some(command)) => format!("runs {command}"),
                    (Some(label), None) => format!("\"{label}\", and a click does nothing"),
                    (None, None) => "a click does nothing".to_string(),
                };
                println!("  {}: {what} (from {})", action.name, set.from.place());
            }
            Some(None) => println!("  {}: not set, so a click does nothing", action.name),
            None => println!("  {}", action.name),
        }
    }
}

/// "3 numbers, by default 0, 0, 0".
fn describe_default(size: usize, default: &[f32]) -> String {
    let values: Vec<String> = default.iter().take(size).map(|v| v.to_string()).collect();
    let count = if size == 1 {
        "a number".to_string()
    } else {
        format!("{size} numbers")
    };
    format!("{count}, by default {}", values.join(", "))
}

/// `phonto settings --json`, and the output of each change with `--json`.
#[derive(Serialize)]
struct SettingsJson<'a> {
    id: &'a str,
    title: Option<String>,
    settings: Option<String>,
    inputs: Vec<InputView>,
    actions: Vec<ActionView>,
}

/// `phonto settings path --json`.
#[derive(Serialize)]
struct PathJson<'a> {
    id: &'a str,
    path: String,
}

/// `phonto settings <WALLPAPER> [COMMAND]`.
pub fn run(wallpaper: &str, command: Option<SettingsCommand>, json: bool) -> anyhow::Result<()> {
    let config = config::load()?;
    let gallery_key = config.gallery.as_deref();
    let id = gallery::wallpaper_id(wallpaper, gallery_key)?;
    let file = path(&id);
    let json = json || command.as_ref().is_some_and(SettingsCommand::json);
    // These need only the id, not the files of the wallpaper.
    match &command {
        Some(SettingsCommand::Path { .. }) if json => {
            let path = file.to_string_lossy().into_owned();
            return print_json(&PathJson { id: &id, path });
        }
        Some(SettingsCommand::Path { .. }) => {
            println!("{}", file.display());
            return Ok(());
        }
        Some(SettingsCommand::Reset { .. }) if !json => {
            if remove_file(&file)? {
                println!("Removed the settings of `{id}`, {}", file.display());
            } else {
                println!("`{id}` has no settings");
            }
            return Ok(());
        }
        Some(SettingsCommand::Edit) if json => {
            bail!("`edit` opens an editor, so it has no --json")
        }
        _ => {}
    }
    let local = gallery::local(wallpaper, gallery_key)?;
    let reads = local.reads()?;
    let show = command.is_none();
    match command {
        None | Some(SettingsCommand::Path { .. }) => {}
        Some(SettingsCommand::Edit) => return edit(&id, &reads, &file),
        Some(SettingsCommand::SetInput {
            name,
            command,
            every,
            suggested,
            pick,
            ..
        }) => {
            let pick = suggested.then_some(pick);
            let entry = input_entry(&id, &reads, &name, command, every, pick)?;
            let message = format!(
                "`{name}` of `{id}` now comes from `{}`, every {} s",
                gallery::clean(&entry.command),
                entry.every
            );
            change(&file, &id, |s| {
                s.inputs.retain(|i| i.name != name);
                s.inputs.push(entry);
            })?;
            if !json {
                println!("{message}. Saved in {}", file.display());
            }
        }
        Some(SettingsCommand::UnsetInput { name, .. }) => {
            change(&file, &id, |s| s.inputs.retain(|i| i.name != name))?;
            if !json {
                println!("`{id}` sets no command for `{name}` now");
            }
        }
        Some(SettingsCommand::SetAction {
            name,
            label,
            command,
            ..
        }) => {
            check_action_name(&id, &reads, &name)?;
            if label.is_none() && command.is_none() {
                bail!("pass --label, --command, or both");
            }
            change(&file, &id, |s| set_action(s, &name, label, command))?;
            if !json {
                println!(
                    "Set the action `{name}` of `{id}`. Saved in {}",
                    file.display()
                );
            }
        }
        Some(SettingsCommand::UnsetAction { name, .. }) => {
            change(&file, &id, |s| s.actions.retain(|a| a.name != name))?;
            if !json {
                println!("`{id}` sets nothing for the action `{name}` now");
            }
        }
        Some(SettingsCommand::Reset { .. }) => {
            remove_file(&file)?;
        }
    }
    if !(json || show) {
        return Ok(());
    }
    let settings = load(&file)?;
    let (inputs, actions) = reads.views(Some((&settings.clone().unwrap_or_default(), &config)));
    let exists = settings.is_some();
    if json {
        return print_json(&SettingsJson {
            id: &id,
            title: Some(gallery::clean(&local.record.title)).filter(|t| !t.is_empty()),
            settings: exists.then(|| file.to_string_lossy().into_owned()),
            inputs,
            actions,
        });
    }
    let record = &local.record;
    let title = gallery::describe(&id, Some(&record.title), Some(&record.author));
    println!("{title}");
    let none = if exists { "" } else { " (none yet)" };
    println!("Settings: {}{none}", file.display());
    print_views(&inputs, &actions);
    if reads.has_commands() {
        let set = if !reads.inputs.is_empty() {
            if reads.suggested.is_empty() {
                format!("`phonto settings {id} set-input <name> --command <command>`")
            } else {
                format!("`phonto settings {id} set-input <name> --suggested`")
            }
        } else {
            format!("`phonto settings {id} set-action <name> --command <command>`")
        };
        println!("Change them with `phonto settings {id} edit`, or {set}.");
    }
    Ok(())
}

/// The entry that `set-input` writes: with the given command, or with a
/// suggested one if `pick` is `Some`.
fn input_entry(
    id: &str,
    reads: &Reads,
    name: &str,
    command: Option<String>,
    every: Option<f64>,
    pick: Option<Option<u32>>,
) -> anyhow::Result<InputCommand> {
    check_input_name(id, reads, name)?;
    let suggested: Vec<&InputCommand> = reads.suggested.iter().filter(|s| s.name == name).collect();
    match (command, pick) {
        (Some(command), _) => Ok(InputCommand {
            name: name.to_string(),
            command,
            every: every.or(suggested.first().map(|s| s.every)).unwrap_or(1.0),
        }),
        (None, Some(pick)) => pick_suggestion(id, name, &suggested, pick),
        (None, None) => bail!("pass --command or --suggested"),
    }
}

fn pick_suggestion(
    id: &str,
    name: &str,
    suggested: &[&InputCommand],
    pick: Option<u32>,
) -> anyhow::Result<InputCommand> {
    let count = suggested.len();
    match (suggested, pick) {
        ([], _) => bail!("`{id}` suggests no command for `{name}`. Set one with --command"),
        ([one], None) => Ok((*one).clone()),
        (_, Some(n)) => (n as usize)
            .checked_sub(1)
            .and_then(|i| suggested.get(i))
            .map(|s| (*s).clone())
            .with_context(|| {
                format!(
                    "`{id}` suggests {count} commands for `{name}`, so --pick takes 1 to {count}"
                )
            }),
        (_, None) => {
            let list: Vec<String> = (suggested.iter().enumerate())
                .map(|(i, s)| {
                    let command = gallery::clean(&s.command);
                    format!("  {}. {command}, every {} s", i + 1, s.every)
                })
                .collect();
            bail!(
                "`{id}` suggests {count} commands for `{name}`. Choose one with --pick:\n{}",
                list.join("\n")
            )
        }
    }
}

/// Sets the label or the command of an action, and keeps what it does not
/// set.
fn set_action(settings: &mut Settings, name: &str, label: Option<String>, command: Option<String>) {
    match settings.actions.iter_mut().find(|a| a.name == name) {
        Some(action) => {
            if label.is_some() {
                action.label = label;
            }
            if command.is_some() {
                action.command = command;
            }
        }
        None => settings.actions.push(ActionCommand {
            name: name.to_string(),
            label,
            command,
        }),
    }
}

/// Checks that `settings` names only inputs and actions that the wallpaper
/// has, to catch a typo.
fn check_names(id: &str, settings: &Settings, reads: &Reads) -> anyhow::Result<()> {
    for input in &settings.inputs {
        check_input_name(id, reads, &input.name)?;
    }
    for action in &settings.actions {
        check_action_name(id, reads, &action.name)?;
    }
    Ok(())
}

fn check_input_name(id: &str, reads: &Reads, name: &str) -> anyhow::Result<()> {
    let names: Vec<&str> = reads.inputs.iter().map(|i| i.name.as_str()).collect();
    if names.contains(&name) {
        return Ok(());
    }
    if names.is_empty() {
        bail!("`{id}` reads no inputs, so it has no input `{name}`");
    }
    bail!(
        "`{id}` has no input named `{name}`. Its inputs are {}",
        names.join(", ")
    )
}

fn check_action_name(id: &str, reads: &Reads, name: &str) -> anyhow::Result<()> {
    if reads.actions.iter().any(|a| a == name) {
        return Ok(());
    }
    if reads.actions.is_empty() {
        bail!("`{id}` has no actions, so it has no action `{name}`");
    }
    bail!(
        "`{id}` has no action named `{name}`. Its actions are {}",
        reads.actions.join(", ")
    )
}

/// Changes the settings of `id`, and writes them again, all at once.
fn change(file: &Path, id: &str, f: impl FnOnce(&mut Settings)) -> anyhow::Result<()> {
    let mut settings = match load(file) {
        Ok(settings) => settings.unwrap_or_default(),
        Err(err) => bail!(
            "{}. Fix it with `phonto settings {id} edit`, or remove it with \
             `phonto settings {id} reset`",
            format!("{err:#}").trim_end()
        ),
    };
    f(&mut settings);
    config::check_commands(&settings.inputs, &settings.actions)?;
    write(file, id, &settings)
}

/// Writes the settings of `id`, under the header. Without entries, the file
/// goes away.
fn write(file: &Path, id: &str, settings: &Settings) -> anyhow::Result<()> {
    if settings.is_empty() {
        remove_file(file)?;
        return Ok(());
    }
    write_atomic(
        file,
        &format!("{}\n{}", header(id), toml::to_string(settings)?),
    )
}

/// Writes a temporary file next to `file`, then renames it, so a reader
/// never sees half a file.
fn write_atomic(file: &Path, text: &str) -> anyhow::Result<()> {
    let dir = file.parent().context("a settings file has no folder")?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    fs::write(&temp, text)
        .and_then(|()| fs::rename(&temp, file))
        .inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
        .with_context(|| format!("failed to write {}", file.display()))
}

/// The comment at the top of every settings file.
fn header(id: &str) -> String {
    format!(
        "# Settings of the wallpaper `{id}` for phonto: the commands of its inputs\n\
         # and actions. An entry here wins over the [[input]] or [[action]] entry\n\
         # with the same name in config.toml. `phonto settings {id}` shows what\n\
         # the wallpaper reads.\n"
    )
}

/// The settings file that `edit` starts from: the suggested entries of the
/// inputs file with their comments, and an empty entry for each input
/// without one and for each action. Every line is a comment, so nothing is
/// on.
fn template(id: &str, reads: &Reads) -> String {
    let mut out = header(id);
    out.push_str(
        "#\n# Nothing below is on. Read a command, then remove the # in front of\n\
         # the lines of its entry to turn it on. Keep one entry for each name.\n",
    );
    let blocks = reads.inputs_text.as_deref().map(blocks).unwrap_or_default();
    for input in &reads.inputs {
        let default = describe_default(input.size, &input.default);
        out.push_str(&format!("\n# Input `{}`: {default}.\n", input.name));
        let mut found = false;
        for block in blocks.iter().filter(|b| b.name == input.name) {
            found = true;
            out.push('\n');
            for line in &block.lines {
                out.push_str(&comment_out(line));
                out.push('\n');
            }
        }
        if !found {
            out.push_str(&format!(
                "\n# [[input]]\n# name = \"{}\"\n# command = \"\"\n# every = 1\n",
                input.name
            ));
        }
    }
    for action in &reads.actions {
        out.push_str(&format!(
            "\n# Action `{action}`.\n\n# [[action]]\n# name = \"{action}\"\n# label = \"\"\n\
             # command = \"\"\n"
        ));
    }
    out
}

/// `line` as a comment. Control characters go away, so a line of the inputs
/// file cannot end the comment early.
fn comment_out(line: &str) -> String {
    let line: String = line
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    if line.trim_start().starts_with('#') {
        line
    } else {
        format!("# {line}")
    }
}

/// An `[[input]]` entry of an inputs file, commented out or not, with the
/// comment over it.
#[derive(Debug, PartialEq)]
struct Block {
    name: String,
    lines: Vec<String>,
}

/// The entries of an inputs file. An entry is its `[[input]]` line, the
/// `key = value` lines under it, and the comment lines over it. Blank lines
/// separate entries too. The text before the first entry, such as the
/// comment at the top of the file, belongs to no entry.
fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for paragraph in lines.split(|line| line.trim().is_empty()) {
        let headers: Vec<usize> = (0..paragraph.len())
            .filter(|&i| uncomment(paragraph[i]) == "[[input]]")
            .collect();
        let mut start = 0;
        for (k, &header) in headers.iter().enumerate() {
            let mut end = header + 1;
            while end < paragraph.len() && is_key_line(uncomment(paragraph[end])) {
                end += 1;
            }
            // The lines after the last entry stay with it.
            if k + 1 == headers.len() {
                end = paragraph.len();
            }
            let name = paragraph[header + 1..end]
                .iter()
                .find_map(|line| key_name(uncomment(line)));
            if let Some(name) = name {
                let lines = paragraph[start..end]
                    .iter()
                    .map(|l| l.to_string())
                    .collect();
                out.push(Block { name, lines });
            }
            start = end;
        }
    }
    out
}

/// A line without the `#` of a comment.
fn uncomment(line: &str) -> &str {
    line.trim().trim_start_matches('#').trim()
}

fn is_key_line(line: &str) -> bool {
    line.split_once('=').is_some_and(|(key, _)| {
        let key = key.trim();
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The value of a `name = "..."` line.
fn key_name(line: &str) -> Option<String> {
    let (key, _) = line.split_once('=')?;
    if key.trim() != "name" {
        return None;
    }
    let table: toml::Table = toml::from_str(line).ok()?;
    table.get("name")?.as_str().map(str::to_string)
}

/// `phonto settings <WALLPAPER> edit`: opens the settings in the editor of
/// the user, and checks them after. A new file starts from the template.
fn edit(id: &str, reads: &Reads, file: &Path) -> anyhow::Result<()> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        bail!(
            "`phonto settings {id} edit` opens an editor, so it needs a terminal. Without one, \
             use `phonto settings {id} set-input` or `set-action`"
        );
    }
    let before = match fs::read_to_string(file) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", file.display())),
    };
    if before.is_none() {
        write_atomic(file, &template(id, reads))?;
    }
    let editor = editor();
    loop {
        // sh splits the editor command, so an editor with arguments works,
        // such as `code -w`. The file is an argument, never part of the
        // command.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(file)
            .status()
            .with_context(|| format!("failed to start the editor `{editor}`"))?;
        if !status.success() {
            restore(file, before.as_deref())?;
            bail!("the editor `{editor}` failed, so the settings stay as they were");
        }
        let text = fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        match parse(&text).and_then(|s| check_names(id, &s, reads).map(|()| s)) {
            Ok(_) => {
                println!("Saved the settings of `{id}` in {}", file.display());
                return Ok(());
            }
            Err(err) => {
                eprintln!("{}: {err:#}", file.display());
                if !ask("Edit it again? [Y/n] ") {
                    restore(file, before.as_deref())?;
                    bail!("the settings stay as they were");
                }
            }
        }
    }
}

/// `$VISUAL`, or else `$EDITOR`, or else vi.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Asks a question, where yes is the default.
fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    match io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => false,
        Ok(_) => !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no"),
    }
}

/// Puts back the file as it was before `edit`: its text, or no file.
fn restore(file: &Path, before: Option<&str>) -> anyhow::Result<()> {
    match before {
        Some(text) => write_atomic(file, text),
        None => remove_file(file).map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\
[image]
shader = \"image.glsl\"

[inputs]
load = [0.0, 0.0, 0.0]
workspace = 1.0
memory = 0

[actions]
buffer = \"pick\"
names = [\"tape1\", \"tape2\"]
";

    const INPUTS: &str = "\
# The commands of this wallpaper. Add them to your configuration:
#
#   cat inputs.toml >> ~/.config/phonto/config.toml

# The load averages of the last 1, 5, and 15 minutes.
[[input]]
name = \"load\"
command = \"uptime | sed 's/.*: //'\"
every = 2

# Hyprland
[[input]]
name = \"workspace\"
command = \"hyprctl activeworkspace -j | jq .id\"
every = 0.2

# Sway
# [[input]]
# name = \"workspace\"
# command = \"swaymsg -t get_workspaces | jq '.[] | select(.focused).num'\"
# every = 0.2

# niri
[[input]]
name = \"workspace\"
command = \"niri msg -j workspaces | jq '.[] | select(.is_focused).idx'\"
every = 0.5

# A command for an input that the wallpaper does not read.
[[input]]
name = \"weather\"
command = \"curl -s wttr.in\"
every = 900
";

    fn reads() -> Reads {
        Reads::parse(Some(MANIFEST), Some(INPUTS)).unwrap()
    }

    fn input(name: &str, command: &str) -> InputCommand {
        InputCommand {
            name: name.to_string(),
            command: command.to_string(),
            every: 1.0,
        }
    }

    fn action(name: &str, command: &str) -> ActionCommand {
        ActionCommand {
            name: name.to_string(),
            label: None,
            command: Some(command.to_string()),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("phonto-settings-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_live_next_to_the_configuration() {
        assert_eq!(
            path_in(Path::new("/home/me/.config/phonto"), "koi"),
            PathBuf::from("/home/me/.config/phonto/wallpapers/koi.toml")
        );
        assert_eq!(
            path("koi"),
            config::config_dir().join("wallpapers/koi.toml")
        );
    }

    #[test]
    fn settings_have_the_checks_of_the_configuration() {
        assert_eq!(parse("").unwrap(), Settings::default());
        let one = "[[input]]\nname = \"load\"\ncommand = \"uptime\"\nevery = 2\n";
        assert_eq!(parse(one).unwrap().inputs[0].every, 2.0);
        for (bad, message) in [
            ("gallery = \"x\"\n", "unknown field"),
            (
                "[[input]]\nname = \"a\"\ncommand = \"x\"\nevery = 0\n",
                "`every` must be",
            ),
            (
                "[[input]]\nname = \"a\"\ncommand = \"x\"\n",
                "missing field `every`",
            ),
            (
                "[[input]]\nname = \"a\"\ncommand = \"x\"\nevery = 1\nshell = \"zsh\"\n",
                "unknown",
            ),
            (&format!("{one}{one}"), "two [[input]] entries"),
            (
                "[[action]]\nname = \"a\"\n[[action]]\nname = \"a\"\n",
                "two [[action]] entries",
            ),
        ] {
            let err = format!("{:#}", parse(bad).unwrap_err());
            assert!(err.contains(message), "{bad}: {err}");
        }
    }

    #[test]
    fn reads_come_from_the_manifest_and_the_inputs_file() {
        let reads = reads();
        let mut names: Vec<&str> = reads.inputs.iter().map(|i| i.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["load", "memory", "workspace"]);
        let load = reads.inputs.iter().find(|i| i.name == "load").unwrap();
        assert_eq!(load.size, 3);
        assert_eq!(reads.actions, ["tape1", "tape2"]);
        // The commented-out entry is no suggestion.
        let suggested: Vec<&str> = reads.suggested.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(suggested, ["load", "workspace", "workspace", "weather"]);
        assert!(reads.has_commands());

        let single = Reads::parse(None, None).unwrap();
        assert!(single.inputs.is_empty() && single.actions.is_empty());
        assert!(!single.has_commands());
        assert_eq!(manifest_path(""), Some("manifest.toml"));
        assert_eq!(
            manifest_path("pack/manifest.toml"),
            Some("pack/manifest.toml")
        );
        assert_eq!(manifest_path("plasma.glsl"), None);

        // An inputs file that phonto cannot read only has no suggestions.
        let broken = Reads::parse(Some(MANIFEST), Some("[[input]\n")).unwrap();
        assert!(broken.suggested.is_empty());
        let bad_every = "[[input]]\nname = \"load\"\ncommand = \"x\"\nevery = -1\n";
        assert!(
            Reads::parse(None, Some(bad_every))
                .unwrap()
                .suggested
                .is_empty()
        );
    }

    #[test]
    fn action_names_cannot_leave_a_comment() {
        let manifest = "[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\n\
                        names = [\"a\\n[[input]]\"]\n";
        assert!(Reads::parse(Some(manifest), None).is_err());
    }

    #[test]
    fn the_template_turns_nothing_on() {
        let text = template("dash", &reads());
        assert!(text.starts_with(&header("dash")), "{text}");
        assert_eq!(parse(&text).unwrap(), Settings::default());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            assert!(line.starts_with('#'), "{line:?} is not a comment");
        }
        // The comments of the inputs file stay with their entries.
        assert!(text.contains(
            "# The load averages of the last 1, 5, and 15 minutes.\n# [[input]]\n# name = \"load\"\n"
        ));
        assert!(text.contains("# Hyprland\n# [[input]]\n# name = \"workspace\"\n"));
        assert!(text.contains("# Sway\n# [[input]]\n# name = \"workspace\"\n"));
        assert!(text.contains("# niri\n# [[input]]\n"));
        // The top of the inputs file, and the inputs it does not read, go.
        assert!(!text.contains("cat inputs.toml"), "{text}");
        assert!(!text.contains("weather"), "{text}");
        // An input without a suggestion, and each action, get an empty entry.
        assert!(text.contains(
            "# Input `memory`: a number, by default 0.\n\n# [[input]]\n# name = \"memory\"\n"
        ));
        assert!(text.contains("# Input `load`: 3 numbers, by default 0, 0, 0.\n"));
        assert!(
            text.contains("# [[action]]\n# name = \"tape1\"\n# label = \"\"\n# command = \"\"\n")
        );
        assert!(text.contains("# name = \"tape2\"\n"));

        // Taking the # away from one entry turns on that entry only.
        let on = text.replace(
            "# [[input]]\n# name = \"load\"\n# command = \"uptime | sed 's/.*: //'\"\n# every = 2\n",
            "[[input]]\nname = \"load\"\ncommand = \"uptime | sed 's/.*: //'\"\nevery = 2\n",
        );
        let settings = parse(&on).unwrap();
        assert_eq!(settings.inputs.len(), 1);
        assert_eq!(settings.inputs[0].command, "uptime | sed 's/.*: //'");
    }

    #[test]
    fn lines_of_the_inputs_file_cannot_end_the_comment() {
        let inputs = "[[input]]\nname = \"load\"\ncommand = \"\"\"\n[[input]]\necho 1\"\"\"\nevery = 1\n\
                      # x\r[[action]]\n";
        let reads = Reads::parse(Some(MANIFEST), Some(inputs)).unwrap();
        let text = template("dash", &reads);
        assert_eq!(parse(&text).unwrap(), Settings::default());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            assert!(line.starts_with('#') && !line.contains('\r'), "{line:?}");
        }
    }

    #[test]
    fn blocks_split_entries_without_blank_lines() {
        let text =
            "# one\n[[input]]\nname = \"a\"\nevery = 1\n# two\n[[input]]\nname = \"b\"\n# end\n";
        let blocks = blocks(text);
        let names: Vec<&str> = blocks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(
            blocks[0].lines,
            ["# one", "[[input]]", "name = \"a\"", "every = 1"]
        );
        assert_eq!(
            blocks[1].lines,
            ["# two", "[[input]]", "name = \"b\"", "# end"]
        );
    }

    #[test]
    fn suggested_commands_are_copied_on_request() {
        let reads = reads();
        let take = |name: &str, pick: Option<u32>| {
            input_entry("dash", &reads, name, None, None, Some(pick)).map_err(|e| format!("{e:#}"))
        };
        let load = take("load", None).unwrap();
        assert_eq!(
            (load.command.as_str(), load.every),
            ("uptime | sed 's/.*: //'", 2.0)
        );
        let err = take("workspace", None).unwrap_err();
        assert!(err.contains("suggests 2 commands for `workspace`. Choose one with --pick"));
        assert!(
            err.contains("1. hyprctl") && err.contains("2. niri"),
            "{err}"
        );
        assert!(
            take("workspace", Some(2))
                .unwrap()
                .command
                .starts_with("niri")
        );
        assert!(
            take("workspace", Some(3))
                .unwrap_err()
                .contains("--pick takes 1 to 2")
        );
        assert!(
            take("memory", None)
                .unwrap_err()
                .contains("suggests no command for `memory`")
        );

        let set = |name: &str, every: Option<f64>| {
            input_entry("dash", &reads, name, Some("echo 1".into()), every, None).unwrap()
        };
        // `every` comes from the first suggestion, or else is 1.
        assert_eq!(set("workspace", None).every, 0.2);
        assert_eq!(set("memory", None).every, 1.0);
        assert_eq!(set("memory", Some(5.0)).every, 5.0);
    }

    #[test]
    fn names_that_the_wallpaper_does_not_read_are_rejected() {
        let reads = reads();
        let err = input_entry("dash", &reads, "weather", Some("x".into()), None, None).unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.contains("`dash` has no input named `weather`. Its inputs are"),
            "{err}"
        );
        let err = check_action_name("dash", &reads, "tape9").unwrap_err();
        assert!(format!("{err:#}").contains("Its actions are tape1, tape2"));
        let none = Reads::default();
        let err = check_input_name("plasma", &none, "load").unwrap_err();
        assert!(format!("{err:#}").contains("reads no inputs"));
        let typo = Settings {
            inputs: vec![input("lod", "uptime")],
            actions: Vec::new(),
        };
        assert!(check_names("dash", &typo, &reads).is_err());
    }

    #[test]
    fn changes_keep_the_header_and_remove_an_empty_file() {
        let dir = temp_dir("change");
        let file = path_in(&dir, "dash");
        change(&file, "dash", |s| s.inputs.push(input("load", "uptime"))).unwrap();
        change(&file, "dash", |s| {
            set_action(s, "tape1", Some("Firefox".into()), None)
        })
        .unwrap();
        change(&file, "dash", |s| {
            set_action(s, "tape1", None, Some("firefox".into()))
        })
        .unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with(&header("dash")), "{text}");
        let settings = parse(&text).unwrap();
        assert_eq!(settings.inputs, [input("load", "uptime")]);
        assert_eq!(settings.actions[0].label.as_deref(), Some("Firefox"));
        assert_eq!(settings.actions[0].command.as_deref(), Some("firefox"));

        change(&file, "dash", |s| s.inputs.retain(|i| i.name != "load")).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with(&header("dash")));
        assert_eq!(parse(&text).unwrap().inputs, []);
        // A change that sets an invalid `every` writes nothing.
        let bad = change(&file, "dash", |s| {
            s.inputs.push(InputCommand {
                every: 0.0,
                ..input("load", "x")
            })
        });
        assert!(bad.is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), text);

        change(&file, "dash", |s| s.actions.clear()).unwrap();
        assert!(!file.exists());
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_replace_or_follow_the_configuration_in_order() {
        let layers = vec![
            (
                "a".to_string(),
                Settings {
                    inputs: vec![input("load", "a"), input("extra", "a")],
                    actions: vec![action("tape1", "a")],
                },
            ),
            (
                "b".to_string(),
                Settings {
                    inputs: vec![input("load", "b")],
                    actions: Vec::new(),
                },
            ),
        ];
        let config_inputs = vec![
            input("load", "config"),
            input("cpu", "config"),
            input("load", "again"),
        ];
        let (inputs, actions) = apply(
            config_inputs,
            vec![action("tape1", "config"), action("tape2", "config")],
            &layers,
        );
        let inputs: Vec<(&str, &str)> = inputs
            .iter()
            .map(|i| (i.name.as_str(), i.command.as_str()))
            .collect();
        // The later wallpaper wins, and no entry of config.toml stays for a
        // name that the settings set.
        assert_eq!(inputs, [("cpu", "config"), ("extra", "a"), ("load", "b")]);
        let actions: Vec<(&str, Option<&str>)> = actions
            .iter()
            .map(|a| (a.name.as_str(), a.command.as_deref()))
            .collect();
        assert_eq!(actions, [("tape2", Some("config")), ("tape1", Some("a"))]);
    }

    fn gallery_folder(root: &Path, id: &str, run: &str) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let record = gallery::Record {
            id: id.to_string(),
            version: "v1".to_string(),
            gallery: "http://localhost:8000".to_string(),
            title: String::new(),
            author: String::new(),
            run: run.to_string(),
            inputs: None,
        };
        fs::write(dir.join(".gallery.toml"), toml::to_string(&record).unwrap()).unwrap();
        dir
    }

    #[test]
    fn the_wallpapers_that_play_bring_their_settings() {
        let root = temp_dir("merge");
        let config_dir = root.join("config");
        let pack = gallery_folder(&root, "dash", "");
        let single = gallery_folder(&root, "plasma", "plasma.glsl");
        let write = |id: &str, text: &str| {
            let file = path_in(&config_dir, id);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        };
        write(
            "dash",
            "[[input]]\nname = \"load\"\ncommand = \"dash\"\nevery = 1\n",
        );
        write(
            "plasma",
            "[[action]]\nname = \"tape1\"\ncommand = \"plasma\"\n",
        );

        let pack_path = pack.to_string_lossy().into_owned();
        let single_path = single.join("plasma.glsl").to_string_lossy().into_owned();
        let paths = [
            pack_path.as_str(),
            single_path.as_str(),
            pack_path.as_str(),
            "https://example.com/w/dash",
            "/no/such/folder",
        ];
        let (inputs, actions) = merge_in(
            &config_dir,
            vec![input("load", "config")],
            Vec::new(),
            &paths,
        )
        .unwrap();
        assert_eq!(inputs, [input("load", "dash")]);
        assert_eq!(actions, [action("tape1", "plasma")]);

        // A file that phonto cannot read stops the wallpaper, with its path.
        write("dash", "[[input]]\nname = \"load\"\n");
        let err = merge_in(&config_dir, Vec::new(), Vec::new(), &paths).unwrap_err();
        assert!(
            format!("{err:#}").contains("wallpapers/dash.toml"),
            "{err:#}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn json_has_the_documented_keys() {
        let reads = reads();
        let settings = Settings {
            inputs: vec![input("load", "mine")],
            actions: Vec::new(),
        };
        let config: Config = toml::from_str(
            "[[input]]\nname = \"memory\"\ncommand = \"free\"\nevery = 5\n\
             [[action]]\nname = \"tape1\"\nlabel = \"Firefox\"\ncommand = \"firefox\"\n",
        )
        .unwrap();
        let (inputs, actions) = reads.views(Some((&settings, &config)));
        let json = serde_json::to_value(SettingsJson {
            id: "dash",
            title: Some("Dashboard".to_string()),
            settings: None,
            inputs,
            actions,
        })
        .unwrap();
        let keys = |v: &serde_json::Value| {
            let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(
            keys(&json),
            ["actions", "id", "inputs", "settings", "title"]
        );
        let by_name = |list: &serde_json::Value, name: &str| {
            list.as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == name)
                .cloned()
                .unwrap()
        };
        let load = by_name(&json["inputs"], "load");
        assert_eq!(
            keys(&load),
            ["configured", "default", "name", "size", "suggested"]
        );
        assert_eq!(load["default"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(
            load["configured"],
            serde_json::json!({ "command": "mine", "every": 1.0, "from": "wallpaper" })
        );
        assert_eq!(
            load["suggested"],
            serde_json::json!([{ "command": "uptime | sed 's/.*: //'", "every": 2.0 }])
        );
        assert_eq!(
            by_name(&json["inputs"], "memory")["configured"]["from"],
            "config"
        );
        assert!(by_name(&json["inputs"], "workspace")["configured"].is_null());
        let tape1 = by_name(&json["actions"], "tape1");
        assert_eq!(keys(&tape1), ["configured", "name"]);
        assert_eq!(
            tape1["configured"],
            serde_json::json!({ "label": "Firefox", "command": "firefox", "from": "config" })
        );
        assert!(by_name(&json["actions"], "tape2")["configured"].is_null());

        // Without settings, as in `phonto gallery show`, there is no
        // `configured` key.
        let (inputs, actions) = reads.views(None);
        let input = serde_json::to_value(&inputs[0]).unwrap();
        assert_eq!(keys(&input), ["default", "name", "size", "suggested"]);
        assert_eq!(keys(&serde_json::to_value(&actions[0]).unwrap()), ["name"]);
    }
}
