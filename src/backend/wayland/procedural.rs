//! Draws procedural wallpapers with OpenGL ES 3.0.
//!
//! A [`Program`] is a pack, compiled once for the shared GL context. Each
//! output gets its own [`Target`], because outputs have different sizes and
//! buffers keep state between frames.

use anyhow::{Context, anyhow, bail};
use glow::HasContext;

use super::gl_renderer::{GlRenderer, TextureRef};
use crate::procedural::{
    self, BufferFormat, BufferSize, Filter, FrameInputs, Input, MAX_CHANNELS, Pack, Pass, PassKind,
    Texture, Wrap,
};

const VERTEX_SHADER: &str = "#version 300 es
layout(location = 0) in vec2 a_pos;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
";

pub struct Program {
    /// The buffer passes in the order that they draw, then the image pass.
    passes: Vec<CompiledPass>,
    buffers: Vec<BufferTarget>,
    textures: Vec<UploadedTexture>,
    samplers: Vec<((Filter, Wrap), glow::Sampler)>,
    render_scale: f32,
}

/// How to make the render targets of one buffer pass.
struct BufferTarget {
    size: BufferSize,
    /// The GL internal format that the GPU can draw to.
    format: u32,
    /// If true, the buffer draws only on the first frame of a target.
    once: bool,
}

struct CompiledPass {
    program: glow::Program,
    uniforms: Uniforms,
    channels: Vec<BoundChannel>,
}

struct BoundChannel {
    input: Input,
    sampler: glow::Sampler,
}

/// The locations of the uniforms that phonto sets. A location is `None` if
/// the shader does not use the uniform.
struct Uniforms {
    resolution: Option<glow::UniformLocation>,
    image_resolution: Option<glow::UniformLocation>,
    time: Option<glow::UniformLocation>,
    time_delta: Option<glow::UniformLocation>,
    frame_rate: Option<glow::UniformLocation>,
    frame: Option<glow::UniformLocation>,
    mouse: Option<glow::UniformLocation>,
    date: Option<glow::UniformLocation>,
    sample_rate: Option<glow::UniformLocation>,
    channel_time: Option<glow::UniformLocation>,
    channel_resolution: Option<glow::UniformLocation>,
    pointer: Option<glow::UniformLocation>,
}

struct UploadedTexture {
    texture: glow::Texture,
    width: u32,
    height: u32,
}

/// The render targets and the frame count of one output.
pub struct Target {
    dims: (u32, u32),
    buffers: Vec<PingPong>,
    image: Attachment,
    frame: i32,
}

/// The output of a buffer pass. The pass draws into one slot. The passes read
/// the other slot, which holds the last finished frame.
struct PingPong {
    slots: [Attachment; 2],
    current: usize,
    dims: (u32, u32),
}

struct Attachment {
    texture: glow::Texture,
    fbo: glow::Framebuffer,
}

impl Program {
    /// Compiles `pack` for the current GL context.
    pub fn compile(gl: &glow::Context, pack: &Pack) -> anyhow::Result<Self> {
        let buffers = pack
            .buffers
            .iter()
            .map(|b| BufferTarget {
                size: b.size,
                format: renderable_format(gl, b.format, &b.pass.name),
                once: b.once,
            })
            .collect();
        let mut program = Self {
            passes: Vec::with_capacity(pack.buffers.len() + 1),
            buffers,
            textures: Vec::with_capacity(pack.textures.len()),
            samplers: Vec::new(),
            render_scale: pack.render_scale,
        };
        if let Err(e) = program.build(gl, pack) {
            program.destroy(gl);
            return Err(e).with_context(|| format!("procedural wallpaper `{}`", pack.name));
        }
        Ok(program)
    }

    fn build(&mut self, gl: &glow::Context, pack: &Pack) -> anyhow::Result<()> {
        let max_size = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.max(0) as u32;
        for t in &pack.textures {
            let uploaded = upload_texture(gl, t, max_size)?;
            self.textures.push(uploaded);
        }
        for buffer in &pack.buffers {
            let compiled = self.compile_pass(gl, pack, &buffer.pass, PassKind::Buffer)?;
            self.passes.push(compiled);
        }
        let image = self.compile_pass(gl, pack, &pack.image, PassKind::Image)?;
        self.passes.push(image);
        Ok(())
    }

