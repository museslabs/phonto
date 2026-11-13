//! Draws procedural wallpapers with Metal.
//!
//! A [`Program`] is a pack, compiled once. Each screen gets its own
//! [`Target`], because screens have different sizes and buffers keep state
//! between frames.
//!
//! The passes run in the coordinates of Metal, unchanged. `fragCoord.y` is
//! then `k + 0.5` in row `k` of a texture, and row 0 is the first row that a
//! texture stores, as in GL. So a pass reads its buffers and images the same
//! way on both backends. Only the screen shows row 0 at the top instead of
//! the bottom. The image turns over on its way to the screen.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::time::SystemTime;

use anyhow::{Context, anyhow, bail};
use objc2::available;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLGPUFamily,
    MTLLibrary, MTLLoadAction, MTLMathMode, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter,
    MTLSamplerMipFilter, MTLSamplerState, MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};

use super::keyboard::KEYS;
use super::msl::{self, Translated};
use crate::backend::DumpRequest;
use crate::procedural::{
    self, BufferFormat, BufferSize, Filter, FrameInputs, Input, MAX_CHANNELS, MAX_DISPLAYS, Pack,
    Pass, PassKind, Texture, Wrap, local_date, location, read_inputs_once,
};

type Device = ProtocolObject<dyn MTLDevice>;
type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;
pub type GpuTexture = Retained<ProtocolObject<dyn MTLTexture>>;
type Sampler = Retained<ProtocolObject<dyn MTLSamplerState>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// The format of the image pass. It holds the same values as the RGBA8 image
/// of the Wayland backend.
const IMAGE_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA8Unorm;

/// The format of the drawables of a `CAMetalLayer` that shows a wallpaper.
pub const SCREEN_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// The image size of `dump` without `--size`, as on Wayland.
const DEFAULT_DUMP_SIZE: (u32, u32) = (1920, 1080);

/// The frame rate of `dump` for an uncapped wallpaper, as on Wayland. A
/// capped wallpaper steps at its cap. Its buffers then change as they do on
/// screen.
const UNCAPPED_DUMP_FPS: f64 = 60.0;

/// The shaders that every pack uses: a triangle that covers the target, and
/// the copy of the image to the screen.
const SHARED_SHADERS: &str = r#"
#include <metal_stdlib>
using namespace metal;

vertex float4 phonto_fullscreen(uint id [[vertex_id]]) {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * 2.0 - 1.0, 0.0, 1.0);
}

struct PresentVertex {
    float4 position [[position]];
    float2 uv;
};

// The bottom of the screen samples row 0 of the image.
vertex PresentVertex phonto_present_vertex(uint id [[vertex_id]]) {
    float2 p = float2((id << 1) & 2, id & 2);
    PresentVertex out;
    out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = p;
    return out;
}

fragment float4 phonto_present_fragment(
    PresentVertex in [[stage_in]],
    texture2d<float> image [[texture(0)]],
    sampler s [[sampler(0)]]
) {
    return image.sample(s, in.uv);
}
"#;

/// The Metal device, and what every program shares.
pub struct Gpu {
    pub device: Retained<Device>,
    pub queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    fullscreen: Retained<ProtocolObject<dyn MTLFunction>>,
    present: Pipeline,
    present_sampler: Sampler,
}

impl Gpu {
    pub fn new() -> anyhow::Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("no Metal device")?;
        let queue = device
            .newCommandQueue()
            .context("create a Metal command queue")?;
        let library = library(&device, SHARED_SHADERS).context("compile phonto's Metal shaders")?;
        let vertex = function(&library, "phonto_present_vertex")?;
        let fragment = function(&library, "phonto_present_fragment")?;
        let present = pipeline(&device, &vertex, &fragment, SCREEN_FORMAT)?;
        Ok(Self {
            fullscreen: function(&library, "phonto_fullscreen")?,
            present,
            present_sampler: sampler(&device, Filter::Nearest, Wrap::Clamp)?,
            device,
            queue,
        })
    }

    pub fn command_buffer(&self) -> anyhow::Result<Retained<CommandBuffer>> {
        self.queue
            .commandBuffer()
            .context("create a Metal command buffer")
    }

    /// Copies the image of `target` to `drawable`, which has the same size.
    pub fn encode_present(
        &self,
        cmd: &CommandBuffer,
        target: &Target,
        drawable: &ProtocolObject<dyn MTLTexture>,
    ) {
        let pass = MTLRenderPassDescriptor::new();
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(drawable));
        color.setLoadAction(MTLLoadAction::DontCare);
        color.setStoreAction(MTLStoreAction::Store);
        let Some(encoder) = cmd.renderCommandEncoderWithDescriptor(&pass) else {
            log::warn!("Metal could not start the copy to the screen");
            return;
        };
        encoder.setRenderPipelineState(&self.present);
        unsafe {
            encoder.setFragmentTexture_atIndex(Some(&target.image), 0);
            encoder.setFragmentSamplerState_atIndex(Some(&self.present_sampler), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        encoder.endEncoding();
    }
}

