//! Procedural wallpapers: GLSL shaders that phonto draws every frame,
//! instead of a video.
//!
//! A wallpaper is a single `.glsl` file, or a directory with a
//! `manifest.toml` (a "pack"). The shaders use the conventions of Shadertoy:
//! `mainImage`, `iTime`, `iMouse`, `iChannel0`.
//!
//! This module does not depend on the platform. It loads packs, builds the
//! shader source, and keeps the clock and the pointer state. Each backend
//! does the drawing.

// Only the Wayland backend draws procedural wallpapers at this time. The
// macOS backend only uses `is_procedural`, to reject them.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod glsl;
mod inputs;
mod pack;

use std::path::Path;

pub use glsl::{PassKind, fragment_shader, source_ids_help};
pub use inputs::{Clock, FrameInputs, FrameLimiter, Pointer, local_date};
pub use pack::{
    BufferFormat, BufferSize, Filter, Input, MANIFEST, MAX_CHANNELS, Pack, Pass, Texture, Wrap,
};

/// True if the backend of this platform can draw procedural wallpapers.
pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// True if `path` is a `.glsl` file, a pack directory, or the
/// `manifest.toml` of a pack.
pub fn is_procedural(path: &str) -> bool {
    if crate::config::is_url(path) {
        return false;
    }
    let path = Path::new(path);
    if path.is_dir() {
        return pack::is_pack_dir(path);
    }
    if path.file_name().is_some_and(|n| n == MANIFEST) {
        return true;
    }
    has_glsl_extension(path)
}

/// True if `path` is a `.glsl` file with a `mainImage` function. `--rand`
/// uses this test, so it does not pick a post-processing shader.
pub fn is_procedural_shader_file(path: &Path) -> bool {
    has_glsl_extension(path)
        && std::fs::read_to_string(path).is_ok_and(|code| pack::defines_main_image(&code))
}

/// True if `path` is a directory that contains a `manifest.toml`.
pub fn is_pack_dir(path: &Path) -> bool {
    pack::is_pack_dir(path)
}

fn has_glsl_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("glsl"))
}
