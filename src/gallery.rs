//! Wallpapers from a gallery: Pinakothiki, or another site with the same
//! layout. `phonto install` keeps a wallpaper in the data folder. A wallpaper
//! that is not installed downloads to a temporary folder, so you can try it
//! first.
//!
//! The gallery serves `index.json`, which lists each wallpaper with its
//! files and their SHA-256, and the files under `packs/<id>/`. phonto checks
//! every path, size, and checksum before it runs a wallpaper. A wallpaper can
//! suggest input commands, but phonto never turns them on: commands come only
//! from the user, in config.toml or in the settings of the wallpaper.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::settings::{self, Reads};

/// The gallery that phonto uses, unless `PHONTO_GALLERY` or the `gallery` key
/// of the configuration names another one.
pub const DEFAULT_GALLERY: &str = "https://pinakothiki.museslabs.org";
/// A page on this host names a wallpaper too, when phonto uses another
/// gallery.
const DEFAULT_HOST: &str = "pinakothiki.museslabs.org";
/// The prefix that names a wallpaper of the gallery: `pinakothiki:koi`.
const SCHEME: &str = "pinakothiki:";
/// The file in the folder of a wallpaper that says what phonto downloaded.
/// phonto writes it last, so a folder without it is not complete.
const RECORD: &str = ".gallery.toml";
/// The index format that this phonto reads.
const FORMAT: u64 = 1;
/// The most that one wallpaper can make phonto download.
const MAX_FILES: usize = 512;
const MAX_BYTES: u64 = 100 * 1024 * 1024;
/// The index lists every wallpaper of the gallery, so it gets its own limit.
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
/// How many folders up from a `.glsl` file phonto looks for its record.
const MAX_RUN_DEPTH: usize = 16;

/// A wallpaper in `index.json`. phonto ignores the keys that it does not use,
/// so the gallery can add keys.
#[derive(Debug, Default, Deserialize)]
struct Entry {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    license: Option<String>,
    /// `""` if the folder is a pack, or else the `.glsl` file to run.
    #[serde(default)]
    run: String,
    /// A file with `[[input]]` entries that the user can copy.
    #[serde(default)]
    inputs: Option<String>,
    /// Changes whenever a file changes.
    version: String,
    /// The bytes of all the files.
    #[serde(default)]
    size: Option<u64>,
    files: Vec<FileEntry>,
    /// The page of the wallpaper and an image of it, as absolute URLs or as
    /// paths on the site.
    #[serde(default)]
    page: Option<String>,
    #[serde(default)]
    preview: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FileEntry {
    path: String,
    size: u64,
    sha256: String,
}

/// `.gallery.toml`: what phonto downloaded into a folder, and how to run it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub version: String,
    pub gallery: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
}

impl Record {
    fn new(entry: &Entry, gallery: &str) -> Self {
        Record {
            id: entry.id.clone(),
            version: entry.version.clone(),
            gallery: gallery.to_string(),
            title: entry.title.clone().unwrap_or_default(),
            author: entry.author.clone().unwrap_or_default(),
            run: entry.run.clone(),
            inputs: entry.inputs.clone(),
        }
    }
}

/// A wallpaper of the gallery in a folder on this computer: installed, or a
/// temporary copy.
pub struct Local {
    pub dir: PathBuf,
    pub record: Record,
}

impl Local {
    /// What the wallpaper reads, from its files.
    pub fn reads(&self) -> anyhow::Result<Reads> {
        Reads::from_folder(&self.dir, &self.record.run, self.record.inputs.as_deref())
    }
}

/// The path to run `s` from, if `s` names a wallpaper of the gallery: the
/// installed copy, or else a temporary copy, which phonto downloads when it
/// does not have this version yet. `None` if `s` is something else, such as a
/// file or a video URL.
pub fn resolve(s: &str, config_gallery: Option<&str>) -> anyhow::Result<Option<String>> {
    let base = base_url(config_gallery);
    let Some(id) = parse_reference(s, base.as_deref().ok())? else {
        return Ok(None);
    };
    if let Some(local) = find_installed(&id) {
        log::info!("playing `{id}` from {}", local.dir.display());
        return Ok(Some(run_path(&local.dir, &local.record.run)));
    }
    let gallery = Gallery::new(base?);
    let entry = gallery.entry(&id)?;
    let dir = preview(&gallery, &entry)?;
    let commands = Reads::from_folder(&dir, &entry.run, entry.inputs.as_deref())
        .is_ok_and(|reads| reads.has_commands());
    let settings = if commands {
        format!(", and `phonto settings {id}` to set the commands that it reads")
    } else {
        String::new()
    };
    eprintln!(
        "`{id}` is not installed, so phonto runs a temporary copy from {}. \
         Run `phonto install {id}` to keep it{settings}.",
        dir.display()
    );
    Ok(Some(run_path(&dir, &entry.run)))
}

/// The id of the wallpaper of the gallery at `path`, as phonto runs it: its
/// folder, its `.glsl` file, or its `manifest.toml`. Its settings apply
/// while it plays.
pub fn identify(path: &str) -> Option<String> {
    if crate::config::is_url(path) {
        return None;
    }
    let path = Path::new(path);
    for dir in path.ancestors().take(MAX_RUN_DEPTH) {
        let Some(record) = read_record(dir) else {
            continue;
        };
        // The id names the settings file, so it must be a plain name.
        check_id(&record.id).ok()?;
        let runs = if record.run.is_empty() {
            path == dir || path == dir.join(crate::procedural::MANIFEST)
        } else {
            path == dir.join(&record.run)
        };
        return runs.then_some(record.id);
    }
    None
}

/// The id that `wallpaper` names, for the commands that take a wallpaper of
/// the gallery: a reference, or a bare id.
pub fn wallpaper_id(wallpaper: &str, config_gallery: Option<&str>) -> anyhow::Result<String> {
    // Only a page URL needs the gallery, and a bare id works without it.
    let base = base_url(config_gallery).ok();
    id_argument(wallpaper, base.as_deref())
}

/// A copy of a wallpaper on this computer, for `phonto settings`: the
/// installed copy, or else the newest temporary copy, or else a new
/// temporary copy.
pub fn local(wallpaper: &str, config_gallery: Option<&str>) -> anyhow::Result<Local> {
    let base = base_url(config_gallery);
    let id = id_argument(wallpaper, base.as_deref().ok())?;
    if let Some(local) = find_installed(&id).or_else(|| find_preview(&id)) {
        return Ok(local);
    }
    let gallery = Gallery::new(base?);
    let entry = gallery.entry(&id)?;
    let dir = preview(&gallery, &entry)?;
    Ok(Local {
        dir,
        record: Record::new(&entry, &gallery.base),
    })
}

/// How `phonto install` left a wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Installed,
    Updated,
    UpToDate,
}

/// `phonto install --json`.
#[derive(Serialize)]
struct InstallJson {
    id: String,
    status: Status,
    path: String,
    run: String,
}

/// `phonto uninstall --json`.
#[derive(Serialize)]
struct UninstallJson {
    id: String,
    status: &'static str,
    path: String,
    settings_removed: bool,
}

/// `phonto install`: downloads a wallpaper into the data folder, or updates
/// it there.
pub fn install(wallpaper: &str, config_gallery: Option<&str>, json: bool) -> anyhow::Result<()> {
    let gallery = Gallery::new(base_url(config_gallery)?);
    let id = id_argument(wallpaper, Some(&gallery.base))?;
    let entry = gallery.entry(&id)?;
    let root = installed_root()?;
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    let dir = root.join(&id);
    let installed = dir.join(RECORD).is_file();
    // phonto replaces only the folders that it made.
    if !installed && fs::symlink_metadata(&dir).is_ok() {
        bail!(
            "{} exists, but phonto did not install it. Move it away, then try again",
            dir.display()
        );
    }
    let status = if is_complete(&dir, &gallery.base, &entry) {
        Status::UpToDate
    } else {
        log::info!("downloading `{id}` from {}", gallery.base);
        fetch_into(&entry, &gallery.base, &dir, |file| {
            gallery.open(&id, &file.path)
        })?;
        if installed {
            Status::Updated
        } else {
            Status::Installed
        }
    };
    let run = format!("{SCHEME}{id}");
    if json {
        return print_json(&InstallJson {
            id,
            status,
            path: dir.to_string_lossy().into_owned(),
            run,
        });
    }
    let name = describe(&id, entry.title.as_deref(), entry.author.as_deref());
    match status {
        Status::UpToDate => println!("{name} is up to date in {}", dir.display()),
        Status::Updated => println!("Updated {name} in {}", dir.display()),
        Status::Installed => println!("Installed {name} in {}", dir.display()),
    }
    println!("Play it with: phonto {run}");
    match Reads::from_folder(&dir, &entry.run, entry.inputs.as_deref()) {
        Ok(reads) => print_commands_hint(&id, &reads),
        Err(err) => log::warn!("{err:#}"),
    }
    Ok(())
}