pub struct Program {
    /// The buffer passes in the order that they draw, then the image pass.
    passes: Vec<CompiledPass>,
    buffers: Vec<BufferTarget>,
    textures: Vec<UploadedTexture>,
    /// What a pass reads from a channel without an input.
    black: GpuTexture,
    default_sampler: Sampler,
    render_scale: f32,
}

/// How to make the render targets of one buffer pass.
struct BufferTarget {
    size: BufferSize,
    format: MTLPixelFormat,
    /// If true, the buffer draws only on the first frame of a target.
    once: bool,
}

struct CompiledPass {
    pipeline: Pipeline,
    /// `Load` if the pass discards. A discarded pixel keeps what the target
    /// held, as in GL. Otherwise the pass covers every pixel, and loading the
    /// old contents would only cost bandwidth.
    load: MTLLoadAction,
    uniforms: UniformLayout,
    channels: Vec<BoundChannel>,
    /// The texture and sampler indices of each channel that the pass reads.
    slots: [Option<(usize, usize)>; MAX_CHANNELS],
}

struct BoundChannel {
    input: Input,
    sampler: Sampler,
}

struct UploadedTexture {
    texture: GpuTexture,
    width: u32,
    height: u32,
}

/// Where each uniform that phonto sets is in the uniform block of a pass. A
/// field is `None` if the block has no such uniform.
#[derive(Default)]
struct UniformLayout {
    buffer: Option<usize>,
    size: usize,
    resolution: Option<usize>,
    image_resolution: Option<usize>,
    time: Option<usize>,
    time_delta: Option<usize>,
    frame_rate: Option<usize>,
    frame: Option<usize>,
    mouse: Option<usize>,
    date: Option<usize>,
    sample_rate: Option<usize>,
    pointer: Option<usize>,
    audio: Option<usize>,
    color_scheme: Option<usize>,
    battery: Option<usize>,
    location: Option<usize>,
    display_count: Option<usize>,
    /// The offset and the array stride.
    channel_time: Option<(usize, usize)>,
    channel_resolution: Option<(usize, usize)>,
    spectrum: Option<(usize, usize)>,
    chroma: Option<(usize, usize)>,
    displays: Option<(usize, usize)>,
    /// `iInput.<name>` for each input of the pack, and its size.
    inputs: Vec<(Option<usize>, usize)>,
}

/// What every pass of a frame reads.
#[derive(Clone, Copy)]
struct Frame<'a> {
    target: &'a Target,
    inputs: &'a FrameInputs,
    keyboard: Option<&'a ProtocolObject<dyn MTLTexture>>,
}

/// The render targets and the frame count of one screen.
pub struct Target {
    dims: (u32, u32),
    buffers: Vec<PingPong>,
    image: GpuTexture,
    frame: i32,
}

/// The output of a buffer pass. The pass draws into one slot. The passes read
/// the other slot, which holds the last finished frame.
struct PingPong {
    slots: [GpuTexture; 2],
    current: usize,
    dims: (u32, u32),
}

impl Program {
    pub fn compile(gpu: &Gpu, pack: &Pack) -> anyhow::Result<Self> {
        Self::build(gpu, pack).with_context(|| format!("procedural wallpaper `{}`", pack.name))
    }

