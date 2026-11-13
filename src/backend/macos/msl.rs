//! Translates the GLSL of a procedural pass to the Metal Shading Language.
//!
//! glslang compiles the GLSL to SPIR-V, and SPIRV-Cross writes MSL. The GLSL
//! is the source that `procedural::fragment_shader` builds for every backend.
//! Nothing here changes the code of the user. Only the names of its functions
//! and structs change in the MSL.

use std::collections::HashSet;

use anyhow::{Context, anyhow};
use glslang::error::GlslangError;
use spirv_cross2::compile::msl::{AutomaticResourceBindingTier, CompilerOptions, MslVersion};
use spirv_cross2::reflect::{ResourceType, TypeInner};
use spirv_cross2::spirv::ExecutionModel;
use spirv_cross2::targets::Msl;
use spirv_cross2::{Compiler, Module};

use crate::procedural::MAX_CHANNELS;

/// The block that glslang puts the loose uniforms in. Vulkan GLSL has no
/// loose uniforms.
const UNIFORM_BLOCK: &str = "PhontoUniforms";

/// Goes before the name of every function and struct of a shader in MSL.
/// Metal declares names that GLSL does not have, such as the functions
/// `atan2`, `log10` and `select`, and the reserved types `quad` and `complex`.
/// A shader function or struct with such a name does not compile, or its
/// calls are ambiguous. SPIRV-Cross renames only some of them. No Metal name
/// starts with this prefix. Every name gets it. Two names then stay
/// different.
const NAME_PREFIX: &str = "phonto_";

// SPIR-V opcodes.
const OP_NAME: u32 = 5;
const OP_ENTRY_POINT: u32 = 15;
const OP_TYPE_STRUCT: u32 = 30;
const OP_FUNCTION: u32 = 54;

/// The words of the SPIR-V header, before the first instruction.
const SPIRV_HEADER: usize = 5;

/// A pass in MSL, and where its inputs go.
pub struct Translated {
    pub source: String,
    /// The name of the fragment function. SPIRV-Cross renames `main`.
    pub entry_point: String,
    /// The buffer index of the uniform block. `None` if the pass reads no
    /// uniform.
    pub uniform_buffer: Option<usize>,
    /// The size of the uniform block in bytes.
    pub uniform_size: usize,
    pub uniforms: Vec<Member>,
    /// The texture and sampler indices of `iChannelN`. `None` if the pass
    /// does not read the channel.
    pub channels: [Option<(usize, usize)>; MAX_CHANNELS],
}

/// A member of the uniform block.
pub struct Member {
    pub name: String,
    pub offset: usize,
    /// The distance between the elements of an array, else 0.
    pub stride: usize,
}

pub fn translate(glsl: &str) -> anyhow::Result<Translated> {
    let spirv = compile_spirv(glsl)?;
    to_msl(&spirv)
}

fn compile_spirv(glsl: &str) -> anyhow::Result<Vec<u32>> {
    use glslang::{
        CompilerOptions, Shader, ShaderInput, ShaderMessage, ShaderOptions, ShaderSource,
        ShaderStage, SourceLanguage, SpirvVersion, Target, VulkanVersion,
    };

    // glslang writes SPIR-V only for GLSL ES 3.10 and later. 3.10 extends
    // 3.00. It only reserves a few more words, such as `buffer` and `shared`.
    let glsl = glsl.replacen("#version 300 es", "#version 310 es", 1);
    let compiler = glslang::Compiler::acquire().context("glslang failed to start")?;
    let source = ShaderSource::from(glsl);
    let options = CompilerOptions {
        source_language: SourceLanguage::GLSL,
        target: Target::Vulkan {
            version: VulkanVersion::Vulkan1_1,
            spirv_version: SpirvVersion::SPIRV1_3,
        },
        version_profile: None,
        messages: ShaderMessage::DEFAULT,
    };
    let input = ShaderInput::new(
        &source,
        ShaderStage::Fragment,
        &options,
        None::<&[(&str, Option<&str>)]>,
        None,
    )
    .map_err(glslang_error)?;
    let mut shader = Shader::new(compiler, input);
    shader.options(
        ShaderOptions::AUTO_MAP_BINDINGS
            | ShaderOptions::AUTO_MAP_LOCATIONS
            | ShaderOptions::VULKAN_RULES_RELAXED,
    );
    shader.default_uniform_block_set_and_binding(0, 0);
    shader.default_uniform_block_name(UNIFORM_BLOCK);
    let shader = shader.parse().map_err(glslang_error)?;
    shader.compile().map_err(glslang_error)
}

/// The compiler log, without the empty debug log that glslang adds.
fn glslang_error(e: GlslangError) -> anyhow::Error {
    match e {
        GlslangError::PreprocessError(log)
        | GlslangError::ParseError(log)
        | GlslangError::MapIoError(log)
        | GlslangError::LinkError(log) => anyhow!("{}", log.log.trim()),
        e => anyhow!(e),
    }
}