/// Tells how to set the commands of a wallpaper that reads inputs or has
/// actions. phonto turns none of them on by itself.
fn print_commands_hint(id: &str, reads: &Reads) {
    let what = match (reads.inputs.is_empty(), reads.actions.is_empty()) {
        (false, false) => "inputs and actions",
        (false, true) => "inputs",
        (true, false) => "actions",
        (true, true) => return,
    };
    println!("It has {what}, and phonto runs no command for them until you set one.");
    println!("See them, with the suggested commands: phonto settings {id}");
    println!("Set them in your editor: phonto settings {id} edit");
    if !reads.suggested.is_empty() {
        println!("Or take a suggested command: phonto settings {id} set-input <name> --suggested");
    }
}

/// `phonto uninstall`: removes a wallpaper that `phonto install` downloaded.
/// Its settings stay, unless `remove_settings`.
pub fn uninstall(
    wallpaper: &str,
    config_gallery: Option<&str>,
    remove_settings: bool,
    json: bool,
) -> anyhow::Result<()> {
    let id = wallpaper_id(wallpaper, config_gallery)?;
    let dir = installed_root()?.join(&id);
    if fs::symlink_metadata(&dir).is_err() {
        bail!("`{id}` is not installed");
    }
    if !dir.join(RECORD).is_file() {
        bail!(
            "phonto did not install {}, so it does not remove it",
            dir.display()
        );
    }
    fs::remove_dir_all(&dir).with_context(|| format!("failed to remove {}", dir.display()))?;
    let settings_removed = remove_settings && settings::remove(&id)?;
    if json {
        return print_json(&UninstallJson {
            id,
            status: "removed",
            path: dir.to_string_lossy().into_owned(),
            settings_removed,
        });
    }
    println!("Removed `{id}` from {}", dir.display());
    let file = settings::path(&id);
    if settings_removed {
        println!("Removed its settings, {}", file.display());
    } else if file.is_file() {
        println!(
            "Its settings stay in {}, for when you install it again. \
             `phonto settings {id} reset` removes them.",
            file.display()
        );
    }
    Ok(())
}

/// A wallpaper of the gallery, in `phonto gallery list --json`.
#[derive(Debug, Serialize)]
struct Listed {
    id: String,
    title: Option<String>,
    author: Option<String>,
    summary: Option<String>,
    tags: Vec<String>,
    license: Option<String>,
    version: String,
    size: u64,
    page: Option<String>,
    preview: Option<String>,
    installed: bool,
    installed_version: Option<String>,
    update: bool,
}

impl Listed {
    fn new(gallery: &str, entry: &Entry, installed: Option<&Record>) -> Self {
        let installed_version = installed.map(|r| r.version.clone());
        Listed {
            id: entry.id.clone(),
            title: entry.title.clone(),
            author: entry.author.clone(),
            summary: entry.summary.clone(),
            tags: entry.tags.clone(),
            license: entry.license.clone(),
            version: entry.version.clone(),
            size: entry.size.unwrap_or_else(|| {
                (entry.files.iter()).fold(0, |total, f| total.saturating_add(f.size))
            }),
            page: entry.page.as_deref().and_then(|p| absolute(gallery, p)),
            preview: entry.preview.as_deref().and_then(|p| absolute(gallery, p)),
            installed: installed.is_some(),
            update: installed_version
                .as_ref()
                .is_some_and(|v| *v != entry.version),
            installed_version,
        }
    }

    /// One line for the human output.
    fn line(&self) -> (String, String, String) {
        let note = match (self.installed, self.update) {
            (true, true) => "installed, update available",
            (true, false) => "installed",
            _ => "",
        };
        let text = describe_title(self.title.as_deref(), self.author.as_deref());
        (self.id.clone(), text, note.to_string())
    }
}

#[derive(Serialize)]
struct ListingJson<'a> {
    gallery: &'a str,
    wallpapers: Vec<Listed>,
}

/// `phonto gallery list`, and `phonto gallery search` with `words`: the
/// wallpapers that have all the words in their id, title, author, summary,
/// or tags.
pub fn browse(words: &[String], config_gallery: Option<&str>, json: bool) -> anyhow::Result<()> {
    let gallery = Gallery::new(base_url(config_gallery)?);
    let words: Vec<String> = words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    let wallpapers: Vec<Listed> = gallery
        .entries()?
        .iter()
        .filter(|entry| has_words(entry, &words))
        .map(|entry| {
            let installed = find_installed(&entry.id);
            Listed::new(&gallery.base, entry, installed.as_ref().map(|l| &l.record))
        })
        .collect();
    if json {
        return print_json(&ListingJson {
            gallery: &gallery.base,
            wallpapers,
        });
    }
    if wallpapers.is_empty() {
        if words.is_empty() {
            println!("The gallery at {} has no wallpapers.", gallery.base);
        } else {
            println!(
                "No wallpaper in the gallery at {} has all of: {}",
                gallery.base,
                words.join(" ")
            );
        }
        return Ok(());
    }
    print_rows(&wallpapers.iter().map(Listed::line).collect::<Vec<_>>());
    println!(
        "See one with `phonto gallery show <id>`, try it with `phonto pinakothiki:<id>`, \
         and keep it with `phonto install <id>`."
    );
    Ok(())
}

/// True if each word is in the id, the title, the author, the summary, or
/// the tags. The words are lowercase.
fn has_words(entry: &Entry, words: &[String]) -> bool {
    let fields = [&entry.title, &entry.author, &entry.summary]
        .into_iter()
        .flatten()
        .chain(&entry.tags)
        .chain([&entry.id]);
    // A line break between the fields, so a word cannot span two of them.
    let text = fields
        .map(|f| f.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    words.iter().all(|w| text.contains(w.as_str()))
}

/// `phonto gallery show --json`.
#[derive(Serialize)]
struct ShowJson<'a> {
    #[serde(flatten)]
    listed: Listed,
    files: &'a [FileEntry],
    inputs: Vec<settings::InputView>,
    actions: Vec<settings::ActionView>,
}

/// `phonto gallery show`: one wallpaper of the gallery, and what it reads.
pub fn show(wallpaper: &str, config_gallery: Option<&str>, json: bool) -> anyhow::Result<()> {
    let gallery = Gallery::new(base_url(config_gallery)?);
    let id = id_argument(wallpaper, Some(&gallery.base))?;
    let entry = gallery.entry(&id)?;
    // A copy of this version on disk has the files already.
    let reads = match copy_of(&gallery.base, &entry) {
        Some(dir) => Reads::from_folder(&dir, &entry.run, entry.inputs.as_deref())?,
        None => {
            let manifest = settings::manifest_path(&entry.run)
                .map(|path| gallery.read(&entry, path))
                .transpose()?;
            let inputs = (entry.inputs.as_deref())
                .map(|path| gallery.read(&entry, path))
                .transpose()?;
            Reads::parse(manifest.as_deref(), inputs.as_deref())?
        }
    };
    let installed = find_installed(&id);
    let listed = Listed::new(&gallery.base, &entry, installed.as_ref().map(|l| &l.record));
    let (inputs, actions) = reads.views(None);
    if json {
        return print_json(&ShowJson {
            listed,
            files: &entry.files,
            inputs,
            actions,
        });
    }
    println!(
        "{}",
        describe(&id, listed.title.as_deref(), listed.author.as_deref())
    );
    if let Some(summary) = &listed.summary {
        println!("{}", clean(summary));
    }
    let mut facts = Vec::new();
    if !listed.tags.is_empty() {
        let tags: Vec<String> = listed.tags.iter().map(|t| clean(t)).collect();
        facts.push(format!("Tags: {}", tags.join(", ")));
    }
    if let Some(license) = &listed.license {
        facts.push(format!("License: {}", clean(license)));
    }
    let files = match entry.files.len() {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    };
    facts.push(format!("{} in {files}", human_size(listed.size)));
    println!("{}.", facts.join(". "));
    if let Some(page) = &listed.page {
        println!("Page: {}", clean(page));
    }
    match &installed {
        Some(local) if listed.update => println!(
            "Installed in {}. The gallery has a newer version: `phonto install {id}` updates it.",
            local.dir.display()
        ),
        Some(local) => println!("Installed in {}.", local.dir.display()),
        None => {
            println!("Try it with `phonto {SCHEME}{id}`, and keep it with `phonto install {id}`.")
        }
    }
    settings::print_views(&inputs, &actions);
    Ok(())
}