    fn build(gpu: &Gpu, pack: &Pack) -> anyhow::Result<Self> {
        let buffers: Vec<BufferTarget> = pack
            .buffers
            .iter()
            .map(|b| BufferTarget {
                size: b.size,
                format: pixel_format(b.format),
                once: b.once,
            })
            .collect();
        let textures = pack
            .textures
            .iter()
            .map(|t| upload_texture(gpu, t))
            .collect::<anyhow::Result<_>>()?;

        let mut samplers = Samplers::default();
        let mut passes = Vec::with_capacity(pack.buffers.len() + 1);
        for (i, buffer) in pack.buffers.iter().enumerate() {
            let format = buffers[i].format;
            let pass = compile_pass(
                gpu,
                pack,
                &buffer.pass,
                PassKind::Buffer,
                format,
                &mut samplers,
            )?;
            passes.push(pass);
        }
        let image = compile_pass(
            gpu,
            pack,
            &pack.image,
            PassKind::Image,
            IMAGE_FORMAT,
            &mut samplers,
        )?;
        passes.push(image);

        Ok(Self {
            passes,
            buffers,
            textures,
            black: black_texture(gpu)?,
            default_sampler: samplers.get(&gpu.device, Filter::Linear, Wrap::Clamp)?,
            render_scale: pack.render_scale,
        })
    }

    /// Uploads a new image for `Pack::textures[index]`, for a watched file
    /// that changed.
    pub fn replace_texture(
        &mut self,
        gpu: &Gpu,
        index: usize,
        texture: &Texture,
    ) -> anyhow::Result<()> {
        let slot = self
            .textures
            .get_mut(index)
            .with_context(|| format!("no texture {index}"))?;
        *slot = upload_texture(gpu, texture)?;
        Ok(())
    }

    /// The size of the render target for a screen of `surface` points.
    pub fn target_dims(&self, surface: (u32, u32)) -> (u32, u32) {
        let scale = |v: u32| ((v as f32 * self.render_scale).round() as u32).max(1);
        (scale(surface.0), scale(surface.1))
    }

    /// Makes the render targets for an image of `dims` pixels. The buffers
    /// start at zero. If `readable` is true, the CPU can read the image.
    pub fn create_target(
        &self,
        gpu: &Gpu,
        dims: (u32, u32),
        readable: bool,
    ) -> anyhow::Result<Target> {
        let mut buffers = Vec::with_capacity(self.buffers.len());
        for b in &self.buffers {
            let buffer_dims = b.size.dims(dims);
            buffers.push(PingPong {
                slots: [
                    render_target(gpu, b.format, buffer_dims, false)?,
                    render_target(gpu, b.format, buffer_dims, false)?,
                ],
                current: 0,
                dims: buffer_dims,
            });
        }
        let image = render_target(gpu, IMAGE_FORMAT, dims, readable)?;

        // A new texture holds whatever was in its memory. A buffer that reads
        // its last frame must read zeros on the first frame, as on Wayland.
        // A pass that discards keeps the old pixels. The image starts clear
        // too.
        let cmd = gpu.command_buffer()?;
        for texture in buffers.iter().flat_map(|pp| &pp.slots) {
            clear(&cmd, texture);
        }
        clear(&cmd, &image);
        cmd.commit();

        Ok(Target {
            dims,
            buffers,
            image,
            frame: 0,
        })
    }

    /// Encodes every pass of one frame. The image of `target` then holds the
    /// frame. `keyboard` is the keyboard texture, if the wallpaper has one.
    pub fn encode(
        &self,
        cmd: &CommandBuffer,
        target: &mut Target,
        inputs: &FrameInputs,
        keyboard: Option<&ProtocolObject<dyn MTLTexture>>,
    ) {
        let buffer_count = self.buffers.len();
        for i in 0..buffer_count {
            // A `once` buffer keeps what it drew on the first frame.
            // `current` still points to that slot.
            if self.buffers[i].once && target.frame > 0 {
                continue;
            }
            let pp = &target.buffers[i];
            let output = pp.slots[1 - pp.current].clone();
            let frame = Frame {
                target,
                inputs,
                keyboard,
            };
            self.draw_pass(cmd, &self.passes[i], &output, pp.dims, &frame);
            target.buffers[i].current ^= 1;
        }
        let frame = Frame {
            target,
            inputs,
            keyboard,
        };
        let pass = &self.passes[buffer_count];
        self.draw_pass(cmd, pass, &target.image, target.dims, &frame);
        target.frame = target.frame.wrapping_add(1);
    }

    /// Draws one pass into `output`, which is `dims` pixels.
    fn draw_pass(
        &self,
        cmd: &CommandBuffer,
        pass: &CompiledPass,
        output: &ProtocolObject<dyn MTLTexture>,
        dims: (u32, u32),
        frame: &Frame,
    ) {
        let Frame {
            target,
            inputs,
            keyboard,
        } = *frame;
        let descriptor = MTLRenderPassDescriptor::new();
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(output));
        color.setLoadAction(pass.load);
        color.setStoreAction(MTLStoreAction::Store);
        let Some(encoder) = cmd.renderCommandEncoderWithDescriptor(&descriptor) else {
            log::warn!("Metal could not start a pass");
            return;
        };
        encoder.setRenderPipelineState(&pass.pipeline);

