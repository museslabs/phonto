//! Loads procedural wallpapers from disk. The README describes the format
//! of `manifest.toml`.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

pub const MANIFEST: &str = "manifest.toml";
pub const MAX_BUFFERS: usize = 8;
pub const MAX_CHANNELS: usize = 8;
/// The largest `size` or `height` of a buffer.
pub const MAX_BUFFER_SIZE: u32 = 8192;
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
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Texture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Texture")
            .field("path", &self.path)
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
    filter: Option<String>,
    wrap: Option<String>,
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

        let mut textures = TextureSet::default();
        let mut buffers = Vec::with_capacity(m.buffers.len());
        for b in &m.buffers {
            let buffer = build_buffer(b, dir, &buffer_index, &mut textures)
                .with_context(|| format!("[[buffer]] `{}`", b.name))?;
            buffers.push(buffer);
        }
        let image = build_pass(
            "image",
            &m.image.shader,
            &m.image.channels,
            dir,
            &buffer_index,
            &mut textures,
        )
        .context("[image]")?;

        Ok(Self {
            name: m.name.unwrap_or_else(|| file_stem(dir)),
            audio: m.audio,
            fps,
            render_scale,
            common,
            buffers,
            image,
            textures: textures.list,
        })
    }
}

fn build_buffer(
    b: &BufferManifest,
    dir: &Path,
    buffer_index: &HashMap<&str, usize>,
    textures: &mut TextureSet,
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
    let pass = build_pass(&b.name, &b.shader, &b.channels, dir, buffer_index, textures)?;
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
    textures: &mut TextureSet,
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
            resolve_channel(c, dir, buffer_index, textures).with_context(|| format!("iChannel{n}"))
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
    textures: &mut TextureSet,
) -> anyhow::Result<Channel> {
    let (buffer, texture, filter, wrap) = match c {
        ChannelManifest::Short(s) if buffer_index.contains_key(s.as_str()) => {
            (Some(s.as_str()), None, None, None)
        }
        ChannelManifest::Short(s) => (None, Some(s.as_str()), None, None),
        ChannelManifest::Full(t) => (
            t.buffer.as_deref(),
            t.texture.as_deref(),
            t.filter.as_deref(),
            t.wrap.as_deref(),
        ),
    };

    let (input, default_filter, default_wrap) = match (buffer, texture) {
        (Some(name), None) => {
            let Some(&i) = buffer_index.get(name) else {
                bail!("no [[buffer]] is named `{name}`");
            };
            (Input::Buffer(i), Filter::Linear, Wrap::Clamp)
        }
        (None, Some(path)) => {
            let i = textures.load(&dir.join(path))?;
            (Input::Texture(i), Filter::Mipmap, Wrap::Repeat)
        }
        _ => bail!("set exactly one of `buffer` or `texture`"),
    };

    let filter = filter
        .map(parse_filter)
        .transpose()?
        .unwrap_or(default_filter);
    let wrap = wrap.map(parse_wrap).transpose()?.unwrap_or(default_wrap);
    if filter == Filter::Mipmap && matches!(input, Input::Buffer(_)) {
        bail!("filter = \"mipmap\" is only supported for textures, not buffers");
    }
    Ok(Channel {
        input,
        filter,
        wrap,
    })
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

/// Decoded images, one for each path.
#[derive(Default)]
struct TextureSet {
    list: Vec<Texture>,
    by_path: HashMap<PathBuf, usize>,
}

impl TextureSet {
    fn load(&mut self, path: &Path) -> anyhow::Result<usize> {
        if let Some(&i) = self.by_path.get(path) {
            return Ok(i);
        }
        let mut img = image::open(path)
            .with_context(|| format!("load texture {}", path.display()))?
            .into_rgba8();
        image::imageops::flip_vertical_in_place(&mut img);
        let (width, height) = img.dimensions();
        let i = self.list.len();
        self.list.push(Texture {
            path: path.to_path_buf(),
            width,
            height,
            rgba: img.into_raw(),
        });
        self.by_path.insert(path.to_path_buf(), i);
        Ok(i)
    }
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
