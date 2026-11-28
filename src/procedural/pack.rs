//! Loads procedural wallpapers from disk. The README describes the format
//! of `manifest.toml`.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, bail};
use serde::Deserialize;

pub const MANIFEST: &str = "manifest.toml";
pub const MAX_BUFFERS: usize = 8;
pub const MAX_CHANNELS: usize = 8;
/// The most `[inputs]` that a pack can read.
pub const MAX_INPUTS: usize = 16;
/// The largest `size` or `height` of a buffer.
pub const MAX_BUFFER_SIZE: u32 = 8192;
/// The most actions that a pack can name. The actions buffer stores the
/// number of an action in one 8-bit channel, and 0 is no action.
pub const MAX_ACTIONS: usize = 255;
const DEFAULT_FPS: f32 = 60.0;

/// A procedural wallpaper, loaded and validated. A backend can compile it.
#[derive(Debug)]
pub struct Pack {
    pub name: String,
    /// If true, phonto reads the playback monitor of the system while this
    /// pack runs.
    pub audio: bool,
    /// The frame-rate cap. `None` draws at the refresh rate of the display.
    pub fps: Option<f32>,
    /// The fraction of the output resolution that every pass draws at.
    pub render_scale: f32,
    /// Code that comes before the code of every pass, like the "Common" tab
    /// on Shadertoy.
    pub common: Option<Source>,
    /// Offscreen passes, in the order that they draw.
    pub buffers: Vec<Buffer>,
    /// The pass that shows on screen.
    pub image: Pass,
    /// Images that channels use. Each image loads once.
    pub textures: Vec<Texture>,
    /// The values that the pack reads from the `[[input]]` commands of the
    /// configuration, as `iInput.<name>`.
    pub inputs: Vec<NamedInput>,
    /// Video files that channels use. Each video decodes once, and loops.
    pub videos: Vec<PathBuf>,
    /// The actions that a click can run, if the pack has an `[actions]`
    /// table.
    pub actions: Option<Actions>,
}

/// The actions of a pack. A click on the wallpaper runs the action under the
/// pointer. The command of each action comes from the `[[action]]` entries of
/// the configuration, never from the pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Actions {
    /// The buffer that holds the number of the action at each pixel, in its
    /// red channel. `names[0]` is number 1, and 0 is no action.
    pub buffer: usize,
    pub names: Vec<String>,
}

/// An input that a pack reads, with its value when no command gives one.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedInput {
    pub name: String,
    /// 1 for a `float`, and 2 to 4 for a `vec2` to a `vec4`.
    pub size: usize,
    pub default: [f32; 4],
}

#[derive(Debug)]
pub struct Source {
    pub path: PathBuf,
    pub code: String,
}

#[derive(Debug)]
pub struct Pass {
    pub name: String,
    pub source: Source,
    /// `channels[n]` is bound to `iChannel{n}`.
    pub channels: Vec<Channel>,
}