/// A folder with this version of the wallpaper, from this gallery.
fn copy_of(gallery: &str, entry: &Entry) -> Option<PathBuf> {
    let same = |r: &Record| r.version == entry.version && r.gallery == gallery;
    if let Some(local) = find_installed(&entry.id)
        && same(&local.record)
    {
        return Some(local.dir);
    }
    let dir =
        (previews_root().ok()?).join(format!("{}-{}", entry.id, version_slug(&entry.version)));
    read_record(&dir).filter(same).map(|_| dir)
}

/// An installed wallpaper, in `phonto list --json`.
#[derive(Serialize)]
struct InstalledJson {
    id: String,
    title: Option<String>,
    author: Option<String>,
    version: String,
    gallery: String,
    path: String,
    run: String,
    settings: Option<String>,
    has_inputs: bool,
}

#[derive(Serialize)]
struct InstalledListJson {
    wallpapers: Vec<InstalledJson>,
}

/// `phonto list`: the installed wallpapers.
pub fn list_installed(json: bool) -> anyhow::Result<()> {
    let root = installed_root()?;
    let wallpapers: Vec<InstalledJson> = installed(&root)
        .into_iter()
        .map(|local| {
            let id = local.record.id.clone();
            let file = settings::path(&id);
            // Without a manifest that phonto can read, the inputs file tells.
            let has_inputs = local
                .reads()
                .map(|reads| !reads.inputs.is_empty())
                .unwrap_or(local.record.inputs.is_some());
            InstalledJson {
                title: non_empty(&local.record.title),
                author: non_empty(&local.record.author),
                version: local.record.version,
                gallery: local.record.gallery,
                path: local.dir.to_string_lossy().into_owned(),
                run: format!("{SCHEME}{id}"),
                settings: file.is_file().then(|| file.to_string_lossy().into_owned()),
                has_inputs,
                id,
            }
        })
        .collect();
    if json {
        return print_json(&InstalledListJson { wallpapers });
    }
    if wallpapers.is_empty() {
        println!(
            "No wallpapers are installed. `phonto gallery list` shows the wallpapers of the gallery."
        );
        return Ok(());
    }
    let rows: Vec<_> = wallpapers
        .iter()
        .map(|w| {
            let text = describe_title(w.title.as_deref(), w.author.as_deref());
            let note = if w.settings.is_some() {
                "has settings"
            } else {
                ""
            };
            (w.id.clone(), text, note.to_string())
        })
        .collect();
    print_rows(&rows);
    println!("They are in {}.", root.display());
    Ok(())
}

/// The wallpapers that phonto installed in `root`, by id.
fn installed(root: &Path) -> Vec<Local> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut all: Vec<Local> = entries
        .flatten()
        .filter_map(|e| find_installed_in(root, e.file_name().to_str()?))
        .collect();
    all.sort_by(|a, b| a.record.id.cmp(&b.record.id));
    all
}

/// Prints one line for each wallpaper: its id, its title, and a note.
fn print_rows(rows: &[(String, String, String)]) {
    let width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    for (id, text, note) in rows {
        let line = format!("{id:<width$}  {text}");
        if note.is_empty() {
            println!("{}", line.trim_end());
        } else {
            println!("{line}  ({note})");
        }
    }
}

/// Prints one JSON document on stdout, for `--json`.
pub fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// The gallery to use: `PHONTO_GALLERY`, or else the `gallery` key of the
/// configuration, or else Pinakothiki.
fn base_url(config_gallery: Option<&str>) -> anyhow::Result<String> {
    let env = std::env::var("PHONTO_GALLERY").ok();
    pick_base(env.as_deref(), config_gallery)
}

/// The base URL from the environment or the configuration, without a slash at
/// the end. An empty value counts as no value.
fn pick_base(env: Option<&str>, config: Option<&str>) -> anyhow::Result<String> {
    let given = |v: &&str| !v.trim().is_empty();
    let (value, source) = if let Some(v) = env.filter(given) {
        (v, "PHONTO_GALLERY")
    } else if let Some(v) = config.filter(given) {
        (v, "`gallery` in the configuration")
    } else {
        return Ok(DEFAULT_GALLERY.to_string());
    };
    normalize_base(value).with_context(|| {
        format!(
            "{source} must be an http or https URL, such as {DEFAULT_GALLERY}, but it is `{value}`"
        )
    })
}

fn normalize_base(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.contains(|c: char| c.is_whitespace() || c == '?' || c == '#') {
        return None;
    }
    split_url(value)?;
    let (scheme, rest) = value.split_once("://")?;
    Some(format!("{}://{rest}", scheme.to_ascii_lowercase()))
}

/// `url` from the index as an absolute URL: as it is with http or https, or
/// else on the site of the gallery. `None` for any other scheme.
fn absolute(gallery: &str, url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if split_url(url).is_some() {
        return Some(url.to_string());
    }
    let (scheme, rest) = gallery.split_once("://")?;
    if let Some(other) = url.strip_prefix("//") {
        return Some(format!("{scheme}://{other}"));
    }
    // A colon before the first slash starts another scheme, as in `data:`.
    if url
        .split('/')
        .next()
        .is_some_and(|first| first.contains(':'))
    {
        return None;
    }
    if url.starts_with('/') {
        let host = rest.split('/').next().unwrap_or(rest);
        Some(format!("{scheme}://{host}{url}"))
    } else {
        Some(format!("{gallery}/{url}"))
    }
}

/// The id of the wallpaper that `s` names: `pinakothiki:<id>`, or the URL of
/// its page on the gallery (`gallery`, if phonto knows it) or on Pinakothiki.
/// `None` if `s` names something else. Other URLs keep their meaning, so
/// `https://example.com/w/x` stays a video URL.
fn parse_reference(s: &str, gallery: Option<&str>) -> anyhow::Result<Option<String>> {
    let s = s.trim();
    if let Some(prefix) = s.get(..SCHEME.len())
        && prefix.eq_ignore_ascii_case(SCHEME)
    {
        let id = &s[SCHEME.len()..];
        check_id(id)?;
        return Ok(Some(id.to_string()));
    }
    let Some((origin, path)) = split_url(s) else {
        return Ok(None);
    };
    let Some(id) = page_id(path) else {
        return Ok(None);
    };
    let ours = origin.host == DEFAULT_HOST
        || gallery
            .and_then(split_url)
            .is_some_and(|(gallery, _)| gallery == origin);
    if !ours {
        return Ok(None);
    }
    check_id(id)?;
    Ok(Some(id.to_string()))
}

/// The id that `install` and `uninstall` take: a reference, or a bare id.
fn id_argument(s: &str, gallery: Option<&str>) -> anyhow::Result<String> {
    if let Some(id) = parse_reference(s, gallery)? {
        return Ok(id);
    }
    let s = s.trim();
    if crate::config::is_url(s) {
        bail!(
            "`{s}` is not the page of a wallpaper in the gallery at {}",
            gallery.unwrap_or(DEFAULT_GALLERY)
        );
    }
    check_id(s)?;
    Ok(s.to_string())
}

/// Checks that `id` matches `[a-z0-9][a-z0-9-]{0,63}`.
fn check_id(id: &str) -> anyhow::Result<()> {
    let b = id.as_bytes();
    let plain = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !((1..=64).contains(&b.len()) && plain(&b[0]) && b.iter().all(|c| plain(c) || *c == b'-')) {
        bail!(
            "`{id}` is not a wallpaper id: an id has 1 to 64 lowercase letters, digits, and \
             dashes, and does not start with a dash"
        );
    }
    Ok(())
}

/// The parts of an http or https URL that make its origin.
#[derive(Debug, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

/// The origin and the path of an http or https URL, without the query or the
/// fragment.
fn split_url(url: &str) -> Option<(Origin, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let (authority, rest) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    // A user name can hide the real host, as in `https://gallery@evil.example/`.
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rfind(':') {
        // A colon inside the brackets of an IPv6 address is not a port.
        Some(i) if !authority[i..].contains(']') => {
            (&authority[..i], authority[i + 1..].parse().ok()?)
        }
        _ => (authority, default_port),
    };
    if host.is_empty() {
        return None;
    }
    let path = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let host = host.to_ascii_lowercase();
    Some((Origin { scheme, host, port }, path))
}

