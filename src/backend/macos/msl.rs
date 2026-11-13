//! Translates the GLSL of a procedural pass to the Metal Shading Language.
//!
//! glslang compiles the GLSL to SPIR-V, and SPIRV-Cross writes MSL. The GLSL
//! is the source that `procedural::fragment_shader` builds for every backend.
//! Nothing here changes the code of the user. Only some of its names change:
//! those of its functions and structs, and those that GLSL ES 3.10 reserves.

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

/// Names that GLSL ES 3.00 allows, and that glslang reserves or declares as
/// built-in functions in GLSL ES 3.10 for Vulkan. 3.10 adds `buffer`,
/// `shared`, `precise`, the `dmat` types, more sampler and image types, and
/// functions such as `bitCount` and `packUnorm4x8`. Vulkan and the extensions
/// that glslang declares add more types, qualifiers and functions.
const RESERVED_IN_310: &str = "
    assumeEXT atomicAdd atomicAnd atomicCompSwap atomicCounter atomicCounterDecrement
    atomicCounterIncrement atomicExchange atomicLoad atomicMax atomicMin atomicStore atomicXor
    attachmentEXT bfloat16BitsToIntEXT bfloat16BitsToUintEXT bitcastExtractfe2m1EXT
    bitcastExtractfe2m3EXT bitcastExtractfe3m2EXT bitCount bitfieldExtract bitfieldInsert
    bitfieldReverse buffer colorAttachmentReadEXT depthAttachmentReadEXT devicecoherent dmat2
    dmat2x2 dmat2x3 dmat2x4 dmat3 dmat3x2 dmat3x3 dmat3x4 dmat4 dmat4x2 dmat4x3 dmat4x4
    doubleBitsToInt64 doubleBitsToUint64 expectEXT findLSB findMSB float16BitsToInt16
    float16BitsToUint16 floate4m3BitsToIntEXT floate4m3BitsToUintEXT floate5m2BitsToIntEXT
    floate5m2BitsToUintEXT floatmxint8BitsToIntEXT floatmxint8BitsToUintEXT
    floatue8m0BitsToIntEXT floatue8m0BitsToUintEXT fma frexp halfBitsToInt16 halfBitsToUint16
    halhBitsToInt16 iattachmentEXT iimage2DMS iimage2DMSArray iimage2DRect iimageCubeArray
    image2DMS image2DMSArray image2DRect imageAtomicAdd imageAtomicAnd imageAtomicCompSwap
    imageAtomicExchange imageAtomicLoad imageAtomicMax imageAtomicMin imageAtomicOr
    imageAtomicStore imageAtomicXor imageCubeArray imageSize imulExtended int16BitsToFloat16
    int16BitsToHalf int64BitsToDouble intBitsToBFloat16EXT intBitsToFloate4m3EXT
    intBitsToFloate5m2EXT intBitsToFloatmxint8EXT intBitsToFloatue8m0EXT interpolateAtCentroid
    interpolateAtOffset interpolateAtSample isamplerCubeArray isubpassInput isubpassInputMS
    itexture1D itexture1DArray itexture2D itexture2DArray itexture2DMS itexture2DMSArray
    itexture2DRect itexture3D itextureBuffer itextureCube itextureCubeArray ldexp
    memoryBarrierAtomicCounter memoryBarrierBuffer memoryBarrierImage nonprivate pack16 pack32
    pack64 packFloat16xfe2m1EXT packFloat16xfe2m3EXT packFloat16xfe3m2EXT packFloat2x16
    packFloat2xfe2m1EXT packFloat4xfe2m1EXT packFloat4xfe2m3EXT packFloat4xfe3m2EXT
    packFloat8xfe2m1EXT packFloat8xfe2m3EXT packFloat8xfe3m2EXT packInt2x16 packInt2x32
    packInt4x16 packSnorm4x8 packUint2x16 packUint2x32 packUint4x16 packUnorm4x8 precise
    queuefamilycoherent sampler samplerCubeArray samplerCubeArrayShadow samplerShadow
    saturatedConvertEXT shadercallcoherent shared stencilAttachmentReadEXT subgroupAdd
    subgroupAll subgroupAllEqual subgroupAnd subgroupAny subgroupBallot subgroupBallotBitCount
    subgroupBallotBitExtract subgroupBallotExclusiveBitCount subgroupBallotFindLSB
    subgroupBallotFindMSB subgroupBallotInclusiveBitCount subgroupBarrier subgroupBroadcast
    subgroupBroadcastFirst subgroupClusteredAdd subgroupClusteredAnd subgroupClusteredMax
    subgroupClusteredMin subgroupClusteredMul subgroupClusteredOr subgroupClusteredRotate
    subgroupClusteredXor subgroupcoherent subgroupElect subgroupExclusiveAdd
    subgroupExclusiveAnd subgroupExclusiveMax subgroupExclusiveMin subgroupExclusiveMul
    subgroupExclusiveOr subgroupExclusiveXor subgroupInclusiveAdd subgroupInclusiveAnd
    subgroupInclusiveMax subgroupInclusiveMin subgroupInclusiveMul subgroupInclusiveOr
    subgroupInclusiveXor subgroupInverseBallot subgroupMax subgroupMemoryBarrier
    subgroupMemoryBarrierBuffer subgroupMemoryBarrierImage subgroupMin subgroupMul subgroupOr
    subgroupPartitionedAddNV subgroupPartitionedAndNV subgroupPartitionedExclusiveAddNV
    subgroupPartitionedExclusiveAndNV subgroupPartitionedExclusiveMaxNV
    subgroupPartitionedExclusiveMinNV subgroupPartitionedExclusiveMulNV
    subgroupPartitionedExclusiveOrNV subgroupPartitionedExclusiveXorNV
    subgroupPartitionedInclusiveAddNV subgroupPartitionedInclusiveAndNV
    subgroupPartitionedInclusiveMaxNV subgroupPartitionedInclusiveMinNV
    subgroupPartitionedInclusiveMulNV subgroupPartitionedInclusiveOrNV
    subgroupPartitionedInclusiveXorNV subgroupPartitionedMaxNV subgroupPartitionedMinNV
    subgroupPartitionedMulNV subgroupPartitionedOrNV subgroupPartitionedXorNV
    subgroupPartitionNV subgroupQuadAll subgroupQuadAny subgroupQuadBroadcast
    subgroupQuadSwapDiagonal subgroupQuadSwapHorizontal subgroupQuadSwapVertical subgroupRotate
    subgroupShuffle subgroupShuffleDown subgroupShuffleUp subgroupShuffleXor subgroupXor
    subpassInput subpassInputMS texture1D texture1DArray texture2D texture2DArray texture2DMS
    texture2DMSArray texture2DRect texture3D textureBlockMatchGatherSADQCOM
    textureBlockMatchGatherSSDQCOM textureBlockMatchSADQCOM textureBlockMatchSSDQCOM
    textureBlockMatchWindowSADQCOM textureBlockMatchWindowSSDQCOM textureBoxFilterQCOM
    textureBuffer textureCube textureCubeArray textureWeightedQCOM tileImageEXT uaddCarry
    uattachmentEXT uimage2DMS uimage2DMSArray uimage2DRect uimageCubeArray uint16BitsToFloat16
    uint16BitsToHalf uint64BitsToDouble uintBitsToBFloat16EXT uintBitsToFloate4m3EXT
    uintBitsToFloate5m2EXT uintBitsToFloatmxint8EXT uintBitsToFloatue8m0EXT umulExtended
    unpack16 unpack32 unpack8 unpackFloat16xfe2m1EXT unpackFloat16xfe2m3EXT
    unpackFloat16xfe3m2EXT unpackFloat2x16 unpackFloat2xfe2m1EXT unpackFloat4xfe2m1EXT
    unpackFloat4xfe2m3EXT unpackFloat4xfe3m2EXT unpackFloat8xfe2m1EXT unpackFloat8xfe2m3EXT
    unpackFloat8xfe3m2EXT unpackInt2x16 unpackInt2x32 unpackInt4x16 unpackSnorm4x8
    unpackUint2x16 unpackUint2x32 unpackUint4x16 unpackUnorm4x8 usamplerCubeArray usubBorrow
    usubpassInput usubpassInputMS utexture1D utexture1DArray utexture2D utexture2DArray
    utexture2DMS utexture2DMSArray utexture2DRect utexture3D utextureBuffer utextureCube
    utextureCubeArray workgroupcoherent