/// An offscreen pass and the shape of its render target.
#[derive(Debug)]
pub struct Buffer {
    pub pass: Pass,
    pub size: BufferSize,
    pub format: BufferFormat,
    /// If true, the buffer draws only into a new target (at start, or after
    /// the display changes size) and keeps the result. Use it for content
    /// that does not change.
    pub once: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSize {
    /// The size of the image pass, as on Shadertoy.
    Image,
    /// A fixed size, for state such as one texel for each creature.
    Fixed(u32, u32),
    /// A fixed height. The width follows the aspect ratio of the image, so a
    /// simulation behaves the same at all resolutions.
    Height(u32),
}

impl BufferSize {
    /// The size of this buffer when the image pass is `image` pixels.
    pub fn dims(self, image: (u32, u32)) -> (u32, u32) {
        match self {
            Self::Image => image,
            Self::Fixed(w, h) => (w, h),
            Self::Height(h) => {
                let aspect = f64::from(image.0) / f64::from(image.1.max(1));
                let w = (aspect * f64::from(h))
                    .round()
                    .clamp(1.0, f64::from(MAX_BUFFER_SIZE));
                (w as u32, h)
            }
        }
    }
}

/// The storage for each texel of a buffer. If the GPU cannot draw to the
/// requested format, the backend uses a smaller one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferFormat {
    Rgba8,
    Rgba16f,
    Rgba32f,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    pub input: Input,
    pub filter: Filter,
    pub wrap: Wrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The output of `Pack::buffers[i]`. If that buffer already drew in this
    /// frame, the input is the output of this frame. If a pass reads itself,
    /// or a buffer that draws later, the input is the output of the last
    /// frame. This is the same as on Shadertoy.
    Buffer(usize),
    /// `Pack::textures[i]`.
    Texture(usize),
    /// The current frame of `Pack::videos[i]`.
    Video(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    Nearest,
    Linear,
    /// Trilinear filtering, for textures only.
    Mipmap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

/// A decoded RGBA8 image. The rows go from bottom to top, so `uv = (0, 0)` is
/// the bottom-left corner. `fragCoord` has the same origin.
pub struct Texture {
    pub path: PathBuf,
    /// If true, phonto loads the image again when the file changes.
    pub watch: bool,
    /// The modification time of the file that this image came from. `None`
    /// for the empty image of a watched file that does not exist yet.
    pub modified: Option<SystemTime>,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Texture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Texture")
            .field("path", &self.path)
            .field("watch", &self.watch)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: Option<String>,
    #[serde(default)]
    audio: bool,
    fps: Option<f32>,
    render_scale: Option<f32>,
    common: Option<String>,
    #[serde(default, rename = "buffer")]
    buffers: Vec<BufferManifest>,
    image: ImageManifest,
    #[serde(default)]
    inputs: toml::Table,
    actions: Option<ActionsManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionsManifest {
    buffer: String,
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferManifest {
    name: String,
    shader: String,
    #[serde(default)]
    channels: Vec<ChannelManifest>,
    size: Option<[u32; 2]>,
    height: Option<u32>,
    format: Option<String>,
    #[serde(default)]
    once: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageManifest {
    shader: String,
    #[serde(default)]
    channels: Vec<ChannelManifest>,
}

/// A channel is a string (a buffer name, else an image path), or a table
/// that names the input and can change the sampling.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChannelManifest {
    Short(String),
    Full(ChannelTable),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelTable {
    buffer: Option<String>,
    texture: Option<String>,
    video: Option<String>,
    filter: Option<String>,
    wrap: Option<String>,
    #[serde(default)]
    watch: bool,
}

impl Pack {
    /// Loads a `.glsl` file, a pack directory, or the `manifest.toml` of a
    /// pack.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if path.is_dir() {
            return Self::from_manifest(&path.join(MANIFEST));
        }
        if path.file_name().is_some_and(|n| n == MANIFEST) {
            return Self::from_manifest(path);
        }
        Self::from_shader_file(path)
    }

    /// Loads the watched images again, for a reader that does not watch
    /// them, such as `dump`. An image that does not load keeps its last
    /// version.
    pub fn reload_watched(&mut self) {
        for texture in self.textures.iter_mut().filter(|t| t.watch) {
            match decode_texture(&texture.path) {
                Ok(new) => *texture = Texture { watch: true, ..new },
                Err(err) => log::info!("{err:#}"),
            }
        }
    }

    fn from_shader_file(path: &Path) -> anyhow::Result<Self> {
        let source = read_source(path)?;
        if !defines_main_image(&source.code) {
            bail!(
                "{} has no `mainImage` function. Procedural wallpapers define \
                 `void mainImage(out vec4 fragColor, in vec2 fragCoord)`. \
                 To post-process a video with this shader, pass it with --shader instead",
                path.display()
            );
        }
        Ok(Self {
            name: file_stem(path),
            audio: false,
            fps: Some(DEFAULT_FPS),
            render_scale: 1.0,
            common: None,
            buffers: Vec::new(),
            image: Pass {
                name: "image".to_string(),
                source,
                channels: Vec::new(),
            },
            textures: Vec::new(),
            inputs: Vec::new(),
            videos: Vec::new(),
            actions: None,
        })
    }

    fn from_manifest(manifest_path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest: Manifest =
            toml::from_str(&text).with_context(|| format!("parse {}", manifest_path.display()))?;
        let dir = manifest_path.parent().unwrap_or(Path::new("."));
        Self::from_parsed(manifest, dir)
            .with_context(|| format!("invalid pack {}", manifest_path.display()))
    }

    fn from_parsed(m: Manifest, dir: &Path) -> anyhow::Result<Self> {
        let fps = match m.fps {
            None => Some(DEFAULT_FPS),
            Some(0.0) => None,
            Some(f) if f.is_finite() && f > 0.0 => Some(f),
            Some(f) => bail!("fps must be a positive number, or 0 for uncapped (got {f})"),
        };

        let render_scale = m.render_scale.unwrap_or(1.0);
        if !(render_scale.is_finite() && render_scale > 0.0 && render_scale <= 1.0) {
            bail!("render_scale must be greater than 0 and at most 1 (got {render_scale})");
        }

        if m.buffers.len() > MAX_BUFFERS {
            bail!(
                "at most {MAX_BUFFERS} [[buffer]] passes are supported (got {})",
                m.buffers.len()
            );
        }
        let mut buffer_index: HashMap<&str, usize> = HashMap::new();
        for (i, b) in m.buffers.iter().enumerate() {
            if b.name.trim().is_empty() {
                bail!("[[buffer]] #{} has an empty name", i + 1);
            }
            if buffer_index.insert(b.name.as_str(), i).is_some() {
                bail!("two [[buffer]] passes are named `{}`", b.name);
            }
        }

        let common = m
            .common
            .as_deref()
            .map(|p| read_source(&dir.join(p)))
            .transpose()?;

        let mut sources = Sources::default();
        let mut buffers = Vec::with_capacity(m.buffers.len());
        for b in &m.buffers {
            let buffer = build_buffer(b, dir, &buffer_index, &mut sources)
                .with_context(|| format!("[[buffer]] `{}`", b.name))?;
            buffers.push(buffer);
        }
        let image = build_pass(
            "image",
            &m.image.shader,
            &m.image.channels,
            dir,
            &buffer_index,
            &mut sources,
        )
        .context("[image]")?;
        let actions = m
            .actions
            .map(|a| parse_actions(a, &buffer_index, &buffers))
            .transpose()
            .context("[actions]")?;

        Ok(Self {
            name: m.name.unwrap_or_else(|| file_stem(dir)),
            audio: m.audio,
            fps,
            render_scale,
            common,
            buffers,
            image,
            textures: sources.textures.list,
            inputs: parse_inputs(&m.inputs).context("[inputs]")?,
            videos: sources.videos,
            actions,
        })
    }
}

fn build_buffer(
    b: &BufferManifest,
    dir: &Path,
    buffer_index: &HashMap<&str, usize>,
    sources: &mut Sources,
) -> anyhow::Result<Buffer> {
    let in_range = |v: u32| (1..=MAX_BUFFER_SIZE).contains(&v);
    let size = match (b.size, b.height) {
        (None, None) => BufferSize::Image,
        (Some([w, h]), None) if in_range(w) && in_range(h) => BufferSize::Fixed(w, h),
        (None, Some(h)) if in_range(h) => BufferSize::Height(h),
        (Some(_), Some(_)) => bail!("set at most one of `size` or `height`"),
        _ => bail!("buffer sizes must be between 1 and {MAX_BUFFER_SIZE}"),
    };
    let format = match b.format.as_deref() {
        None | Some("rgba16f") => BufferFormat::Rgba16f,
        Some("rgba32f") => BufferFormat::Rgba32f,
        Some("rgba8") => BufferFormat::Rgba8,
        Some(f) => bail!("unknown format `{f}` (expected rgba8, rgba16f or rgba32f)"),
    };
    let pass = build_pass(&b.name, &b.shader, &b.channels, dir, buffer_index, sources)?;
    Ok(Buffer {
        pass,
        size,
        format,
        once: b.once,
    })
}

fn build_pass(
    name: &str,
    shader: &str,
    channels: &[ChannelManifest],
    dir: &Path,
    buffer_index: &HashMap<&str, usize>,
    sources: &mut Sources,
) -> anyhow::Result<Pass> {
    if channels.len() > MAX_CHANNELS {
        bail!(
            "at most {MAX_CHANNELS} channels are supported (got {})",
            channels.len()
        );
    }
    let source = read_source(&dir.join(shader))?;
    if !defines_main_image(&source.code) {
        bail!(
            "{} does not define `mainImage(out vec4, in vec2)`",
            source.path.display()
        );
    }
    let channels = channels
        .iter()
        .enumerate()
        .map(|(n, c)| {
            resolve_channel(c, dir, buffer_index, sources).with_context(|| format!("iChannel{n}"))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Pass {
        name: name.to_string(),
        source,
        channels,
    })
}

fn resolve_channel(
    c: &ChannelManifest,
    dir: &Path,
    buffer_index: &HashMap<&str, usize>,
    sources: &mut Sources,
) -> anyhow::Result<Channel> {
    let (buffer, texture, video, filter, wrap, watch) = match c {
        ChannelManifest::Short(s) if buffer_index.contains_key(s.as_str()) => {
            (Some(s.as_str()), None, None, None, None, false)
        }
        ChannelManifest::Short(s) => (None, Some(s.as_str()), None, None, None, false),
        ChannelManifest::Full(t) => (
            t.buffer.as_deref(),
            t.texture.as_deref(),
            t.video.as_deref(),
            t.filter.as_deref(),
            t.wrap.as_deref(),
            t.watch,
        ),
    };

    let (input, default_filter, default_wrap) = match (buffer, texture, video) {
        (Some(_), None, None) | (None, None, Some(_)) if watch => {
            bail!("`watch` is only for textures")
        }
        (Some(name), None, None) => {
            let Some(&i) = buffer_index.get(name) else {
                bail!("no [[buffer]] is named `{name}`");
            };
            (Input::Buffer(i), Filter::Linear, Wrap::Clamp)
        }
        (None, Some(path), None) => {
            let i = sources
                .textures
                .load(&dir.join(crate::config::expand_tilde(path)), watch)?;
            (Input::Texture(i), Filter::Mipmap, Wrap::Repeat)
        }
        (None, None, Some(path)) => {
            let i = sources.video(&dir.join(crate::config::expand_tilde(path)))?;
            (Input::Video(i), Filter::Linear, Wrap::Clamp)
        }
        _ => bail!("set exactly one of `buffer`, `texture`, or `video`"),
    };

    let filter = filter
        .map(parse_filter)
        .transpose()?
        .unwrap_or(default_filter);
    let wrap = wrap.map(parse_wrap).transpose()?.unwrap_or(default_wrap);
    if filter == Filter::Mipmap && !matches!(input, Input::Texture(_)) {
        bail!("filter = \"mipmap\" is only supported for textures, not buffers or videos");
    }
    Ok(Channel {
        input,
        filter,
        wrap,
    })
}

fn parse_actions(
    m: ActionsManifest,
    buffer_index: &HashMap<&str, usize>,
    buffers: &[Buffer],
) -> anyhow::Result<Actions> {
    let Some(&buffer) = buffer_index.get(m.buffer.as_str()) else {
        bail!("no [[buffer]] is named `{}`", m.buffer);
    };
    // The number of an action must come back exactly when phonto reads the
    // pixel under a click. A float buffer can change it, so the buffer
    // stores 8-bit values.
    if buffers[buffer].format != BufferFormat::Rgba8 {
        bail!(
            "the actions buffer `{}` must set format = \"rgba8\"",
            m.buffer
        );
    }
    if m.names.is_empty() || m.names.len() > MAX_ACTIONS {
        bail!(
            "`names` must have 1 to {MAX_ACTIONS} actions (got {})",
            m.names.len()
        );
    }
    for (i, name) in m.names.iter().enumerate() {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !valid {
            bail!("`{name}` is not a valid action name. Use letters, digits, _, and -");
        }
        if m.names[..i].contains(name) {
            bail!("two actions are named `{name}`");
        }
    }
    Ok(Actions {
        buffer,
        names: m.names,
    })
}

fn parse_inputs(table: &toml::Table) -> anyhow::Result<Vec<NamedInput>> {
    if table.len() > MAX_INPUTS {
        bail!(
            "at most {MAX_INPUTS} inputs are supported (got {})",
            table.len()
        );
    }
    table
        .iter()
        .map(|(name, value)| {
            if !is_glsl_name(name) {
                bail!(
                    "`{name}` is not a GLSL name. Use letters, digits, and _, and start with a letter"
                );
            }
            let number = |v: &toml::Value| match v {
                toml::Value::Integer(i) => Some(*i as f32),
                toml::Value::Float(f) => Some(*f as f32),
                _ => None,
            };
            let numbers: Vec<f32> = match value {
                toml::Value::Array(items) => items.iter().map(number).collect::<Option<_>>(),
                v => number(v).map(|n| vec![n]),
            }
            .with_context(|| format!("`{name}` must be a number or a list of numbers"))?;
            if !(1..=4).contains(&numbers.len()) {
                bail!("`{name}` has {} numbers. Use 1 to 4", numbers.len());
            }
            let mut default = [0.0; 4];
            default[..numbers.len()].copy_from_slice(&numbers);
            Ok(NamedInput {
                name: name.clone(),
                size: numbers.len(),
                default,
            })
        })
        .collect()
}

/// True for a name that GLSL accepts and does not reserve.
fn is_glsl_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("gl_")
        && !name.contains("__")
}

fn parse_filter(s: &str) -> anyhow::Result<Filter> {
    match s {
        "nearest" => Ok(Filter::Nearest),
        "linear" => Ok(Filter::Linear),
        "mipmap" => Ok(Filter::Mipmap),
        _ => bail!("unknown filter `{s}` (expected nearest, linear or mipmap)"),
    }
}

fn parse_wrap(s: &str) -> anyhow::Result<Wrap> {
    match s {
        "clamp" => Ok(Wrap::Clamp),
        "repeat" => Ok(Wrap::Repeat),
        "mirror" => Ok(Wrap::Mirror),
        _ => bail!("unknown wrap `{s}` (expected clamp, repeat or mirror)"),
    }
}

/// The images and the videos that the channels of a pack use.
#[derive(Default)]
struct Sources {
    textures: TextureSet,
    videos: Vec<PathBuf>,
}

impl Sources {
    /// The index of the video at `path`. Two channels that name the same
    /// file share one decoder.
    fn video(&mut self, path: &Path) -> anyhow::Result<usize> {
        if let Some(i) = self.videos.iter().position(|v| v == path) {
            return Ok(i);
        }
        if !path.is_file() {
            bail!("video {} does not exist", path.display());
        }
        self.videos.push(path.to_path_buf());
        Ok(self.videos.len() - 1)
    }
}

/// Decoded images, one for each path.
#[derive(Default)]
struct TextureSet {
    list: Vec<Texture>,
    by_path: HashMap<PathBuf, usize>,
}

impl TextureSet {
    /// Loads the image at `path`. A watched image that does not load yet is
    /// empty until its file changes.
    fn load(&mut self, path: &Path, watch: bool) -> anyhow::Result<usize> {
        if let Some(&i) = self.by_path.get(path) {
            self.list[i].watch |= watch;
            return Ok(i);
        }
        let texture = match decode_texture(path) {
            Ok(texture) => texture,
            Err(err) if watch => {
                log::info!("{err:#}. The channel stays empty until the file changes");
                Texture {
                    path: path.to_path_buf(),
                    watch,
                    modified: None,
                    width: 1,
                    height: 1,
                    rgba: vec![0; 4],
                }
            }
            Err(err) => return Err(err),
        };
        let i = self.list.len();
        self.list.push(Texture { watch, ..texture });
        self.by_path.insert(path.to_path_buf(), i);
        Ok(i)
    }
}

/// Decodes the image at `path`, with its rows from bottom to top.
pub(super) fn decode_texture(path: &Path) -> anyhow::Result<Texture> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    // The content sets the format, so a script can name a file `cover` and
    // write a JPEG or a PNG into it.
    let mut img = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(|r| r.decode())
        .with_context(|| format!("load texture {}", path.display()))?
        .into_rgba8();
    image::imageops::flip_vertical_in_place(&mut img);
    let (width, height) = img.dimensions();
    Ok(Texture {
        path: path.to_path_buf(),
        watch: false,
        modified,
        width,
        height,
        rgba: img.into_raw(),
    })
}

fn read_source(path: &Path) -> anyhow::Result<Source> {
    let code =
        std::fs::read_to_string(path).with_context(|| format!("read shader {}", path.display()))?;
    Ok(Source {
        path: path.to_path_buf(),
        code,
    })
}

pub(super) fn defines_main_image(code: &str) -> bool {
    code.contains("mainImage")
}

pub(super) fn is_pack_dir(path: &Path) -> bool {
    path.join(MANIFEST).is_file()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "procedural".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str = "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }";

    /// A new directory in the system temp directory. Drop removes it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("phonto-pack-test-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.0.join(name), contents).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn single_shader_file_uses_defaults() {
        let dir = TempDir::new("single");
        dir.write("plasma.glsl", SHADER);
        let pack = Pack::load(&dir.0.join("plasma.glsl")).unwrap();
        assert_eq!(pack.name, "plasma");
        assert!(!pack.audio);
        assert_eq!(pack.fps, Some(DEFAULT_FPS));
        assert_eq!(pack.render_scale, 1.0);
        assert!(pack.buffers.is_empty());
        assert!(pack.image.channels.is_empty());
    }

    #[test]
    fn shader_without_main_image_points_at_shader_flag() {
        let dir = TempDir::new("post");
        dir.write("edge.glsl", "uniform sampler2D u_tex; void main() {}");
        let err = Pack::load(&dir.0.join("edge.glsl")).unwrap_err();
        assert!(format!("{err:#}").contains("--shader"));
    }

    #[test]
    fn manifest_resolves_buffers_and_feedback() {
        let dir = TempDir::new("manifest");
        dir.write("a.glsl", SHADER);
        dir.write("b.glsl", SHADER);
        dir.write("image.glsl", SHADER);
        dir.write(
            MANIFEST,
            r#"
            name = "Test"
            audio = true
            fps = 0
            render_scale = 0.5

            [[buffer]]
            name = "a"
            shader = "a.glsl"
            channels = ["a", "b"]

            [[buffer]]
            name = "b"
            shader = "b.glsl"
            channels = [{ buffer = "a", filter = "nearest", wrap = "repeat" }]

            [image]
            shader = "image.glsl"
            channels = ["b"]
            "#,
        );
        let pack = Pack::load(&dir.0).unwrap();
        assert_eq!(pack.name, "Test");
        assert!(pack.audio);
        assert_eq!(pack.fps, None);
        assert_eq!(pack.render_scale, 0.5);
        assert_eq!(pack.buffers.len(), 2);
        assert_eq!(pack.buffers[0].size, BufferSize::Image);
        assert_eq!(pack.buffers[0].format, BufferFormat::Rgba16f);
        assert_eq!(
            pack.buffers[0].pass.channels[0],
            Channel {
                input: Input::Buffer(0),
                filter: Filter::Linear,
                wrap: Wrap::Clamp
            }
        );
        assert_eq!(pack.buffers[0].pass.channels[1].input, Input::Buffer(1));
        assert_eq!(
            pack.buffers[1].pass.channels[0],
            Channel {
                input: Input::Buffer(0),
                filter: Filter::Nearest,
                wrap: Wrap::Repeat
            }
        );
        assert_eq!(pack.image.channels[0].input, Input::Buffer(1));
    }

    #[test]
    fn buffer_size_and_format() {
        let dir = TempDir::new("buffer-size");
        dir.write("s.glsl", SHADER);
        dir.write(
            MANIFEST,
            r#"
            [[buffer]]
            name = "state"
            shader = "s.glsl"
            size = [16, 2]
            format = "rgba32f"

            [[buffer]]
            name = "sim"
            shader = "s.glsl"
            height = 300
            format = "rgba8"
            once = true

            [image]
            shader = "s.glsl"
            "#,
        );
        let pack = Pack::load(&dir.0).unwrap();
        assert_eq!(pack.buffers[0].size, BufferSize::Fixed(16, 2));
        assert_eq!(pack.buffers[0].format, BufferFormat::Rgba32f);
        assert_eq!(pack.buffers[1].size, BufferSize::Height(300));
        assert_eq!(pack.buffers[1].format, BufferFormat::Rgba8);
        assert!(!pack.buffers[0].once);
        assert!(pack.buffers[1].once);

        assert_eq!(BufferSize::Image.dims((1920, 1080)), (1920, 1080));
        assert_eq!(BufferSize::Fixed(16, 2).dims((1920, 1080)), (16, 2));
        assert_eq!(BufferSize::Height(300).dims((1920, 1080)), (533, 300));
        assert_eq!(BufferSize::Height(300).dims((3840, 2160)), (533, 300));
        assert_eq!(BufferSize::Height(300).dims((1080, 1920)), (169, 300));
    }

    #[test]
    fn textures_load_flipped_and_deduplicated() {
        let dir = TempDir::new("textures");
        dir.write("image.glsl", SHADER);
        // 1x2 image: top row red, bottom row blue.
        let mut img = image::RgbaImage::new(1, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.save(dir.0.join("tex.png")).unwrap();
        dir.write(
            MANIFEST,
            r#"
            [image]
            shader = "image.glsl"
            channels = ["tex.png", { texture = "tex.png", filter = "linear" }]
            "#,
        );
        let pack = Pack::load(&dir.0.join(MANIFEST)).unwrap();
        assert_eq!(pack.textures.len(), 1);
        assert_eq!(pack.image.channels[0].input, Input::Texture(0));
        assert_eq!(pack.image.channels[0].filter, Filter::Mipmap);
        assert_eq!(pack.image.channels[0].wrap, Wrap::Repeat);
        assert_eq!(pack.image.channels[1].filter, Filter::Linear);
        // The first stored row is the bottom row of the image.
        assert_eq!(&pack.textures[0].rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn inputs_and_watched_textures() {
        let dir = TempDir::new("inputs");
        dir.write("i.glsl", SHADER);
        dir.write(
            MANIFEST,
            r#"
            [image]
            shader = "i.glsl"
            channels = [{ texture = "cover.png", watch = true }]

            [inputs]
            rain = 0.5
            wind = [1, 2]
            "#,
        );
        let pack = Pack::load(&dir.0).unwrap();
        let named = |name: &str, size: usize, default: [f32; 4]| NamedInput {
            name: name.into(),
            size,
            default,
        };
        assert_eq!(
            pack.inputs,
            [
                named("rain", 1, [0.5, 0.0, 0.0, 0.0]),
                named("wind", 2, [1.0, 2.0, 0.0, 0.0]),
            ]
        );
        // The file does not exist yet, so the channel starts empty.
        let cover = &pack.textures[0];
        assert!(cover.watch && cover.modified.is_none());
        assert_eq!((cover.width, cover.height), (1, 1));

        // When a command writes the file, a reload finds it.
        let rgba = vec![255; 3 * 2 * 4];
        image::save_buffer(
            dir.0.join("cover.png"),
            &rgba,
            3,
            2,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let mut pack = pack;
        pack.reload_watched();
        let cover = &pack.textures[0];
        assert!(cover.watch && cover.modified.is_some());
        assert_eq!((cover.width, cover.height), (3, 2));
    }

    #[test]
    fn videos_and_actions() {
        let dir = TempDir::new("videos-actions");
        dir.write("i.glsl", SHADER);
        dir.write("clip.mkv", "not decoded at load");
        dir.write(
            MANIFEST,
            r#"
            [[buffer]]
            name = "pick"
            shader = "i.glsl"
            format = "rgba8"
            once = true

            [image]
            shader = "i.glsl"
            channels = [{ video = "clip.mkv" }, { video = "clip.mkv", filter = "nearest" }, "pick"]

            [actions]
            buffer = "pick"
            names = ["tape1", "open-term"]
            "#,
        );
        let pack = Pack::load(&dir.0).unwrap();
        assert_eq!(pack.videos, [dir.0.join("clip.mkv")]);
        assert_eq!(
            pack.image.channels[0],
            Channel {
                input: Input::Video(0),
                filter: Filter::Linear,
                wrap: Wrap::Clamp
            }
        );
        assert_eq!(pack.image.channels[1].input, Input::Video(0));
        assert_eq!(pack.image.channels[1].filter, Filter::Nearest);
        assert_eq!(
            pack.actions,
            Some(Actions {
                buffer: 0,
                names: vec!["tape1".into(), "open-term".into()],
            })
        );
    }

    #[test]
    fn rejects_invalid_manifests() {
        let cases = [
            ("fps = -1\n[image]\nshader = \"i.glsl\"", "fps"),
            (
                "render_scale = 2.0\n[image]\nshader = \"i.glsl\"",
                "render_scale",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\n[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\n[image]\nshader = \"i.glsl\"",
                "named `a`",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ buffer = \"nope\" }]",
                "no [[buffer]] is named `nope`",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\n[image]\nshader = \"i.glsl\"\nchannels = [{ buffer = \"a\", filter = \"mipmap\" }]",
                "only supported for textures",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ buffer = \"a\", texture = \"t.png\" }]",
                "exactly one",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [\"a\", \"b\", \"c\", \"d\", \"e\", \"f\", \"g\", \"h\", \"i\"]",
                "at most 8 channels",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\nsize = [4, 4]\nheight = 4\n[image]\nshader = \"i.glsl\"",
                "at most one of `size` or `height`",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\nsize = [0, 4]\n[image]\nshader = \"i.glsl\"",
                "between 1 and",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\nformat = \"rgb565\"\n[image]\nshader = \"i.glsl\"",
                "unknown format",
            ),
            (
                "[[buffer]]\nname = \"a\"\nshader = \"i.glsl\"\n[image]\nshader = \"i.glsl\"\nchannels = [{ buffer = \"a\", watch = true }]",
                "only for textures",
            ),
            (
                "[image]\nshader = \"i.glsl\"\n[inputs]\n2d = 1.0",
                "not a GLSL name",
            ),
            (
                "[image]\nshader = \"i.glsl\"\n[inputs]\ngl_rain = 1.0",
                "not a GLSL name",
            ),
            (
                "[image]\nshader = \"i.glsl\"\n[inputs]\nrain = \"heavy\"",
                "a number or a list",
            ),
            (
                "[image]\nshader = \"i.glsl\"\n[inputs]\nrain = [1, 2, 3, 4, 5]",
                "Use 1 to 4",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ video = \"missing.mkv\" }]",
                "does not exist",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ video = \"i.glsl\", filter = \"mipmap\" }]",
                "only supported for textures",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ video = \"i.glsl\", watch = true }]",
                "only for textures",
            ),
            (
                "[image]\nshader = \"i.glsl\"\nchannels = [{ video = \"i.glsl\", texture = \"t.png\" }]",
                "exactly one",
            ),
            (
                "[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\nnames = [\"a\"]",
                "no [[buffer]] is named `pick`",
            ),
            (
                "[[buffer]]\nname = \"pick\"\nshader = \"i.glsl\"\n[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\nnames = [\"a\"]",
                "must set format = \"rgba8\"",
            ),
            (
                "[[buffer]]\nname = \"pick\"\nshader = \"i.glsl\"\nformat = \"rgba8\"\n[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\nnames = []",
                "1 to 255 actions",
            ),
            (
                "[[buffer]]\nname = \"pick\"\nshader = \"i.glsl\"\nformat = \"rgba8\"\n[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\nnames = [\"a\", \"a\"]",
                "two actions are named `a`",
            ),
            (
                "[[buffer]]\nname = \"pick\"\nshader = \"i.glsl\"\nformat = \"rgba8\"\n[image]\nshader = \"i.glsl\"\n[actions]\nbuffer = \"pick\"\nnames = [\"open term\"]",
                "not a valid action name",
            ),
        ];
        for (i, (manifest, expected)) in cases.iter().enumerate() {
            let dir = TempDir::new(&format!("invalid-{i}"));
            dir.write("i.glsl", SHADER);
            dir.write(MANIFEST, manifest);
            let err = format!("{:#}", Pack::load(&dir.0).unwrap_err());
            assert!(
                err.contains(expected),
                "case {i}: expected `{expected}` in `{err}`"
            );
        }
    }
}
