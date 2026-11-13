//! Procedural wallpapers: GLSL shaders that phonto draws every frame,
//! instead of a video.
//!
//! A wallpaper is a single `.glsl` file, or a directory with a
//! `manifest.toml` (a "pack"). The shaders use the conventions of Shadertoy:
//! `mainImage`, `iTime`, `iMouse`, `iChannel0`.
//!
//! This module does not depend on the platform. It loads packs, builds the
//! shader source, keeps the clock and the pointer state, turns audio samples
//! into levels, runs the input commands, and watches image files. Each
//! backend does the drawing and reads the system.

// Only the Wayland backend draws procedural wallpapers at this time. The
// macOS backend only uses `is_procedural`, to reject them.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod audio;
mod commands;
mod glsl;
mod inputs;
mod pack;
mod watch;

use std::path::Path;

pub use audio::{Analyzer, AudioFrame, SAMPLE_RATE};
pub use commands::{CommandInputs, read_once as read_inputs_once};
pub use glsl::{PassKind, fragment_shader, source_ids_help};
pub use inputs::{Clock, FrameInputs, FrameLimiter, MAX_DISPLAYS, Pointer, local_date, location};
pub use pack::{
    BufferFormat, BufferSize, Filter, Input, MANIFEST, MAX_CHANNELS, Pack, Pass, Texture, Wrap,
};
pub use watch::TextureWatcher;

/// True if the backend of this platform can draw procedural wallpapers.
pub const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

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