        let mut resolutions = [[0.0f32; 3]; MAX_CHANNELS];
        for (unit, slot) in pass.slots.iter().enumerate() {
            let (texture, sampler) = match pass.channels.get(unit) {
                Some(channel) => {
                    let (texture, (w, h)): (&ProtocolObject<dyn MTLTexture>, _) =
                        match channel.input {
                            Input::Buffer(i) => {
                                let pp = &target.buffers[i];
                                (&pp.slots[pp.current], pp.dims)
                            }
                            Input::Texture(i) => {
                                let t = &self.textures[i];
                                (&t.texture, (t.width, t.height))
                            }
                            Input::Keyboard => {
                                let dims = (KEYS as u32, 3);
                                (keyboard.unwrap_or(&self.black), dims)
                            }
                        };
                    resolutions[unit] = [w as f32, h as f32, 1.0];
                    (texture, &channel.sampler)
                }
                // A channel without an input samples as black.
                None => (&*self.black, &self.default_sampler),
            };
            if let Some((texture_index, sampler_index)) = *slot {
                unsafe {
                    encoder.setFragmentTexture_atIndex(Some(texture), texture_index);
                    encoder.setFragmentSamplerState_atIndex(Some(sampler), sampler_index);
                }
            }
        }

        if let Some(index) = pass.uniforms.buffer {
            let bytes = pass
                .uniforms
                .bytes(dims, target.dims, target.frame, inputs, &resolutions);
            let ptr = NonNull::new(bytes.as_ptr().cast_mut().cast::<c_void>())
                .expect("a Vec has a non-null pointer");
            unsafe { encoder.setFragmentBytes_length_atIndex(ptr, bytes.len(), index) };
        }
        unsafe { encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
        encoder.endEncoding();
    }
}

impl Target {
    pub fn dims(&self) -> (u32, u32) {
        self.dims
    }

    /// The image as RGBA rows from the top of the picture to the bottom. The
    /// target must be readable, and the GPU must have finished the frame.
    fn read_image(&self) -> image::RgbaImage {
        let (w, h) = self.dims;
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        let ptr =
            NonNull::new(rgba.as_mut_ptr().cast::<c_void>()).expect("a Vec has a non-null pointer");
        unsafe {
            self.image.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                ptr,
                w as usize * 4,
                region(self.dims),
                0,
            );
        }
        let mut image = image::RgbaImage::from_raw(w, h, rgba).expect("the buffer fits the image");
        // Row 0 is the bottom of the picture.
        image::imageops::flip_vertical_in_place(&mut image);
        image
    }
}

impl UniformLayout {
    fn new(t: &Translated, pack: &Pack) -> Self {
        let offset = |name: &str| t.uniforms.iter().find(|m| m.name == name).map(|m| m.offset);
        let array = |name: &str| {
            t.uniforms
                .iter()
                .find(|m| m.name == name)
                .map(|m| (m.offset, m.stride))
        };
        Self {
            buffer: t.uniform_buffer,
            // Metal wants the bytes of a buffer in whole 16-byte rows.
            size: t.uniform_size.max(16).next_multiple_of(16),
            resolution: offset("iResolution"),
            image_resolution: offset("iImageResolution"),
            time: offset("iTime"),
            time_delta: offset("iTimeDelta"),
            frame_rate: offset("iFrameRate"),
            frame: offset("iFrame"),
            mouse: offset("iMouse"),
            date: offset("iDate"),
            sample_rate: offset("iSampleRate"),
            pointer: offset("iPointer"),
            audio: offset("iAudio"),
            color_scheme: offset("iColorScheme"),
            battery: offset("iBattery"),
            location: offset("iLocation"),
            display_count: offset("iDisplayCount"),
            channel_time: array("iChannelTime"),
            channel_resolution: array("iChannelResolution"),
            spectrum: array("iSpectrum"),
            chroma: array("iChroma"),
            displays: array("iDisplays"),
            inputs: pack
                .inputs
                .iter()
                .map(|i| (offset(&format!("iInput.{}", i.name)), i.size))
                .collect(),
        }
    }