/// The id in the path of a page: `/w/<id>`, with or without a slash at the
/// end.
fn page_id(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/w/")?;
    let id = id.strip_suffix('/').unwrap_or(id);
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// Where `phonto install` keeps wallpapers.
fn installed_root() -> anyhow::Result<PathBuf> {
    installed_root_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
        .context("cannot find the data folder, because neither XDG_DATA_HOME nor HOME is set")
}

/// `$XDG_DATA_HOME/phonto/wallpapers`, or `~/.local/share/phonto/wallpapers`.
/// macOS uses the same folder, as it uses `~/.config/phonto` for the
/// configuration. The XDG specification ignores a relative XDG_DATA_HOME.
fn installed_root_from(
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let data = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(data.join("phonto").join("wallpapers"))
}

/// The installed copy of `id`, if phonto installed it completely.
fn find_installed(id: &str) -> Option<Local> {
    find_installed_in(&installed_root().ok()?, id)
}

fn find_installed_in(root: &Path, id: &str) -> Option<Local> {
    check_id(id).ok()?;
    let dir = root.join(id);
    let record = read_record(&dir)?;
    let runs = record.run.is_empty() || check_path(&record.run).is_ok();
    (record.id == id && runs).then_some(Local { dir, record })
}

/// The newest complete temporary copy of `id`.
fn find_preview(id: &str) -> Option<Local> {
    let root = previews_root().ok()?;
    let copies = fs::read_dir(&root).ok()?.flatten().filter_map(|e| {
        let name = e.file_name().into_string().ok()?;
        // `<id>-<version>`, where the version has no dash.
        let version = name.strip_prefix(id)?.strip_prefix('-')?;
        if version.contains('-') {
            return None;
        }
        let dir = e.path();
        let record = read_record(&dir).filter(|r| r.id == id)?;
        let time = fs::metadata(dir.join(RECORD))
            .and_then(|m| m.modified())
            .ok()?;
        Some((time, Local { dir, record }))
    });
    copies.max_by_key(|(time, _)| *time).map(|(_, local)| local)
}

/// `phonto-gallery` in the temporary folder, where wallpapers that are not
/// installed download. Other users can write to the temporary folder, so
/// phonto makes this folder private, and refuses one that someone else made.
fn previews_root() -> anyhow::Result<PathBuf> {
    let root = std::env::temp_dir().join("phonto-gallery");
    if let Err(err) = fs::DirBuilder::new().mode(0o700).create(&root)
        && err.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(err).with_context(|| format!("failed to create {}", root.display()));
    }
    let meta = fs::symlink_metadata(&root)
        .with_context(|| format!("failed to read {}", root.display()))?;
    // SAFETY: getuid only reads the user id of this process.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        bail!(
            "{} belongs to another user, or other users can open it. Remove it, or set TMPDIR \
             to a private folder",
            root.display()
        );
    }
    Ok(root)
}

/// The temporary copy of this version of a wallpaper. phonto downloads it if
/// it does not have it yet.
fn preview(gallery: &Gallery, entry: &Entry) -> anyhow::Result<PathBuf> {
    let name = format!("{}-{}", entry.id, version_slug(&entry.version));
    let dir = previews_root()?.join(name);
    if !is_complete(&dir, &gallery.base, entry) {
        log::info!("downloading `{}` from {}", entry.id, gallery.base);
        fetch_into(entry, &gallery.base, &dir, |file| {
            gallery.open(&entry.id, &file.path)
        })?;
    }
    Ok(dir)
}

/// The version as part of a folder name. A plain version stays as it is.
/// Any other string, or one with a dash, becomes the start of its SHA-256, so
/// `<id>-<version>` never names two wallpapers.
fn version_slug(version: &str) -> String {
    let plain = version.len() <= 64
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_');
    if plain && !version.is_empty() {
        version.to_string()
    } else {
        hex(&Sha256::digest(version.as_bytes()))[..16].to_string()
    }
}

fn run_path(dir: &Path, run: &str) -> String {
    let path = if run.is_empty() {
        dir.to_path_buf()
    } else {
        dir.join(run)
    };
    path.to_string_lossy().into_owned()
}

/// A gallery, and the HTTP client that reads it.
struct Gallery {
    base: String,
    agent: ureq::Agent,
}

impl Gallery {
    fn new(base: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(concat!("phonto/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            // A limit on each read, not on the whole download, so a slow
            // connection can still download a large wallpaper.
            .timeout_per_read(Some(Duration::from_secs(30)))
            // A gallery on https stays on https, also through redirects.
            .https_only(base.starts_with("https://"))
            .build()
            .into();
        Gallery { base, agent }
    }

    fn get(&self, url: &str) -> anyhow::Result<ureq::Body> {
        let response = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("failed to download {url}"))?;
        Ok(response.into_body())
    }

    /// The wallpapers in the index, as JSON.
    fn index(&self) -> anyhow::Result<Vec<Value>> {
        let url = format!("{}/index.json", self.base);
        let json = self
            .get(&url)?
            .into_with_config()
            .limit(MAX_INDEX_BYTES)
            .read_to_vec()
            .with_context(|| format!("failed to download {url}"))?;
        parse_index(&json, &self.base)
    }

    /// The entry of `id` in the index, after the checks.
    fn entry(&self, id: &str) -> anyhow::Result<Entry> {
        find_entry(&self.index()?, id)?
            .ok_or_else(|| anyhow!("no wallpaper named `{id}` in the gallery at {}", self.base))
    }

    /// Every entry of the index that phonto can read, for a list. It skips
    /// the others with a warning. Only a download checks the files.
    fn entries(&self) -> anyhow::Result<Vec<Entry>> {
        let index = self.index()?;
        let entries = index.iter().filter_map(|value| {
            let entry = Entry::deserialize(value)
                .map_err(anyhow::Error::from)
                .and_then(|entry| check_id(&entry.id).map(|()| entry));
            entry
                .inspect_err(|err| log::warn!("skipping an entry of the gallery index: {err:#}"))
                .ok()
        });
        Ok(entries.collect())
    }

    /// The contents of a file of a wallpaper.
    fn open(&self, id: &str, path: &str) -> anyhow::Result<ureq::BodyReader<'static>> {
        // Ids and paths have only characters that need no percent-encoding.
        let url = format!("{}/packs/{id}/{path}", self.base);
        Ok(self.get(&url)?.into_reader())
    }

    /// The text of one file of a wallpaper, after the checks of its size and
    /// SHA-256.
    fn read(&self, entry: &Entry, path: &str) -> anyhow::Result<String> {
        let file = (entry.files.iter())
            .find(|f| f.path == path)
            .with_context(|| format!("`{}` has no file `{path}`", entry.id))?;
        let data = check_copy(file, self.open(&entry.id, path)?, Vec::new())
            .with_context(|| format!("failed to download `{path}` of `{}`", entry.id))?;
        String::from_utf8(data).with_context(|| format!("`{path}` of `{}` is not text", entry.id))
    }
}

/// The wallpapers in `index.json`, if phonto can read its format. They stay
/// JSON, so one entry that phonto cannot read does not hide the others.
fn parse_index(json: &[u8], gallery: &str) -> anyhow::Result<Vec<Value>> {
    let index: Value = serde_json::from_slice(json)
        .with_context(|| format!("the index of the gallery at {gallery} is not valid JSON"))?;
    match index.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(format) => bail!(
            "this gallery needs a newer phonto: the gallery at {gallery} has index format \
             {format}, and this phonto reads format {FORMAT}"
        ),
        None => bail!("the index of the gallery at {gallery} has no `format`"),
    }
    match index.get("wallpapers") {
        Some(Value::Array(wallpapers)) => Ok(wallpapers.clone()),
        _ => bail!("the index of the gallery at {gallery} has no list of `wallpapers`"),
    }
}

fn find_entry(wallpapers: &[Value], id: &str) -> anyhow::Result<Option<Entry>> {
    let Some(value) = wallpapers
        .iter()
        .find(|w| w.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Ok(None);
    };
    let entry = Entry::deserialize(value)
        .map_err(anyhow::Error::from)
        .and_then(|entry| check_entry(&entry).map(|()| entry))
        .with_context(|| format!("phonto cannot use the entry of `{id}` in the gallery index"))?;
    Ok(Some(entry))
}