    fn compile_pass(
        &mut self,
        gl: &glow::Context,
        pack: &Pack,
        pass: &Pass,
        kind: PassKind,
    ) -> anyhow::Result<CompiledPass> {
        let common = pack.common.as_ref();
        let source =
            procedural::fragment_shader(common.map(|c| c.code.as_str()), &pass.source.code, kind);
        let program = link(gl, &source).with_context(|| {
            format!(
                "compile pass `{}` ({})",
                pass.name,
                procedural::source_ids_help(
                    &pass.source.path.display().to_string(),
                    common.map(|c| c.path.display().to_string()).as_deref(),
                )
            )
        })?;

        let uniforms = unsafe {
            gl.use_program(Some(program));
            for unit in 0..MAX_CHANNELS {
                if let Some(loc) = gl.get_uniform_location(program, &format!("iChannel{unit}")) {
                    gl.uniform_1_i32(Some(&loc), unit as i32);
                }
            }
            let loc = |name: &str| gl.get_uniform_location(program, name);
            Uniforms {
                resolution: loc("iResolution"),
                image_resolution: loc("iImageResolution"),
                time: loc("iTime"),
                time_delta: loc("iTimeDelta"),
                frame_rate: loc("iFrameRate"),
                frame: loc("iFrame"),
                mouse: loc("iMouse"),
                date: loc("iDate"),
                sample_rate: loc("iSampleRate"),
                channel_time: loc("iChannelTime"),
                channel_resolution: loc("iChannelResolution"),
                pointer: loc("iPointer"),
            }
        };

        let float_linear = gl
            .supported_extensions()
            .contains("GL_OES_texture_float_linear");
        let mut channels = Vec::with_capacity(pass.channels.len());
        for c in &pass.channels {
            let mut filter = c.filter;
            if let Input::Buffer(i) = c.input
                && self.buffers[i].format == glow::RGBA32F
                && filter != Filter::Nearest
                && !float_linear
            {
                log::warn!(
                    "pass `{}`: the GPU cannot filter 32-bit float buffers. Buffer `{}` uses nearest filtering.",
                    pass.name,
                    pack.buffers[i].pass.name
                );
                filter = Filter::Nearest;
            }
            let sampler = match self.sampler(gl, filter, c.wrap) {
                Ok(s) => s,
                Err(e) => {
                    unsafe { gl.delete_program(program) };
                    return Err(e);
                }
            };
            channels.push(BoundChannel {
                input: c.input,
                sampler,
            });
        }

        Ok(CompiledPass {
            program,
            uniforms,
            channels,
        })
    }