";

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
    /// The members of the uniform block that the pass reads.
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
    // 3.00, but it reserves more names.
    let glsl = rename_reserved(&glsl.replacen("#version 300 es", "#version 310 es", 1));
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

/// Puts [`NAME_PREFIX`] before each word of [`RESERVED_IN_310`] in the
/// code and in its macros. A `#define` of the word would clash with a
/// shader's own, such as `#define texture2D texture`. The qualifier in
/// `layout(shared)` changes too, and glslang rejects it.
fn rename_reserved(glsl: &str) -> String {
    let reserved: HashSet<&str> = RESERVED_IN_310.split_ascii_whitespace().collect();
    let in_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(glsl.len());
    // Each piece is a name, or nothing, and then one other character.
    for piece in glsl.split_inclusive(|c| !in_name(c)) {
        if reserved.contains(piece.trim_end_matches(|c| !in_name(c))) {
            out.push_str(NAME_PREFIX);
        }
        out.push_str(piece);
    }
    out
}

/// The name in the shader of a name that [`rename_reserved`] changed.
fn original_name(name: &str) -> &str {
    name.strip_prefix(NAME_PREFIX)
        .filter(|n| RESERVED_IN_310.split_ascii_whitespace().any(|w| w == *n))
        .unwrap_or(name)
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
            // The block declares every uniform of the header.
            let read = compiler
                .active_buffer_ranges(r.id)
                .map_err(spirv_cross_error)?;
            for m in s.members {
                if !read.iter().any(|range| range.index as usize == m.index) {
                    continue;
                }
                let name = m.name.map(|n| n.to_string()).unwrap_or_default();
                let offset = m.offset as usize;
                let member_type = compiler.type_description(m.id).map_err(spirv_cross_error)?;
                // The inputs of a pack are the fields of the struct `iInput`.
                // Each field is a member named `iInput.<field>`, as in GL.
                if let TypeInner::Struct(fields) = member_type.inner {
                    uniforms.extend(fields.members.into_iter().map(|f| Member {
                        name: format!(
                            "{name}.{}",
                            original_name(f.name.as_deref().unwrap_or_default())
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
                c = texture(iChannel2, p / iResolution.xy) + vec4(iTime + iChannelTime[1]) + iMouse;
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
        assert!(
            offset("iPointer").is_none(),
            "unread uniforms have no offset"
        );
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

    /// GLSL ES 3.00 allows these names. glslang reserves or declares them in
    /// 3.10 for Vulkan.
    #[test]
    fn translates_names_that_310_reserves() {
        let glsl = "#version 300 es
            precision highp float;
            uniform sampler2D iChannel0;
            struct phonto_Inputs { float buffer; };
            uniform phonto_Inputs iInput;
            out vec4 o;
            #define texture2D texture
            vec4 sampler(vec2 p) { return texture2D(iChannel0, p); }
            float ldexp(float x, float e) { return x * exp2(e); }
            void main() {
                float precise = ldexp(iInput.buffer, 1.0);
                vec4 buffer = sampler(gl_FragCoord.xy) * precise;
                o = buffer;
            }";
        let t = translate(glsl).unwrap();
        assert!(t.uniforms.iter().any(|m| m.name == "iInput.buffer"));
        assert!(t.channels[0].is_some());
    }

    #[test]
    fn renames_whole_names_only() {
        assert_eq!(
            rename_reserved("#define texture2D texture\nvec4 buffer2, buffer;\n// precise"),
            "#define phonto_texture2D texture\nvec4 buffer2, phonto_buffer;\n// phonto_precise"
        );
        assert_eq!(original_name("phonto_buffer"), "buffer");
        assert_eq!(original_name("phonto_rain"), "phonto_rain");
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