fn to_msl(spirv: &[u32]) -> anyhow::Result<Translated> {
    let spirv = prefix_names(spirv);
    let compiler = Compiler::<Msl>::new(Module::from_words(&spirv)).map_err(spirv_cross_error)?;
    let resources = compiler.shader_resources().map_err(spirv_cross_error)?;

    let mut block = None;
    let mut uniform_size = 0;
    let mut uniforms = Vec::new();
    for r in resources
        .resources_for_type(ResourceType::UniformBuffer)
        .map_err(spirv_cross_error)?
    {
        let ty = compiler
            .type_description(r.base_type_id)
            .map_err(spirv_cross_error)?;
        if let TypeInner::Struct(s) = ty.inner {
            uniform_size = s.size;
            for m in s.members {
                let name = m.name.map(|n| n.to_string()).unwrap_or_default();
                let offset = m.offset as usize;
                let member_type = compiler.type_description(m.id).map_err(spirv_cross_error)?;
                // The inputs of a pack are the fields of the struct `iInput`.
                // Each field is a member named `iInput.<field>`, as in GL.
                if let TypeInner::Struct(fields) = member_type.inner {
                    uniforms.extend(fields.members.into_iter().map(|f| Member {
                        name: format!(
                            "{name}.{}",
                            f.name.map(|n| n.to_string()).unwrap_or_default()
                        ),
                        offset: offset + f.offset as usize,
                        stride: f.array_stride.unwrap_or(0) as usize,
                    }));
                    continue;
                }
                uniforms.push(Member {
                    name,
                    offset,
                    stride: m.array_stride.unwrap_or(0) as usize,
                });
            }
        }
        block = Some(r.id);
    }

    let channels: Vec<_> = resources
        .resources_for_type(ResourceType::SampledImage)
        .map_err(spirv_cross_error)?
        .filter_map(|r| Some((channel_index(&r.name)?, r.id)))
        .collect();

    let mut options = CompilerOptions::default();
    options.version = MslVersion::new(2, 4, 0);
    let msl = compiler.compile(&options).map_err(spirv_cross_error)?;

    let entry_point = msl
        .cleansed_entry_point_name("main", ExecutionModel::Fragment)
        .map_err(spirv_cross_error)?
        .context("the shader has no fragment entry point")?
        .to_string();

    // SPIRV-Cross numbers the resources that the shader reads.
    let binding = |id, tier| {
        msl.automatic_resource_binding(id, tier)
            .map(|i| i.map(|i| i as usize))
            .map_err(spirv_cross_error)
    };
    let uniform_buffer = match block {
        Some(id) => binding(id, AutomaticResourceBindingTier::Primary)?,
        None => None,
    };
    let mut slots = [None; MAX_CHANNELS];
    for (n, id) in channels {
        let texture = binding(id, AutomaticResourceBindingTier::Primary)?;
        let sampler = binding(id, AutomaticResourceBindingTier::Secondary)?;
        if let (Some(texture), Some(sampler)) = (texture, sampler) {
            slots[n] = Some((texture, sampler));
        }
    }

    Ok(Translated {
        source: msl.to_string(),
        entry_point,
        uniform_buffer,
        uniform_size,
        uniforms,
        channels: slots,
    })
}

/// Adds [`NAME_PREFIX`] to the debug name of every function and struct,
/// except the entry point. SPIRV-Cross writes MSL with these names.
fn prefix_names(spirv: &[u32]) -> Vec<u32> {
    let Some(instructions) = instructions(spirv) else {
        // glslang writes valid SPIR-V. If it did not, SPIRV-Cross reports it.
        return spirv.to_vec();
    };
    let mut renamed = HashSet::new();
    let mut entry_points = HashSet::new();
    for inst in &instructions {
        match (inst[0] & 0xffff, inst.get(1), inst.get(2)) {
            (OP_TYPE_STRUCT, Some(&id), _) | (OP_FUNCTION, _, Some(&id)) => {
                renamed.insert(id);
            }
            (OP_ENTRY_POINT, _, Some(&id)) => {
                entry_points.insert(id);
            }
            _ => {}
        }
    }

    let mut out = spirv[..SPIRV_HEADER].to_vec();
    for inst in instructions {
        match inst.get(1) {
            Some(id)
                if inst[0] & 0xffff == OP_NAME
                    && renamed.contains(id)
                    && !entry_points.contains(id) =>
            {
                let name = encode_string(&format!("{NAME_PREFIX}{}", decode_string(&inst[2..])));
                out.push(((2 + name.len() as u32) << 16) | OP_NAME);
                out.push(*id);
                out.extend(name);
            }
            _ => out.extend_from_slice(inst),
        }
    }
    out
}