    /// The uniform block of a pass of `dims` pixels.
    fn bytes(
        &self,
        dims: (u32, u32),
        image: (u32, u32),
        frame: i32,
        inputs: &FrameInputs,
        resolutions: &[[f32; 3]; MAX_CHANNELS],
    ) -> Vec<u8> {
        let mut b = vec![0u8; self.size];
        put(
            &mut b,
            self.resolution,
            &[dims.0 as f32, dims.1 as f32, 1.0],
        );
        put(
            &mut b,
            self.image_resolution,
            &[image.0 as f32, image.1 as f32, 1.0],
        );
        put(&mut b, self.time, &[inputs.time]);
        put(&mut b, self.time_delta, &[inputs.time_delta]);
        put(&mut b, self.frame_rate, &[inputs.frame_rate]);
        put(&mut b, self.mouse, &inputs.mouse);
        put(&mut b, self.date, &inputs.date);
        put(&mut b, self.sample_rate, &[44_100.0]);
        put(&mut b, self.pointer, &inputs.pointer);
        put(&mut b, self.audio, &inputs.audio.levels);
        put(&mut b, self.color_scheme, &[inputs.color_scheme]);
        put(&mut b, self.battery, &inputs.battery);
        put(&mut b, self.location, &inputs.location);
        put_int(&mut b, self.frame, frame);
        put_int(&mut b, self.display_count, inputs.display_count);
        for (unit, resolution) in resolutions.iter().enumerate() {
            if let Some((at, stride)) = self.channel_time {
                put(&mut b, Some(at + unit * stride), &[inputs.time]);
            }
            if let Some((at, stride)) = self.channel_resolution {
                put(&mut b, Some(at + unit * stride), resolution);
            }
        }
        put_array(&mut b, self.spectrum, inputs.audio.spectrum.map(|v| [v]));
        put_array(&mut b, self.chroma, inputs.audio.chroma.map(|v| [v]));
        put_array(&mut b, self.displays, inputs.displays);
        for (&(at, size), value) in self.inputs.iter().zip(&inputs.inputs) {
            put(&mut b, at, &value[..size]);
        }
        b
    }
}

/// Writes `values` at byte `at` of `b`, if the uniform exists.
fn put(b: &mut [u8], at: Option<usize>, values: &[f32]) {
    let Some(at) = at else {
        return;
    };
    for (k, v) in values.iter().enumerate() {
        if let Some(dst) = b.get_mut(at + 4 * k..at + 4 * k + 4) {
            dst.copy_from_slice(&v.to_ne_bytes());
        }
    }
}

fn put_int(b: &mut [u8], at: Option<usize>, value: i32) {
    if let Some(at) = at
        && let Some(dst) = b.get_mut(at..at + 4)
    {
        dst.copy_from_slice(&value.to_ne_bytes());
    }
}

/// Writes each element of an array at its stride, if the array exists.
fn put_array<const N: usize>(
    b: &mut [u8],
    array: Option<(usize, usize)>,
    elements: impl IntoIterator<Item = [f32; N]>,
) {
    let Some((at, stride)) = array else {
        return;
    };
    for (k, element) in elements.into_iter().enumerate() {
        put(b, Some(at + k * stride), &element);
    }
}

fn compile_pass(
    gpu: &Gpu,
    pack: &Pack,
    pass: &Pass,
    kind: PassKind,
    format: MTLPixelFormat,
    samplers: &mut Samplers,
) -> anyhow::Result<CompiledPass> {
    let common = pack.common.as_ref();
    let glsl = procedural::fragment_shader(
        common.map(|c| c.code.as_str()),
        &pass.source.code,
        kind,
        &pack.inputs,
    );
    let translated = msl::translate(&glsl).with_context(|| {
        format!(
            "compile pass `{}` ({})",
            pass.name,
            procedural::source_ids_help(
                &pass.source.path.display().to_string(),
                common.map(|c| c.path.display().to_string()).as_deref(),
            )
        )
    })?;
    let library = library(&gpu.device, &translated.source)
        .with_context(|| format!("pass `{}`: Metal rejected the translated shader", pass.name))?;
    let fragment = function(&library, &translated.entry_point)?;
    let pipeline = pipeline(&gpu.device, &gpu.fullscreen, &fragment, format)
        .with_context(|| format!("pass `{}`", pass.name))?;

    let float_filtering = gpu.device.supports32BitFloatFiltering();
    let mut channels = Vec::with_capacity(pass.channels.len());
    for c in &pass.channels {
        let mut filter = c.filter;
        if let Input::Buffer(i) = c.input
            && pack.buffers[i].format == BufferFormat::Rgba32f
            && filter != Filter::Nearest
            && !float_filtering
        {
            log::warn!(
                "pass `{}`: the GPU cannot filter 32-bit float buffers. Buffer `{}` uses nearest filtering.",
                pass.name,
                pack.buffers[i].pass.name
            );
            filter = Filter::Nearest;
        }
        channels.push(BoundChannel {
            input: c.input,
            sampler: samplers.get(&gpu.device, filter, c.wrap)?,
        });
    }

    let discards = translated.source.contains("discard_fragment");
    Ok(CompiledPass {
        pipeline,
        load: if discards {
            MTLLoadAction::Load
        } else {
            MTLLoadAction::DontCare
        },
        uniforms: UniformLayout::new(&translated, pack),
        channels,
        slots: translated.channels,
    })
}