    /// The sampler object for `filter` and `wrap`, made at its first use.
    fn sampler(
        &mut self,
        gl: &glow::Context,
        filter: Filter,
        wrap: Wrap,
    ) -> anyhow::Result<glow::Sampler> {
        if let Some((_, s)) = self.samplers.iter().find(|(k, _)| *k == (filter, wrap)) {
            return Ok(*s);
        }
        let (min, mag) = match filter {
            Filter::Nearest => (glow::NEAREST, glow::NEAREST),
            Filter::Linear => (glow::LINEAR, glow::LINEAR),
            Filter::Mipmap => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
        };
        let wrap_mode = match wrap {
            Wrap::Clamp => glow::CLAMP_TO_EDGE,
            Wrap::Repeat => glow::REPEAT,
            Wrap::Mirror => glow::MIRRORED_REPEAT,
        };
        let sampler = unsafe {
            let s = gl.create_sampler().map_err(|e| anyhow!(e))?;
            gl.sampler_parameter_i32(s, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.sampler_parameter_i32(s, glow::TEXTURE_MAG_FILTER, mag as i32);
            gl.sampler_parameter_i32(s, glow::TEXTURE_WRAP_S, wrap_mode as i32);
            gl.sampler_parameter_i32(s, glow::TEXTURE_WRAP_T, wrap_mode as i32);
            s
        };
        self.samplers.push(((filter, wrap), sampler));
        Ok(sampler)
    }

    /// The size of the render target for an output of `surface` pixels.
    pub fn target_dims(&self, surface: (u32, u32)) -> (u32, u32) {
        let scale = |v: u32| ((v as f32 * self.render_scale).round() as u32).max(1);
        (scale(surface.0), scale(surface.1))
    }

    /// Makes the render targets for an image of `dims` pixels, cleared to
    /// zero.
    pub fn create_target(&self, gl: &glow::Context, dims: (u32, u32)) -> anyhow::Result<Target> {
        let mut created: Vec<Attachment> = Vec::with_capacity(self.buffers.len() * 2 + 1);
        let result = (|| {
            for b in &self.buffers {
                let buffer_dims = b.size.dims(dims);
                for _ in 0..2 {
                    created.push(Attachment::new(gl, b.format, buffer_dims)?);
                }
            }
            created.push(Attachment::new(gl, glow::RGBA8, dims)?);
            anyhow::Ok(())
        })();
        if let Err(e) = result {
            for a in created {
                a.destroy(gl);
            }
            return Err(e);
        }

        let image = created.pop().expect("image attachment was created");
        let mut slots = created.into_iter();
        let buffers = self
            .buffers
            .iter()
            .map(|b| PingPong {
                slots: [
                    slots.next().expect("buffer slot"),
                    slots.next().expect("buffer slot"),
                ],
                current: 0,
                dims: b.size.dims(dims),
            })
            .collect();
        Ok(Target {
            dims,
            buffers,
            image,
            frame: 0,
        })
    }

    /// Draws every pass for one frame and returns the output of the image
    /// pass. The context must be current.
    pub fn render(
        &self,
        gl: &glow::Context,
        target: &mut Target,
        inputs: &FrameInputs,
    ) -> TextureRef {
        let (width, height) = target.dims;
        let buffer_count = self.buffers.len();
        unsafe {
            for i in 0..buffer_count {
                // A `once` buffer keeps what it drew on the first frame.
                // `current` still points to that slot.
                if self.buffers[i].once && target.frame > 0 {
                    continue;
                }
                let pp = &target.buffers[i];
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(pp.slots[1 - pp.current].fbo));
                self.draw_pass(gl, &self.passes[i], pp.dims, target, inputs);
                target.buffers[i].current ^= 1;
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.image.fbo));
            self.draw_pass(gl, &self.passes[buffer_count], target.dims, target, inputs);

            for unit in 0..MAX_CHANNELS as u32 {
                gl.bind_sampler(unit, None);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        target.frame = target.frame.wrapping_add(1);

        TextureRef {
            texture: target.image.texture,
            width,
            height,
            flip_y: true,
        }
    }

    /// Draws one pass into the bound framebuffer of `(width, height)` pixels.
    unsafe fn draw_pass(
        &self,
        gl: &glow::Context,
        pass: &CompiledPass,
        (width, height): (u32, u32),
        target: &Target,
        inputs: &FrameInputs,
    ) {
        let u = &pass.uniforms;
        let (image_w, image_h) = target.dims;
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.use_program(Some(pass.program));
            gl.uniform_3_f32(u.resolution.as_ref(), width as f32, height as f32, 1.0);
            gl.uniform_3_f32(
                u.image_resolution.as_ref(),
                image_w as f32,
                image_h as f32,
                1.0,
            );
            gl.uniform_1_f32(u.time.as_ref(), inputs.time);
            gl.uniform_1_f32(u.time_delta.as_ref(), inputs.time_delta);
            gl.uniform_1_f32(u.frame_rate.as_ref(), inputs.frame_rate);
            gl.uniform_1_i32(u.frame.as_ref(), target.frame);
            let [x, y, z, w] = inputs.mouse;
            gl.uniform_4_f32(u.mouse.as_ref(), x, y, z, w);
            let [x, y, z, w] = inputs.date;
            gl.uniform_4_f32(u.date.as_ref(), x, y, z, w);
            let [x, y, z, w] = inputs.pointer;
            gl.uniform_4_f32(u.pointer.as_ref(), x, y, z, w);
            gl.uniform_1_f32(u.sample_rate.as_ref(), 44_100.0);
            gl.uniform_1_f32_slice(u.channel_time.as_ref(), &[inputs.time; MAX_CHANNELS]);

            let mut resolutions = [0.0f32; MAX_CHANNELS * 3];
            for unit in 0..MAX_CHANNELS {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                let Some(channel) = pass.channels.get(unit) else {
                    // A channel without an input samples as black.
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    gl.bind_sampler(unit as u32, None);
                    continue;
                };
                let (texture, w, h) = match channel.input {
                    Input::Buffer(i) => {
                        let pp = &target.buffers[i];
                        (pp.slots[pp.current].texture, pp.dims.0, pp.dims.1)
                    }
                    Input::Texture(i) => {
                        let t = &self.textures[i];
                        (t.texture, t.width, t.height)
                    }
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.bind_sampler(unit as u32, Some(channel.sampler));
                resolutions[unit * 3..unit * 3 + 3].copy_from_slice(&[w as f32, h as f32, 1.0]);
            }
            gl.uniform_3_f32_slice(u.channel_resolution.as_ref(), &resolutions);

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    /// Frees the GL objects. The context must be current.
    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            for pass in self.passes {
                gl.delete_program(pass.program);
            }
            for t in self.textures {
                gl.delete_texture(t.texture);
            }
            for (_, s) in self.samplers {
                gl.delete_sampler(s);
            }
        }
    }
}

impl Target {
    pub fn dims(&self) -> (u32, u32) {
        self.dims
    }