/// The instructions after the header, or `None` if the SPIR-V is malformed.
/// The first word of an instruction holds its length in words and its
/// opcode.
fn instructions(spirv: &[u32]) -> Option<Vec<&[u32]>> {
    let mut rest = spirv.get(SPIRV_HEADER..)?;
    let mut out = Vec::new();
    while let Some(&first) = rest.first() {
        let len = (first >> 16) as usize;
        if len == 0 || len > rest.len() {
            return None;
        }
        let (inst, tail) = rest.split_at(len);
        out.push(inst);
        rest = tail;
    }
    Some(out)
}

/// A SPIR-V string: UTF-8 in words, low byte first, ended by a zero byte.
fn decode_string(words: &[u32]) -> String {
    let bytes: Vec<u8> = words
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn encode_string(s: &str) -> Vec<u32> {
    let mut bytes = s.as_bytes().to_vec();
    // At least one zero byte ends the string. More fill the last word.
    bytes.resize((bytes.len() + 1).next_multiple_of(4), 0);
    let (words, _) = bytes.as_chunks::<4>();
    words.iter().map(|w| u32::from_le_bytes(*w)).collect()
}

fn spirv_cross_error(e: spirv_cross2::SpirvCrossError) -> anyhow::Error {
    anyhow!("SPIRV-Cross: {e}")
}

/// `n` for `iChannel{n}`.
fn channel_index(name: &str) -> Option<usize> {
    name.strip_prefix("iChannel")?
        .parse()
        .ok()
        .filter(|&n| n < MAX_CHANNELS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedural::{PassKind, fragment_shader};

    #[test]
    fn translates_a_pass_and_maps_its_inputs() {
        let glsl = fragment_shader(
            None,
            "void mainImage(out vec4 c, in vec2 p) {
                c = texture(iChannel2, p / iResolution.xy) + vec4(iTime) + iMouse;
            }",
            PassKind::Image,
            &[],
        );
        let t = translate(&glsl).unwrap();
        assert!(
            t.source
                .contains(&format!("fragment main0_out {}(", t.entry_point))
        );
        assert!(t.uniform_buffer.is_some());
        let offset = |name: &str| t.uniforms.iter().find(|m| m.name == name).map(|m| m.offset);
        assert!(offset("iTime").is_some());
        assert!(offset("iMouse").is_some());
        let channel_time = t
            .uniforms
            .iter()
            .find(|m| m.name == "iChannelTime")
            .unwrap();
        assert_eq!(channel_time.stride, 16, "std140 pads a float array to vec4");
        assert!(t.channels[2].is_some());
        assert!(t.channels[0].is_none(), "unused channels have no slot");
    }

    #[test]
    fn errors_name_the_line_of_the_pass() {
        let glsl = fragment_shader(
            None,
            "void mainImage(out vec4 c, in vec2 p) {\n    c = nope;\n}",
            PassKind::Image,
            &[],
        );
        let err = format!("{:#}", translate(&glsl).err().unwrap());
        assert!(err.contains("0:2"), "{err}");
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn prefixes_functions_and_structs_but_not_the_entry_point() {
        let glsl = fragment_shader(
            None,
            "struct complex { vec2 v; };
            float quad(vec2 a) { return a.x; }
            void mainImage(out vec4 c, in vec2 p) { c = vec4(quad(complex(p).v)); }",
            PassKind::Image,
            &[],
        );
        let t = translate(&glsl).unwrap();
        assert!(t.source.contains("phonto_quad("), "{}", t.source);
        assert!(t.source.contains("struct phonto_complex"), "{}", t.source);
        assert!(t.source.contains("phonto_mainImage("), "{}", t.source);
        assert!(!t.entry_point.starts_with(NAME_PREFIX));
    }

    #[test]
    fn struct_fields_are_members_of_their_own() {
        let glsl = "#version 300 es
            precision highp float;
            struct phonto_Inputs { float rain; vec2 wind; };
            uniform phonto_Inputs iInput;
            out vec4 o;
            void main() { o = vec4(iInput.rain, iInput.wind, 1.0); }";
        let t = translate(glsl).unwrap();
        let offset = |name: &str| t.uniforms.iter().find(|m| m.name == name).map(|m| m.offset);
        let rain = offset("iInput.rain").unwrap();
        assert_eq!(
            offset("iInput.wind"),
            Some(rain + 8),
            "std140 aligns a vec2 to 8"
        );
        assert_eq!(offset("iInput"), None);
    }

    #[test]
    fn strings_round_trip() {
        for s in ["", "abc", "abcd", "quad(vf2;"] {
            let words = encode_string(s);
            assert_eq!(decode_string(&words), s);
            assert!(words.last().unwrap().to_le_bytes().contains(&0));
        }
    }

    #[test]
    fn channel_names() {
        assert_eq!(channel_index("iChannel0"), Some(0));
        assert_eq!(channel_index("iChannel7"), Some(7));
        assert_eq!(channel_index("iChannel8"), None);
        assert_eq!(channel_index("iChannelTime"), None);
    }
}