/// Sampler states, made at their first use.
#[derive(Default)]
struct Samplers(Vec<((Filter, Wrap), Sampler)>);

impl Samplers {
    fn get(&mut self, device: &Device, filter: Filter, wrap: Wrap) -> anyhow::Result<Sampler> {
        if let Some((_, s)) = self.0.iter().find(|(k, _)| *k == (filter, wrap)) {
            return Ok(s.clone());
        }
        let s = sampler(device, filter, wrap)?;
        self.0.push(((filter, wrap), s.clone()));
        Ok(s)
    }
}

fn sampler(device: &Device, filter: Filter, wrap: Wrap) -> anyhow::Result<Sampler> {
    let descriptor = MTLSamplerDescriptor::new();
    let min_mag = match filter {
        Filter::Nearest => MTLSamplerMinMagFilter::Nearest,
        Filter::Linear | Filter::Mipmap => MTLSamplerMinMagFilter::Linear,
    };
    descriptor.setMinFilter(min_mag);
    descriptor.setMagFilter(min_mag);
    descriptor.setMipFilter(match filter {
        Filter::Mipmap => MTLSamplerMipFilter::Linear,
        Filter::Nearest | Filter::Linear => MTLSamplerMipFilter::NotMipmapped,
    });
    let address = match wrap {
        Wrap::Clamp => MTLSamplerAddressMode::ClampToEdge,
        Wrap::Repeat => MTLSamplerAddressMode::Repeat,
        Wrap::Mirror => MTLSamplerAddressMode::MirrorRepeat,
    };
    descriptor.setSAddressMode(address);
    descriptor.setTAddressMode(address);
    device
        .newSamplerStateWithDescriptor(&descriptor)
        .context("create a Metal sampler")
}

fn pixel_format(format: BufferFormat) -> MTLPixelFormat {
    match format {
        BufferFormat::Rgba8 => MTLPixelFormat::RGBA8Unorm,
        BufferFormat::Rgba16f => MTLPixelFormat::RGBA16Float,
        BufferFormat::Rgba32f => MTLPixelFormat::RGBA32Float,
    }
}

fn region((w, h): (u32, u32)) -> MTLRegion {
    MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: w as usize,
            height: h as usize,
            depth: 1,
        },
    }
}

fn render_target(
    gpu: &Gpu,
    format: MTLPixelFormat,
    (w, h): (u32, u32),
    readable: bool,
) -> anyhow::Result<GpuTexture> {
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, w as usize, h as usize, false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(if readable {
        MTLStorageMode::Shared
    } else {
        MTLStorageMode::Private
    });
    gpu.device
        .newTextureWithDescriptor(&descriptor)
        .with_context(|| format!("create a {w}x{h} render target"))
}

fn clear(cmd: &CommandBuffer, texture: &ProtocolObject<dyn MTLTexture>) {
    let pass = MTLRenderPassDescriptor::new();
    let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(texture));
    color.setLoadAction(MTLLoadAction::Clear);
    color.setClearColor(MTLClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    });
    color.setStoreAction(MTLStoreAction::Store);
    if let Some(encoder) = cmd.renderCommandEncoderWithDescriptor(&pass) {
        encoder.endEncoding();
    }
}