/// Checks an entry before phonto downloads anything for it.
fn check_entry(entry: &Entry) -> anyhow::Result<()> {
    if entry.version.is_empty() {
        bail!("it has no version");
    }
    if entry.files.is_empty() {
        bail!("it has no files");
    }
    if entry.files.len() > MAX_FILES {
        bail!(
            "it has {} files, and phonto downloads at most {MAX_FILES}",
            entry.files.len()
        );
    }
    // Lowercase, because macOS file systems ignore case by default, and two
    // names that differ only in case would be one file there.
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    for file in &entry.files {
        check_path(&file.path)?;
        if !seen.insert(file.path.to_ascii_lowercase()) {
            bail!("it has `{}` twice", file.path);
        }
        if !(file.sha256.len() == 64 && file.sha256.bytes().all(|b| b.is_ascii_hexdigit())) {
            bail!("the SHA-256 of `{}` is not 64 hex digits", file.path);
        }
        total = total.saturating_add(file.size);
    }
    if total > MAX_BYTES {
        bail!(
            "it has {} MB of files, and phonto downloads at most {} MB",
            total.div_ceil(1024 * 1024),
            MAX_BYTES / (1024 * 1024)
        );
    }
    let listed = |path: &str| entry.files.iter().any(|f| f.path == path);
    if entry.run.is_empty() {
        if !listed(crate::procedural::MANIFEST) {
            bail!(
                "its `run` is empty, so it is a pack, but it has no `{}`",
                crate::procedural::MANIFEST
            );
        }
    } else {
        check_path(&entry.run).context("`run` is not a valid file name")?;
        if !listed(&entry.run) {
            bail!("its `run` file `{}` is not among its files", entry.run);
        }
    }
    if let Some(inputs) = &entry.inputs {
        check_path(inputs).context("`inputs` is not a valid file name")?;
        if !listed(inputs) {
            bail!("its `inputs` file `{inputs}` is not among its files");
        }
    }
    Ok(())
}

/// Checks a path from the gallery before phonto writes a file there: plain
/// names, with `/` between them, none of them hidden. So a path cannot leave
/// the folder of the wallpaper, or replace `.gallery.toml`.
fn check_path(path: &str) -> anyhow::Result<()> {
    if path == RECORD {
        bail!("`{path}` is the file where phonto keeps what it downloaded");
    }
    for name in path.split('/') {
        if name.is_empty() {
            bail!("`{path}` is not a relative path with a name between each `/`");
        }
        if name.starts_with('.') {
            bail!("`{path}` has a name that starts with a dot");
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            bail!("`{path}` has characters other than A-Z, a-z, 0-9, `.`, `_`, and `-`");
        }
    }
    Ok(())
}

/// Downloads the files of `entry` into `dir`, through a partial folder next to
/// it. `dir` changes only when every file is complete and matches its
/// checksum. On an error, the partial folder goes away.
fn fetch_into<R: Read>(
    entry: &Entry,
    gallery: &str,
    dir: &Path,
    open: impl FnMut(&FileEntry) -> anyhow::Result<R>,
) -> anyhow::Result<()> {
    let parent = dir
        .parent()
        .context("the folder of a wallpaper has no parent")?;
    sweep(parent);
    let part = sibling(dir, "part");
    let _ = fs::remove_dir_all(&part);
    fs::create_dir(&part).with_context(|| format!("failed to create {}", part.display()))?;
    let result = fill(entry, gallery, &part, open).and_then(|()| replace(&part, dir));
    if result.is_err() {
        let _ = fs::remove_dir_all(&part);
    }
    result
}

fn fill<R: Read>(
    entry: &Entry,
    gallery: &str,
    part: &Path,
    mut open: impl FnMut(&FileEntry) -> anyhow::Result<R>,
) -> anyhow::Result<()> {
    for file in &entry.files {
        let reader = open(file)?;
        download(file, part, reader)
            .with_context(|| format!("failed to download `{}` of `{}`", file.path, entry.id))?;
    }
    let record = toml::to_string(&Record::new(entry, gallery))?;
    fs::write(part.join(RECORD), record)
        .with_context(|| format!("failed to write {}", part.join(RECORD).display()))
}

/// Writes one file into `dir`, and checks its size and SHA-256.
fn download(file: &FileEntry, dir: &Path, reader: impl Read) -> anyhow::Result<()> {
    let target = dir.join(&file.path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    check_copy(file, reader, fs::File::create_new(&target)?)?;
    Ok(())
}

/// Copies one file of the gallery into `out`, and checks its size and
/// SHA-256.
fn check_copy<W: Write>(file: &FileEntry, reader: impl Read, out: W) -> anyhow::Result<W> {
    let mut out = Hashing::new(out);
    // One byte more than the index says, to notice a larger file.
    let size = io::copy(&mut reader.take(file.size + 1), &mut out)?;
    if size != file.size {
        bail!(
            "the gallery sent {} bytes, and its index says {}",
            if size > file.size {
                "more".to_string()
            } else {
                size.to_string()
            },
            file.size
        );
    }
    let (out, hash) = out.finish();
    if !hash.eq_ignore_ascii_case(&file.sha256) {
        bail!("the file does not match its SHA-256 in the gallery index");
    }
    Ok(out)
}

/// True if `dir` holds this version of the wallpaper from this gallery, with
/// every file whole.
fn is_complete(dir: &Path, gallery: &str, entry: &Entry) -> bool {
    let Some(record) = read_record(dir) else {
        return false;
    };
    let same = record.id == entry.id
        && record.version == entry.version
        && record.gallery == gallery
        && record.run == entry.run
        && record.inputs == entry.inputs;
    same && entry.files.iter().all(|file| {
        let path = dir.join(&file.path);
        fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == file.size)
            && hash_file(&path).is_ok_and(|hash| hash.eq_ignore_ascii_case(&file.sha256))
    })
}

fn read_record(dir: &Path) -> Option<Record> {
    let text = fs::read_to_string(dir.join(RECORD)).ok()?;
    toml::from_str(&text).ok()
}