    /// Frees the GL objects. The context must be current.
    pub fn destroy(self, gl: &glow::Context) {
        for pp in self.buffers {
            for a in pp.slots {
                a.destroy(gl);
            }
        }
        self.image.destroy(gl);
    }
}

impl Attachment {
    fn new(gl: &glow::Context, format: u32, (width, height): (u32, u32)) -> anyhow::Result<Self> {
        unsafe {
            let texture = gl.create_texture().map_err(|e| anyhow!(e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, format, width as i32, height as i32);
            // The present shader samples the image without a sampler object.
            set_texture_params(gl, glow::LINEAR);
            gl.bind_texture(glow::TEXTURE_2D, None);

            let fbo = match gl.create_framebuffer() {
                Ok(f) => f,
                Err(e) => {
                    gl.delete_texture(texture);
                    bail!(e);
                }
            };
            let attachment = Self { texture, fbo };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                attachment.destroy(gl);
                bail!("{width}x{height} render target is incomplete (status {status:#x})");
            }
            gl.clear_buffer_f32_slice(glow::COLOR, 0, &[0.0; 4]);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Ok(attachment)
        }
    }

    fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.texture);
        }
    }
}

/// Compiles the fragment shader of a pass and links it with the vertex
/// shader of the fullscreen quad.
fn link(gl: &glow::Context, fragment_source: &str) -> anyhow::Result<glow::Program> {
    let vertex = GlRenderer::compile_shader(gl, glow::VERTEX_SHADER, VERTEX_SHADER)?;
    let fragment = match GlRenderer::compile_shader(gl, glow::FRAGMENT_SHADER, fragment_source) {
        Ok(f) => f,
        Err(e) => {
            unsafe { gl.delete_shader(vertex) };
            return Err(e);
        }
    };
    GlRenderer::link_program(gl, vertex, fragment)
}

fn upload_texture(
    gl: &glow::Context,
    t: &Texture,
    max_size: u32,
) -> anyhow::Result<UploadedTexture> {
    if t.width > max_size || t.height > max_size {
        bail!(
            "texture {} is {}x{}, larger than the {max_size}x{max_size} limit of this GPU",
            t.path.display(),
            t.width,
            t.height
        );
    }
    unsafe {
        let texture = gl.create_texture().map_err(|e| anyhow!(e))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            t.width as i32,
            t.height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&t.rgba)),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        set_texture_params(gl, glow::LINEAR_MIPMAP_LINEAR);
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(UploadedTexture {
            texture,
            width: t.width,
            height: t.height,
        })
    }
}

/// The filtering and wrapping of the bound texture, for when no sampler
/// object replaces them.
unsafe fn set_texture_params(gl: &glow::Context, min_filter: u32) {
    unsafe {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            min_filter as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }
}

/// The GL internal format for a buffer that requested `requested`. If the
/// GPU cannot draw to it, this is a smaller format.
fn renderable_format(gl: &glow::Context, requested: BufferFormat, name: &str) -> u32 {
    let extensions = gl.supported_extensions();
    let float32 = extensions.contains("GL_EXT_color_buffer_float");
    let float16 = float32 || extensions.contains("GL_EXT_color_buffer_half_float");
    match requested {
        BufferFormat::Rgba8 => glow::RGBA8,
        BufferFormat::Rgba32f if float32 => glow::RGBA32F,
        BufferFormat::Rgba16f if float16 => glow::RGBA16F,
        BufferFormat::Rgba32f if float16 => {
            log::warn!(
                "buffer `{name}`: the GPU cannot draw to 32-bit floats. The buffer uses 16-bit floats."
            );
            glow::RGBA16F
        }
        BufferFormat::Rgba32f | BufferFormat::Rgba16f => {
            log::warn!(
                "buffer `{name}`: the GPU cannot draw to float textures. The buffer uses 8-bit RGBA, so values are clamped to 0..1."
            );
            glow::RGBA8
        }
    }
}
