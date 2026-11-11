//! The GLSL ES 3.00 source that phonto compiles for each pass. It contains
//! the phonto declarations, the common code of the pack, the code of the
//! pass, and a `main` function that calls `mainImage`.
//!
//! A backend compiles this source as it is, or translates it.

use std::fmt::Write;

/// The declarations that each pass gets. The names and meanings are the same
/// as on Shadertoy. phonto adds `iImageResolution`, `iPointer`, `iAudio`,
/// `iSpectrum`, `iChroma`, and `iChannel4` to `iChannel7`.
const HEADER: &str = "#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;

uniform vec3 iResolution;           // size of this pass in pixels, z = 1
uniform vec3 iImageResolution;      // size of the image pass, the pixels of iMouse and iPointer
uniform float iTime;                // seconds of playback
uniform float iTimeDelta;           // seconds since the last frame
uniform float iFrameRate;           // frames per second, smoothed
uniform int iFrame;                 // frames drawn since the target was made
uniform vec4 iMouse;                // as on Shadertoy
uniform vec4 iDate;                 // year, month (0-11), day (1-31), seconds since midnight
uniform float iSampleRate;          // always 44100, only so that shaders compile
uniform float iChannelTime[8];      // the same as iTime, only so that shaders compile
uniform vec3 iChannelResolution[8]; // size of each channel in pixels
uniform sampler2D iChannel0;
uniform sampler2D iChannel1;
uniform sampler2D iChannel2;
uniform sampler2D iChannel3;
uniform sampler2D iChannel4;
uniform sampler2D iChannel5;
uniform sampler2D iChannel6;
uniform sampler2D iChannel7;
uniform vec4 iPointer;              // xy = position, z = 1 over the wallpaper, w = 1 while pressed
uniform vec4 iAudio;                // bass, midrange, treble, overall volume, all in 0..1
uniform float iSpectrum[32];        // logarithmic bands from 40 Hz to 11 kHz, in 0..1
uniform float iChroma[12];          // strength of each note name, C to B, in 0..1

out vec4 phonto_FragColor;
";

/// Source-string numbers for `#line`. A compile error then tells which file a
/// line is in. For example, `0:12` is line 12 of the pass file.
const PASS_SOURCE: u32 = 0;
const COMMON_SOURCE: u32 = 1;
const WRAPPER_SOURCE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    /// An offscreen pass. Alpha is kept, so a buffer can store four values.
    Buffer,
    /// The pass on screen. Alpha is always 1, as on Shadertoy.
    Image,
}

pub fn fragment_shader(common: Option<&str>, code: &str, kind: PassKind) -> String {
    let mut s = String::with_capacity(HEADER.len() + common.map_or(0, str::len) + code.len() + 256);
    s.push_str(HEADER);
    if let Some(common) = common {
        let _ = writeln!(s, "#line 1 {COMMON_SOURCE}");
        s.push_str(&strip_version(common));
        s.push('\n');
    }
    let _ = writeln!(s, "#line 1 {PASS_SOURCE}");
    s.push_str(&strip_version(code));
    s.push('\n');
    let _ = writeln!(s, "#line 1 {WRAPPER_SOURCE}");
    s.push_str("void main() {\n");
    s.push_str("    phonto_FragColor = vec4(0.0, 0.0, 0.0, 1.0);\n");
    s.push_str("    mainImage(phonto_FragColor, gl_FragCoord.xy);\n");
    if kind == PassKind::Image {
        s.push_str("    phonto_FragColor.a = 1.0;\n");
    }
    s.push_str("}\n");
    s
}

/// Text that explains the `#line` source numbers in a compile error.
pub fn source_ids_help(pass_file: &str, common_file: Option<&str>) -> String {
    let mut s = format!("{PASS_SOURCE}:N is line N of {pass_file}");
    if let Some(common) = common_file {
        let _ = write!(s, ", {COMMON_SOURCE}:N of {common}");
    }
    let _ = write!(s, ", {WRAPPER_SOURCE}:N is phonto's main()");
    s
}

/// Removes a `#version` line that was copied with the code, because phonto
/// adds its own. The empty line stays, so the line numbers in errors do not
/// change.
fn strip_version(code: &str) -> String {
    code.lines()
        .map(|line| {
            if line.trim_start().starts_with("#version") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_header_common_pass_and_main() {
        let src = fragment_shader(
            Some("float helper() { return 1.0; }"),
            "void mainImage(out vec4 c, in vec2 p) { c = vec4(helper()); }",
            PassKind::Buffer,
        );
        assert!(src.starts_with("#version 300 es\n"));
        let common = src.find("#line 1 1\nfloat helper").unwrap();
        let pass = src.find("#line 1 0\nvoid mainImage").unwrap();
        let main = src.find("#line 1 2\nvoid main()").unwrap();
        assert!(common < pass && pass < main);
        assert!(!src.contains(".a = 1.0"));
    }

    #[test]
    fn header_declares_every_channel() {
        let n = super::super::MAX_CHANNELS;
        assert!(HEADER.contains(&format!("uniform sampler2D iChannel{};", n - 1)));
        assert!(!HEADER.contains(&format!("iChannel{};", n)));
        assert!(HEADER.contains(&format!("iChannelResolution[{n}]")));
        assert!(HEADER.contains(&format!("iChannelTime[{n}]")));
    }

    #[test]
    fn header_declares_the_audio_arrays() {
        use super::super::audio::{NOTE_NAMES, SPECTRUM_BANDS};
        assert!(HEADER.contains(&format!("uniform float iSpectrum[{SPECTRUM_BANDS}];")));
        assert!(HEADER.contains(&format!("uniform float iChroma[{NOTE_NAMES}];")));
    }

    #[test]
    fn image_pass_forces_opaque_output() {
        let src = fragment_shader(
            None,
            "void mainImage(out vec4 c, in vec2 p) {}",
            PassKind::Image,
        );
        assert!(src.contains("phonto_FragColor.a = 1.0;"));
        assert!(!src.contains("#line 1 1"));
    }

    #[test]
    fn strips_copied_version_line_keeping_line_count() {
        let src = fragment_shader(
            None,
            "#version 300 es\nvoid mainImage(out vec4 c, in vec2 p) {}",
            PassKind::Image,
        );
        assert_eq!(src.matches("#version").count(), 1);
        assert!(src.contains("#line 1 0\n\nvoid mainImage"));
    }
}