/// Puts the folder `part` in the place of `dir`. One rename cannot replace a
/// folder that has files, so the old folder moves aside first, and comes back
/// if the new one cannot take its place.
fn replace(part: &Path, dir: &Path) -> anyhow::Result<()> {
    let moved = || format!("failed to move {} to {}", part.display(), dir.display());
    if fs::symlink_metadata(dir).is_err() {
        return fs::rename(part, dir).with_context(moved);
    }
    let old = sibling(dir, "old");
    let _ = fs::remove_dir_all(&old);
    fs::rename(dir, &old).with_context(|| format!("failed to move {} aside", dir.display()))?;
    if let Err(err) = fs::rename(part, dir) {
        let _ = fs::rename(&old, dir);
        return Err(err).with_context(moved);
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// A hidden folder next to `dir` for this process: `.koi.part-1234`.
fn sibling(dir: &Path, kind: &str) -> PathBuf {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    dir.with_file_name(format!(".{name}.{kind}-{}", std::process::id()))
}

/// Removes the folders that downloads left when they stopped early, as on
/// Ctrl-C, once their process is gone.
fn sweep(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(leftover_pid) else {
            continue;
        };
        if pid != std::process::id() && !running(pid) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// The process of a folder that `sibling` named.
fn leftover_pid(name: &str) -> Option<u32> {
    let name = name.strip_prefix('.')?;
    let (_, pid) = name
        .rsplit_once(".part-")
        .or_else(|| name.rsplit_once(".old-"))?;
    pid.parse().ok()
}

fn running(pid: u32) -> bool {
    // kill(0) and negative ids name process groups, so keep such a folder.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal 0 sends nothing. It only checks that the process exists.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Writes to `inner`, and hashes what it writes.
struct Hashing<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Hashing<W> {
    fn new(inner: W) -> Self {
        Hashing {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The writer, and the SHA-256 of what it wrote, in hex.
    fn finish(self) -> (W, String) {
        (self.inner, hex(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut out = Hashing::new(io::sink());
    io::copy(&mut fs::File::open(path)?, &mut out)?;
    Ok(out.finish().1)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// "`koi` (A koi pond, by museslabs)".
pub fn describe(id: &str, title: Option<&str>, author: Option<&str>) -> String {
    let text = describe_title(title, author);
    if text.is_empty() {
        format!("`{id}`")
    } else {
        format!("`{id}` ({text})")
    }
}

/// "A koi pond, by museslabs", or "" without a title.
fn describe_title(title: Option<&str>, author: Option<&str>) -> String {
    let (title, author) = (clean(title.unwrap_or("")), clean(author.unwrap_or("")));
    match (title.as_str(), author.as_str()) {
        ("", _) => String::new(),
        (title, "") => title.to_string(),
        (title, author) => format!("{title}, by {author}"),
    }
}

/// Text from the gallery, for the terminal. Control characters, which could
/// drive the terminal, go away.
pub fn clean(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    text.trim().to_string()
}

fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// "57 KB".
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    match bytes {
        0..KB => format!("{bytes} bytes"),
        KB..1_048_576 => format!("{} KB", bytes.div_ceil(KB)),
        _ => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;

    use super::*;

    const LOCAL: &str = "http://localhost:8000";

    fn reference(s: &str) -> Option<String> {
        parse_reference(s, Some(LOCAL)).unwrap()
    }

    fn sha(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    #[test]
    fn references_name_ids_with_the_prefix_or_a_page_url() {
        let koi = Some("koi".to_string());
        for s in [
            "pinakothiki:koi",
            " PINAKOTHIKI:koi ",
            "https://pinakothiki.museslabs.org/w/koi",
            "https://pinakothiki.museslabs.org/w/koi/",
            "http://Pinakothiki.MusesLabs.org/w/koi/",
            "https://pinakothiki.museslabs.org:8443/w/koi",
            "https://pinakothiki.museslabs.org/w/koi/?from=home#top",
            "http://localhost:8000/w/koi",
            "HTTP://LOCALHOST:8000/w/koi/",
        ] {
            assert_eq!(reference(s), koi, "{s}");
        }
        assert_eq!(
            parse_reference("http://[::1]:8000/w/koi", Some("http://[::1]:8000")).unwrap(),
            koi
        );
        // Only the origin of the gallery counts, not its path.
        assert_eq!(
            parse_reference(
                "https://gallery.example/w/koi/",
                Some("https://gallery.example/pk")
            )
            .unwrap(),
            koi
        );
    }

    #[test]
    fn other_strings_keep_their_meaning() {
        for s in [
            "koi",
            "/home/me/koi",
            "~/wallpapers/koi.glsl",
            "examples/procedural/koi",
            "https://example.com/w/koi",
            "http://localhost:8001/w/koi",
            "https://localhost:8000/w/koi",
            "https://pinakothiki.museslabs.org.evil.example/w/koi",
            "https://pinakothiki.museslabs.org@evil.example/w/koi",
            "https://evil.example/?u=https://pinakothiki.museslabs.org/w/koi",
            "https://pinakothiki.museslabs.org/",
            "https://pinakothiki.museslabs.org/w/",
            "https://pinakothiki.museslabs.org/w/koi/more",
            "https://pinakothiki.museslabs.org/koi",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "rtsp://pinakothiki.museslabs.org/w/koi",
        ] {
            assert_eq!(reference(s), None, "{s}");
        }
        // Without a known gallery, only Pinakothiki pages count.
        assert_eq!(
            parse_reference("http://localhost:8000/w/koi", None).unwrap(),
            None
        );
    }

    #[test]
    fn references_with_a_bad_id_fail() {
        for s in [
            "pinakothiki:",
            "pinakothiki:Koi",
            "pinakothiki://koi",
            "pinakothiki:-koi",
            "pinakothiki:koi/x",
            "pinakothiki:../koi",
            "https://pinakothiki.museslabs.org/w/Koi",
            "https://pinakothiki.museslabs.org/w/k%6Fi",
            "http://localhost:8000/w/koi_pond/",
        ] {
            let err = parse_reference(s, Some(LOCAL)).unwrap_err();
            assert!(
                format!("{err:#}").contains("is not a wallpaper id"),
                "{s}: {err:#}"
            );
        }
    }

    #[test]
    fn ids_are_short_lowercase_names() {
        for id in ["koi", "0", "koi-pond", "a1-b2-", &"a".repeat(64)] {
            assert!(check_id(id).is_ok(), "{id}");
        }
        for id in [
            "",
            "-koi",
            "Koi",
            "koi pond",
            "koi_pond",
            "κόι",
            &"a".repeat(65),
        ] {
            assert!(check_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn install_takes_a_bare_id_too() {
        assert_eq!(id_argument("koi", Some(LOCAL)).unwrap(), "koi");
        assert_eq!(id_argument(" koi\n", None).unwrap(), "koi");
        assert_eq!(id_argument("pinakothiki:koi", None).unwrap(), "koi");
        assert_eq!(
            id_argument("http://localhost:8000/w/koi/", Some(LOCAL)).unwrap(),
            "koi"
        );
        let err = id_argument("https://example.com/w/koi", Some(LOCAL)).unwrap_err();
        assert!(format!("{err:#}").contains("is not the page of a wallpaper"));
        assert!(id_argument("Koi", Some(LOCAL)).is_err());
        assert!(id_argument("./koi", Some(LOCAL)).is_err());
    }

    #[test]
    fn the_gallery_comes_from_the_environment_then_the_config() {
        let pick = |env, config| pick_base(env, config).unwrap();
        assert_eq!(pick(None, None), DEFAULT_GALLERY);
        assert_eq!(pick(Some(""), Some(" ")), DEFAULT_GALLERY);
        assert_eq!(pick(None, Some("http://localhost:8000/")), LOCAL);
        assert_eq!(
            pick(Some("HTTPS://mirror.example/pinakothiki//"), Some(LOCAL)),
            "https://mirror.example/pinakothiki"
        );
        for bad in [
            "ftp://mirror.example",
            "mirror.example",
            "https://",
            "https://a b",
            "https://mirror.example/?x",
            "https://me@mirror.example",
            "https://mirror.example:port",
        ] {
            let err = pick_base(Some(bad), None).unwrap_err();
            assert!(
                format!("{err:#}").contains("PHONTO_GALLERY must be"),
                "{bad}"
            );
            let err = pick_base(None, Some(bad)).unwrap_err();
            assert!(format!("{err:#}").contains("`gallery` in the configuration"));
        }
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        for path in [
            "manifest.toml",
            "image.glsl",
            "shaders/common.glsl",
            "a/b/c_d-e.1.png",
            "LICENSE",
        ] {
            assert!(check_path(path).is_ok(), "{path}");
        }
        for path in [
            "",
            "/etc/passwd",
            "a//b",
            "a/",
            ".",
            "..",
            "./a",
            "../a",
            "a/../b",
            "a/./b",
            ".hidden",
            "a/.git/config",
            ".gallery.toml",
            "a b.glsl",
            "a\\b",
            "c:a",
            "κόι.glsl",
            "a\0b",
        ] {
            assert!(check_path(path).is_err(), "{path:?}");
        }
    }

    fn index(wallpapers: &str) -> String {
        format!(r#"{{ "format": 1, "generated": "today", "wallpapers": [{wallpapers}] }}"#)
    }

    fn koi_json(run: &str, inputs: &str, files: &str) -> String {
        format!(
            r#"{{
                "id": "koi",
                "title": "A koi pond",
                "author": "museslabs",
                "summary": "One line.",
                "tags": ["water", "time"],
                "license": "GPL-3.0-or-later",
                "run": "{run}",
                "inputs": {inputs},
                "version": "3f2a9c1b7d4e",
                "size": 123456,
                "files": [{files}],
                "page": "/w/koi/",
                "stars": 5
            }}"#
        )
    }

    fn file_json(path: &str, size: u64) -> String {
        format!(
            r#"{{ "path": "{path}", "size": {size}, "sha256": "{}", "mime": "text/plain" }}"#,
            "ab".repeat(32)
        )
    }

    fn entry_of(json: &str, id: &str) -> anyhow::Result<Option<Entry>> {
        find_entry(&parse_index(json.as_bytes(), LOCAL)?, id)
    }

    #[test]
    fn the_index_ignores_unknown_keys() {
        let pack = koi_json(
            "",
            r#""inputs.toml""#,
            &[
                file_json("manifest.toml", 10),
                file_json("image.glsl", 20),
                file_json("inputs.toml", 30),
            ]
            .join(","),
        );
        let other = r#"{ "id": "broken", "files": "not a list" }"#;
        let json = index(&format!("{other}, {pack}"));
        let entry = entry_of(&json, "koi").unwrap().unwrap();
        assert_eq!(entry.id, "koi");
        assert_eq!(entry.title.as_deref(), Some("A koi pond"));
        assert_eq!(entry.run, "");
        assert_eq!(entry.inputs.as_deref(), Some("inputs.toml"));
        assert_eq!(entry.version, "3f2a9c1b7d4e");
        assert_eq!(entry.files.len(), 3);
        assert_eq!(entry.files[1].path, "image.glsl");
        assert_eq!(entry.files[1].size, 20);
        assert!(entry_of(&json, "carp").unwrap().is_none());
        // An entry that phonto cannot read fails alone.
        let err = entry_of(&json, "broken").unwrap_err();
        assert!(format!("{err:#}").contains("entry of `broken`"), "{err:#}");

        let single = koi_json("plasma.glsl", "null", &file_json("plasma.glsl", 5));
        let entry = entry_of(&index(&single), "koi").unwrap().unwrap();
        assert_eq!(entry.run, "plasma.glsl");
        assert_eq!(entry.inputs, None);
    }

    #[test]
    fn another_index_format_needs_a_newer_phonto() {
        let err = parse_index(br#"{ "format": 2, "wallpapers": {} }"#, LOCAL).unwrap_err();
        assert!(
            format!("{err:#}").contains("this gallery needs a newer phonto"),
            "{err:#}"
        );
        assert!(parse_index(br#"{ "wallpapers": [] }"#, LOCAL).is_err());
        assert!(parse_index(br#"{ "format": 1 }"#, LOCAL).is_err());
        assert!(parse_index(b"<html>", LOCAL).is_err());
        assert!(parse_index(br#"{ "format": 1, "wallpapers": [] }"#, LOCAL).is_ok());
    }

    #[test]
    fn entries_are_checked_before_any_download() {
        let check = |run: &str, inputs: &str, files: &[String]| {
            let json = index(&koi_json(run, inputs, &files.join(",")));
            entry_of(&json, "koi")
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        };
        let manifest = file_json("manifest.toml", 1);
        assert!(check("", "null", std::slice::from_ref(&manifest)).is_ok());

        let err = check("", "null", &[file_json("image.glsl", 1)]).unwrap_err();
        assert!(err.contains("no `manifest.toml`"), "{err}");
        let err = check("plasma.glsl", "null", std::slice::from_ref(&manifest)).unwrap_err();
        assert!(err.contains("not among its files"), "{err}");
        let err = check("../plasma.glsl", "null", std::slice::from_ref(&manifest)).unwrap_err();
        assert!(err.contains("`run` is not a valid file name"), "{err}");
        let err = check("", r#""inputs.toml""#, std::slice::from_ref(&manifest)).unwrap_err();
        assert!(err.contains("not among its files"), "{err}");
        let err = check("", "null", &[manifest.clone(), file_json("../x", 1)]).unwrap_err();
        assert!(err.contains("`../x`"), "{err}");
        let err = check(
            "",
            "null",
            &[manifest.clone(), file_json(".gallery.toml", 1)],
        );
        assert!(err.is_err());
        let err = check(
            "",
            "null",
            &[manifest.clone(), file_json("MANIFEST.toml", 1)],
        );
        assert!(err.unwrap_err().contains("twice"));

        let many: Vec<String> = (0..=MAX_FILES)
            .map(|i| file_json(&format!("f{i}"), 1))
            .chain([manifest.clone()])
            .collect();
        let err = check("", "null", &many).unwrap_err();
        assert!(err.contains("at most 512"), "{err}");
        let large = [manifest.clone(), file_json("video.mp4", MAX_BYTES)];
        let err = check("", "null", &large).unwrap_err();
        assert!(err.contains("at most 100 MB"), "{err}");

        let bad_sha = manifest.replace(&"ab".repeat(32), "abc");
        let err = check("", "null", &[bad_sha]).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
    }

    #[test]
    fn wallpapers_install_in_the_xdg_data_folder() {
        let root = |xdg: Option<&str>, home: Option<&str>| {
            installed_root_from(xdg.map(OsString::from), home.map(OsString::from))
        };
        assert_eq!(
            root(Some("/data"), Some("/home/me")),
            Some(PathBuf::from("/data/phonto/wallpapers"))
        );
        let home = Some(PathBuf::from("/home/me/.local/share/phonto/wallpapers"));
        assert_eq!(root(None, Some("/home/me")), home);
        assert_eq!(root(Some(""), Some("/home/me")), home);
        assert_eq!(root(Some("relative"), Some("/home/me")), home);
        assert_eq!(root(None, None), None);
        assert_eq!(root(None, Some("")), None);
    }

    #[test]
    fn folder_names_keep_plain_versions() {
        assert_eq!(version_slug("3f2a9c1b7d4e"), "3f2a9c1b7d4e");
        assert_eq!(version_slug("1.2.0"), "1.2.0");
        for odd in ["", "a-b", "../../x", "v/1", "a b", &"a".repeat(65)] {
            let slug = version_slug(odd);
            assert_eq!(slug.len(), 16, "{odd}");
            assert!(slug.bytes().all(|b| b.is_ascii_hexdigit()), "{odd}");
        }
        assert_ne!(version_slug("a-b"), version_slug("a-c"));
    }

    #[test]
    fn leftovers_name_their_process() {
        assert_eq!(leftover_pid(".koi.part-1234"), Some(1234));
        assert_eq!(leftover_pid(".koi-3f2a.old-7"), Some(7));
        assert_eq!(leftover_pid("koi.part-1234"), None);
        assert_eq!(leftover_pid(".koi"), None);
        assert_eq!(leftover_pid(".koi.part-x"), None);
        assert!(running(std::process::id()));
        assert!(running(0));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("phonto-gallery-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A pack with two files, and a fake gallery that serves `served`.
    fn pack(version: &str, served: &[(&str, &[u8])]) -> (Entry, HashMap<String, Vec<u8>>) {
        let files = [
            (
                "manifest.toml",
                b"[image]\nshader = \"shaders/image.glsl\"\n".as_slice(),
            ),
            (
                "shaders/image.glsl",
                b"void mainImage(out vec4 c, in vec2 p) {}".as_slice(),
            ),
        ];
        let entry = Entry {
            id: "koi".to_string(),
            title: Some("A koi pond".to_string()),
            author: None,
            run: String::new(),
            inputs: None,
            version: version.to_string(),
            files: files
                .iter()
                .map(|(path, data)| FileEntry {
                    path: path.to_string(),
                    size: data.len() as u64,
                    sha256: sha(data),
                })
                .collect(),
            ..Default::default()
        };
        let mut gallery: HashMap<_, _> = files
            .iter()
            .map(|(path, data)| (path.to_string(), data.to_vec()))
            .collect();
        for (path, data) in served {
            gallery.insert(path.to_string(), data.to_vec());
        }
        (entry, gallery)
    }

    fn fetch(entry: &Entry, served: &HashMap<String, Vec<u8>>, dir: &Path) -> anyhow::Result<()> {
        fetch_into(entry, LOCAL, dir, |file| {
            Ok(io::Cursor::new(served[&file.path].clone()))
        })
    }

    fn leftovers(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect()
    }

    #[test]
    fn a_download_replaces_the_folder_when_it_is_complete() {
        let root = temp_dir("replace");
        let dir = root.join("koi");

        let (v1, served) = pack("v1", &[]);
        fetch(&v1, &served, &dir).unwrap();
        assert!(is_complete(&dir, LOCAL, &v1));
        assert!(dir.join("shaders/image.glsl").is_file());
        let record = read_record(&dir).unwrap();
        assert_eq!((record.id.as_str(), record.version.as_str()), ("koi", "v1"));
        assert_eq!(record.gallery, LOCAL);
        assert_eq!(record.title, "A koi pond");
        assert_eq!(run_path(&dir, &record.run), dir.to_string_lossy());
        assert!(!is_complete(&dir, "https://other.example", &v1));

        let (v2, served) = pack("v2", &[]);
        assert!(!is_complete(&dir, LOCAL, &v2));
        fs::write(dir.join("mine.txt"), "gone after an update").unwrap();
        fetch(&v2, &served, &dir).unwrap();
        assert!(is_complete(&dir, LOCAL, &v2));
        assert!(!dir.join("mine.txt").exists());
        assert_eq!(leftovers(&root), Vec::<String>::new());

        // A file that changed on disk makes the folder incomplete.
        fs::write(dir.join("manifest.toml"), "[image]\nshader = \"x.glsl\"\n").unwrap();
        assert!(!is_complete(&dir, LOCAL, &v2));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_download_leaves_the_old_folder() {
        let root = temp_dir("bad");
        let dir = root.join("koi");
        let (v1, served) = pack("v1", &[]);
        fetch(&v1, &served, &dir).unwrap();

        let glsl = "shaders/image.glsl";
        let (v2, _) = pack("v2", &[]);
        let size = v2.files[1].size as usize;
        let cases: [(&[u8], &str); 3] = [
            (
                &b"void mainImage(out vec4 c, in vec2 p) {}"[..size - 1],
                "bytes",
            ),
            (
                b"void mainImage(out vec4 c, in vec2 p) {}\n",
                "sent more bytes",
            ),
            (b"void mainImage(out vec4 c, in vec2 p) {;", "SHA-256"),
        ];
        for (data, message) in cases {
            let (v2, served) = pack("v2", &[(glsl, data)]);
            let err = format!("{:#}", fetch(&v2, &served, &dir).unwrap_err());
            assert!(err.contains(message), "{err}");
            assert!(err.contains(glsl), "{err}");
            assert!(is_complete(&dir, LOCAL, &v1), "{message}");
            assert_eq!(leftovers(&root), Vec::<String>::new());
        }

        // A first download that fails leaves no folder at all.
        let fresh = root.join("carp");
        let (v2, served) = pack("v2", &[(glsl, b"nope")]);
        assert!(fetch(&v2, &served, &fresh).is_err());
        assert!(!fresh.exists());
        assert_eq!(leftovers(&root), Vec::<String>::new());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn leftovers_of_stopped_downloads_go_away() {
        let root = temp_dir("sweep");
        // No process has the largest id, and this process is running.
        let dead = root.join(format!(".koi.part-{}", libc::pid_t::MAX));
        let live = sibling(&root.join("carp"), "part");
        fs::create_dir_all(dead.join("shaders")).unwrap();
        fs::create_dir_all(&live).unwrap();
        let (entry, served) = pack("v1", &[]);
        fetch(&entry, &served, &root.join("koi")).unwrap();
        assert!(!dead.exists());
        assert!(live.exists());
        let _ = fs::remove_dir_all(&root);
    }

    fn record(id: &str, run: &str) -> Record {
        Record {
            id: id.to_string(),
            version: "v1".to_string(),
            gallery: LOCAL.to_string(),
            title: String::new(),
            author: String::new(),
            run: run.to_string(),
            inputs: None,
        }
    }

    fn folder(root: &Path, name: &str, record: &Record) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(RECORD), toml::to_string(record).unwrap()).unwrap();
        dir
    }

    #[test]
    fn a_path_names_the_wallpaper_that_phonto_runs_from_it() {
        let root = temp_dir("identify");
        let text = |p: PathBuf| p.to_string_lossy().into_owned();
        let pack = folder(&root, "koi", &record("koi", ""));
        let single = folder(&root, "plasma", &record("plasma", "plasma.glsl"));
        let nested = folder(&root, "deep", &record("deep", "glsl/deep.glsl"));
        let odd = folder(&root, "odd", &record("../odd", ""));
        let koi = Some("koi".to_string());
        assert_eq!(identify(&text(pack.clone())), koi);
        assert_eq!(identify(&format!("{}/", pack.display())), koi);
        assert_eq!(identify(&text(pack.join("manifest.toml"))), koi);
        assert_eq!(identify(&text(pack.join("image.glsl"))), None);
        assert_eq!(
            identify(&text(single.join("plasma.glsl"))),
            Some("plasma".into())
        );
        assert_eq!(identify(&text(single.clone())), None);
        assert_eq!(
            identify(&text(nested.join("glsl/deep.glsl"))),
            Some("deep".into())
        );
        assert_eq!(identify(&text(odd)), None);
        assert_eq!(identify(&text(root.join("none"))), None);
        assert_eq!(identify("https://pinakothiki.museslabs.org/w/koi"), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn urls_of_the_index_become_absolute() {
        let at = |gallery: &str, url: &str| absolute(gallery, url);
        let site = "https://g.example";
        assert_eq!(
            at(site, "/w/koi/").as_deref(),
            Some("https://g.example/w/koi/")
        );
        assert_eq!(
            at("https://g.example/pk", "/w/koi/").as_deref(),
            Some("https://g.example/w/koi/")
        );
        assert_eq!(
            at("https://g.example/pk", "img/koi.png").as_deref(),
            Some("https://g.example/pk/img/koi.png")
        );
        assert_eq!(
            at(site, "https://cdn.example/koi.png").as_deref(),
            Some("https://cdn.example/koi.png")
        );
        assert_eq!(
            at("http://localhost:8000", "//cdn.example/a.png").as_deref(),
            Some("http://cdn.example/a.png")
        );
        assert_eq!(at(site, ""), None);
        assert_eq!(at(site, "javascript:alert(1)"), None);
        assert_eq!(at(site, "data:image/png;base64,AA"), None);
    }

    #[test]
    fn a_search_needs_every_word() {
        let entry = Entry {
            id: "koi".to_string(),
            title: Some("A Koi pond".to_string()),
            author: Some("museslabs".to_string()),
            summary: Some("Fish in the light of the sun.".to_string()),
            tags: vec!["water".to_string(), "time".to_string()],
            ..Default::default()
        };
        let has = |words: &[&str]| {
            let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
            has_words(&entry, &words)
        };
        assert!(has(&[]));
        assert!(has(&["koi", "WATER", "muses", "sun"]));
        assert!(has(&["pond"]));
        assert!(!has(&["koi", "fire"]));
        // A word cannot start in one field and end in the next.
        assert!(!has(&["museslabsfish"]));
    }

    fn keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    #[test]
    fn json_has_the_documented_keys() {
        let (mut entry, _) = pack("v2", &[]);
        entry.preview = Some("/w/koi/preview.webp".to_string());
        entry.page = Some("/w/koi/".to_string());
        let mut installed = record("koi", "");
        let listed = Listed::new(LOCAL, &entry, Some(&installed));
        let json = serde_json::to_value(&listed).unwrap();
        let listed_keys = [
            "author",
            "id",
            "installed",
            "installed_version",
            "license",
            "page",
            "preview",
            "size",
            "summary",
            "tags",
            "title",
            "update",
            "version",
        ];
        assert_eq!(keys(&json), listed_keys);
        assert_eq!(json["preview"], "http://localhost:8000/w/koi/preview.webp");
        assert_eq!(json["page"], "http://localhost:8000/w/koi/");
        assert_eq!(json["installed_version"], "v1");
        assert_eq!(json["update"], true);
        assert_eq!(json["license"], serde_json::Value::Null);
        assert_eq!(
            json["size"],
            entry.files.iter().map(|f| f.size).sum::<u64>()
        );
        installed.version = "v2".to_string();
        let same = serde_json::to_value(Listed::new(LOCAL, &entry, Some(&installed))).unwrap();
        assert_eq!(same["update"], false);
        let none = serde_json::to_value(Listed::new(LOCAL, &entry, None)).unwrap();
        assert_eq!(
            (&none["installed"], &none["update"]),
            (&false.into(), &false.into())
        );
        assert!(none["installed_version"].is_null() && none["preview"].is_string());

        let listing = serde_json::to_value(ListingJson {
            gallery: LOCAL,
            wallpapers: vec![listed],
        })
        .unwrap();
        assert_eq!(keys(&listing), ["gallery", "wallpapers"]);

        let show = serde_json::to_value(ShowJson {
            listed: Listed::new(LOCAL, &entry, None),
            files: &entry.files,
            inputs: Vec::new(),
            actions: Vec::new(),
        })
        .unwrap();
        let mut show_keys: Vec<&str> = listed_keys.to_vec();
        show_keys.extend(["actions", "files", "inputs"]);
        show_keys.sort();
        assert_eq!(keys(&show), show_keys);
        assert_eq!(keys(&show["files"][0]), ["path", "sha256", "size"]);

        let install = serde_json::to_value(InstallJson {
            id: "koi".to_string(),
            status: Status::UpToDate,
            path: "/data/koi".to_string(),
            run: "pinakothiki:koi".to_string(),
        })
        .unwrap();
        assert_eq!(keys(&install), ["id", "path", "run", "status"]);
        assert_eq!(install["status"], "up_to_date");
        let uninstall = serde_json::to_value(UninstallJson {
            id: "koi".to_string(),
            status: "removed",
            path: "/data/koi".to_string(),
            settings_removed: false,
        })
        .unwrap();
        assert_eq!(
            keys(&uninstall),
            ["id", "path", "settings_removed", "status"]
        );

        let list = serde_json::to_value(InstalledListJson {
            wallpapers: vec![InstalledJson {
                id: "koi".to_string(),
                title: None,
                author: None,
                version: "v1".to_string(),
                gallery: LOCAL.to_string(),
                path: "/data/koi".to_string(),
                run: "pinakothiki:koi".to_string(),
                settings: None,
                has_inputs: false,
            }],
        })
        .unwrap();
        assert_eq!(keys(&list), ["wallpapers"]);
        assert_eq!(
            keys(&list["wallpapers"][0]),
            [
                "author",
                "gallery",
                "has_inputs",
                "id",
                "path",
                "run",
                "settings",
                "title",
                "version"
            ]
        );
    }
}