/// Uploads an image of a pack, with mipmaps for `filter = "mipmap"`.
fn upload_texture(gpu: &Gpu, t: &Texture) -> anyhow::Result<UploadedTexture> {
    let texture = upload(
        gpu,
        MTLPixelFormat::RGBA8Unorm,
        (t.width, t.height),
        &t.rgba,
        true,
    )
    .with_context(|| format!("texture {} ({}x{})", t.path.display(), t.width, t.height))?;
    let cmd = gpu.command_buffer()?;
    let blit = cmd
        .blitCommandEncoder()
        .context("create a Metal blit encoder")?;
    blit.generateMipmapsForTexture(&texture);
    blit.endEncoding();
    cmd.commit();
    Ok(UploadedTexture {
        texture,
        width: t.width,
        height: t.height,
    })
}

/// Opaque black, which is what GL samples from a texture unit with no
/// texture.
fn black_texture(gpu: &Gpu) -> anyhow::Result<GpuTexture> {
    upload(
        gpu,
        MTLPixelFormat::RGBA8Unorm,
        (1, 1),
        &[0, 0, 0, 255],
        false,
    )
}

/// The keyboard texture, from the rows of `Keyboard::take_frame`.
pub fn keyboard_texture(gpu: &Gpu, rows: &[u8; KEYS * 3]) -> anyhow::Result<GpuTexture> {
    upload(gpu, MTLPixelFormat::R8Unorm, (KEYS as u32, 3), rows, false)
}

/// A texture of `format` that holds `data`, rows packed without padding.
fn upload(
    gpu: &Gpu,
    format: MTLPixelFormat,
    (w, h): (u32, u32),
    data: &[u8],
    mipmapped: bool,
) -> anyhow::Result<GpuTexture> {
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, w as usize, h as usize, mipmapped,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(MTLStorageMode::Shared);
    let Some(texture) = gpu.device.newTextureWithDescriptor(&descriptor) else {
        bail!("the GPU cannot make a {w}x{h} texture");
    };
    let ptr = NonNull::new(data.as_ptr().cast_mut().cast::<c_void>())
        .expect("a slice has a non-null pointer");
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            region((w, h)),
            0,
            ptr,
            data.len() / h as usize,
        );
    }
    Ok(texture)
}

fn library(
    device: &Device,
    source: &str,
) -> anyhow::Result<Retained<ProtocolObject<dyn MTLLibrary>>> {
    let options = MTLCompileOptions::new();
    // Fast math, the default, lets the compiler assume that no value is NaN
    // or infinite. A check such as `isnan(v)` can then come out false for a
    // NaN. Relaxed keeps NaN and infinity and costs koi about 3% more GPU
    // time on an M5. Before macOS 15, and on Intel and AMD GPUs, the math
    // stays fast. The cost of Relaxed there is unmeasured.
    if available!(macos = 15.0) && device.supportsFamily(MTLGPUFamily::Apple1) {
        options.setMathMode(MTLMathMode::Relaxed);
    }
    device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|e| anyhow!("{}", e.localizedDescription()))
}

fn function(
    library: &ProtocolObject<dyn MTLLibrary>,
    name: &str,
) -> anyhow::Result<Retained<ProtocolObject<dyn MTLFunction>>> {
    library
        .newFunctionWithName(&NSString::from_str(name))
        .with_context(|| format!("no Metal function `{name}`"))
}

fn pipeline(
    device: &Device,
    vertex: &ProtocolObject<dyn MTLFunction>,
    fragment: &ProtocolObject<dyn MTLFunction>,
    format: MTLPixelFormat,
) -> anyhow::Result<Pipeline> {
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setVertexFunction(Some(vertex));
    descriptor.setFragmentFunction(Some(fragment));
    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(format);
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|e| anyhow!("create a Metal pipeline: {}", e.localizedDescription()))
}

/// `iDisplays` and `iDisplayCount` for an image of `dims` pixels. macOS does
/// not span a wallpaper across screens. The image is one display.
pub fn one_display((w, h): (u32, u32)) -> ([[f32; 4]; MAX_DISPLAYS], i32) {
    let mut displays = [[0.0; 4]; MAX_DISPLAYS];
    displays[0] = [0.0, 0.0, w as f32, h as f32];
    (displays, 1)
}

/// `phonto dump` for a procedural wallpaper. It steps the wallpaper from 0 to
/// `--at` seconds, as on Wayland. Buffers that keep state get there the same
/// way.
pub fn dump(request: DumpRequest) -> anyhow::Result<()> {
    let mut pack = Pack::load(Path::new(&request.path))?;
    // An input command can write an image that the pack watches. The
    // commands run first, and then the watched images load again.
    let user_inputs = read_inputs_once(&pack.inputs, &request.inputs);
    pack.reload_watched();
    let gpu = Gpu::new()?;
    let program = Program::compile(&gpu, &pack)?;
    let (width, height) = request.size.unwrap_or(DEFAULT_DUMP_SIZE);
    let dims = program.target_dims((width, height));
    let mut target = program.create_target(&gpu, dims, true)?;

    let fps = pack.fps.map_or(UNCAPPED_DUMP_FPS, f64::from);
    let frames = (request.at * fps).floor() as u64;
    let wall = SystemTime::now();
    let (displays, display_count) = one_display(dims);
    // dump uses the inputs of this moment, as it does for iDate.
    let system = FrameInputs {
        date: local_date(wall),
        location: location(wall, request.location),
        inputs: user_inputs,
        displays,
        display_count,
        ..FrameInputs::default()
    };
    let mut previous = 0.0;
    let mut last = None;
    for i in 0..=frames {
        let time = if i == frames {
            request.at
        } else {
            i as f64 / fps
        };
        let inputs = FrameInputs {
            time: time as f32,
            time_delta: (time - previous) as f32,
            frame_rate: fps as f32,
            ..system
        };
        previous = time;
        let cmd = gpu.command_buffer()?;
        program.encode(&cmd, &mut target, &inputs, None);
        cmd.commit();
        last = Some(cmd);
    }
    let cmd = last.expect("rendered at least one frame");
    cmd.waitUntilCompleted();
    if let Some(e) = cmd.error() {
        bail!(
            "the GPU failed to draw the frame: {}",
            e.localizedDescription()
        );
    }

    let mut image = target.read_image();
    // A render scale below 1 draws fewer pixels. Scale them up to the
    // requested size, as the screen would.
    if image.dimensions() != (width, height) {
        image =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
    }
    image
        .save(&request.out)
        .with_context(|| format!("save frame to {}", request.out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metal declares these names, and GLSL does not. Before phonto renamed
    /// the functions and structs of a shader, each one failed in Metal.
    #[test]
    fn metal_compiles_shaders_that_use_metal_names() {
        let code = "
            struct complex { vec2 v; };
            float quad(vec2 a) { return a.x; }
            float atan2(float y, float x) { return atan(y, x); }
            float log10(float x) { return log(x) / log(10.0); }
            float rsqrt(float x) { return inversesqrt(x); }
            float select(float a, float b, bool c) { return c ? b : a; }
            float powr(float a, float b) { return pow(a, b); }
            void mainImage(out vec4 c, in vec2 p) {
                complex z = complex(p);
                c = vec4(quad(z.v), atan2(p.y, p.x), log10(p.x) + rsqrt(p.y),
                         select(powr(p.x, 2.0), 0.0, false));
            }";
        let gpu = Gpu::new().unwrap();
        let glsl = procedural::fragment_shader(None, code, PassKind::Image, &[]);
        let translated = msl::translate(&glsl).unwrap();
        library(&gpu.device, &translated.source).unwrap();
    }

    /// The inputs of the pack, with a default and from a command, and the
    /// display of the image reach the shader.
    #[test]
    fn dump_draws_the_inputs_and_the_display() {
        let dir = std::env::temp_dir().join(format!("phonto-metal-inputs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("image.glsl"),
            "void mainImage(out vec4 c, in vec2 p) {
                c = vec4(iInput.rain, iInput.wind.x, iDisplays[0].z / 8.0 * float(iDisplayCount), 1.0);
            }",
        )
        .unwrap();
        std::fs::write(
            dir.join(procedural::MANIFEST),
            "[image]\nshader = \"image.glsl\"\n[inputs]\nrain = 0.5\nwind = [0.0, 0.0]\n",
        )
        .unwrap();
        let out = dir.join("frame.png");
        dump(DumpRequest {
            path: dir.to_string_lossy().into_owned(),
            at: 0.0,
            out: out.clone(),
            size: Some((4, 2)),
            location: None,
            inputs: vec![crate::config::InputCommand {
                name: "wind".into(),
                command: "echo 0.25 1".into(),
                every: 1.0,
            }],
        })
        .unwrap();
        let pixel = image::open(&out).unwrap().to_rgba8().get_pixel(0, 0).0;
        std::fs::remove_dir_all(&dir).unwrap();
        let expected = [128, 64, 128, 255];
        assert!(
            pixel.iter().zip(expected).all(|(&p, e)| p.abs_diff(e) <= 1),
            "{pixel:?}"
        );
    }
}
