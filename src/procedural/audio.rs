//! The audio input of a frame. `Analyzer` turns the samples of the system
//! output into the smoothed levels of `iAudio`, `iSpectrum`, and `iChroma`.
//! Each backend captures the samples.

use std::f32::consts::{PI, TAU};

/// The rate of the mono samples that `Analyzer::push` takes.
pub const SAMPLE_RATE: f32 = 22_050.0;

/// The number of `iSpectrum` bands. The GLSL header declares the same size.
pub const SPECTRUM_BANDS: usize = 32;

/// The frequency range of `iSpectrum`. The bands have a logarithmic spacing.
const LOWEST_HZ: f32 = 40.0;
const HIGHEST_HZ: f32 = 11_000.0;

/// The number of `iChroma` values: one for each note name, C to B.
pub const NOTE_NAMES: usize = 12;

/// The frequency range of `iChroma`. Below it, the notes are too close
/// together for the long window. Above it, the bins hold mostly overtones.
const CHROMA_LOWEST_HZ: f32 = 130.0;
const CHROMA_HIGHEST_HZ: f32 = 5_000.0;

/// The spectrum uses a short window, so that it reacts fast.
const WINDOW: usize = 1024;
/// The chroma uses a long window. Its bins are 5.4 Hz wide, so it can
/// separate two neighbor notes from about 180 Hz.
const LONG_WINDOW: usize = 4096;
/// The analysis runs after each `HOP` new samples, about 43 times each
/// second.
const HOP: usize = WINDOW / 2;

/// The smoothed levels of the system output. Each value is from 0 to 1. The
/// four levels are bass, midrange, treble, and overall volume. The chroma is
/// the strength of each note name, C to B, over all octaves. While sound
/// plays, the strongest note name is near 1.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioFrame {
    pub levels: [f32; 4],
    pub spectrum: [f32; SPECTRUM_BANDS],
    pub chroma: [f32; NOTE_NAMES],
}

pub struct Analyzer {
    /// The last `LONG_WINDOW` samples, with the newest at the end. At the
    /// start, the older samples are silence.
    samples: [f32; LONG_WINDOW],
    filled: usize,
    frame: AudioFrame,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            samples: [0.0; LONG_WINDOW],
            filled: LONG_WINDOW - WINDOW,
            frame: AudioFrame::default(),
        }
    }
}

impl Analyzer {
    /// Adds one sample. Returns a new frame when the window of samples is
    /// full.
    pub fn push(&mut self, sample: f32) -> Option<AudioFrame> {
        self.samples[self.filled] = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.filled += 1;
        if self.filled < LONG_WINDOW {
            return None;
        }
        let recent = &self.samples[LONG_WINDOW - WINDOW..];
        let rms = (recent.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).sqrt();
        self.update_spectrum(rms);
        self.update_chroma(rms < 0.0001);
        self.samples.copy_within(HOP.., 0);
        self.filled = LONG_WINDOW - HOP;
        Some(self.frame)
    }

    fn update_spectrum(&mut self, rms: f32) {
        let mut real = [0.0; WINDOW];
        let mut imag = [0.0; WINDOW];
        hann(&self.samples[LONG_WINDOW - WINDOW..], &mut real);
        fft(&mut real, &mut imag);
        let mut levels = [0.0f32; 4];
        levels[3] = ((rms - 0.0005).max(0.0) * 5.0).min(1.0).sqrt();
        let edge = |band: usize| {
            LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(band as f32 / SPECTRUM_BANDS as f32)
        };
        for i in 0..SPECTRUM_BANDS {
            let (low, high) = (edge(i), edge(i + 1));
            let start = ((low * WINDOW as f32 / SAMPLE_RATE).floor() as usize).max(1);
            let end =
                ((high * WINDOW as f32 / SAMPLE_RATE).ceil() as usize).clamp(start + 1, WINDOW / 2);
            let power = (start..end)
                .map(|k| real[k] * real[k] + imag[k] * imag[k])
                .fold(0.0f32, f32::max);
            let amplitude = power.sqrt() * 4.0 / WINDOW as f32;
            let target = if rms < 0.0001 {
                0.0
            } else {
                ((20.0 * amplitude.max(1e-8).log10() + 72.0) / 66.0).clamp(0.0, 1.0)
            };
            smooth(&mut self.frame.spectrum[i], target);
            let group = if low < 220.0 {
                0
            } else if low < 3500.0 {
                1
            } else {
                2
            };
            levels[group] = levels[group].max(target);
        }
        for (value, target) in self.frame.levels.iter_mut().zip(levels) {
            smooth(value, target);
        }
    }

    /// Adds the strength of each bin of the long window to its note name.
    fn update_chroma(&mut self, silent: bool) {
        let mut real = [0.0; LONG_WINDOW];
        let mut imag = [0.0; LONG_WINDOW];
        hann(&self.samples, &mut real);
        fft(&mut real, &mut imag);
        let bin_hz = SAMPLE_RATE / LONG_WINDOW as f32;
        let first = (CHROMA_LOWEST_HZ / bin_hz).ceil() as usize;
        let last = (CHROMA_HIGHEST_HZ / bin_hz) as usize;
        let mut chroma = [0.0f32; NOTE_NAMES];
        for k in first..=last {
            // The MIDI number of the bin. 69 is A at 440 Hz, and 60 is C.
            let note = 69.0 + 12.0 * (k as f32 * bin_hz / 440.0).log2();
            let nearest = note.round();
            // A bin between two notes counts less.
            let weight = (PI * (note - nearest)).cos().powi(2);
            let name = (nearest as i32).rem_euclid(NOTE_NAMES as i32) as usize;
            chroma[name] += weight * (real[k] * real[k] + imag[k] * imag[k]).sqrt();
        }
        let strongest = chroma.iter().copied().fold(0.0, f32::max);
        for (value, strength) in self.frame.chroma.iter_mut().zip(chroma) {
            let target = if silent || strongest <= 0.0 {
                0.0
            } else {
                strength / strongest
            };
            smooth(value, target);
        }
    }
}

/// Copies `samples` into `out` with a Hann window, which softens the edges
/// of the window.
fn hann(samples: &[f32], out: &mut [f32]) {
    let last = (samples.len() - 1) as f32;
    for (i, (out, &sample)) in out.iter_mut().zip(samples).enumerate() {
        *out = sample * (0.5 - 0.5 * (TAU * i as f32 / last).cos());
    }
}

fn smooth(value: &mut f32, target: f32) {
    let amount = if target > *value { 0.60 } else { 0.10 };
    *value += (target - *value) * amount;
}

/// A radix-2 FFT that works in place. The length must be a power of two. The
/// windows are small, so phonto does not need an FFT crate.
fn fft(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    debug_assert!(n.is_power_of_two() && imag.len() == n);
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - n.ilog2());
        if j > i {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= n {
        let (sin, cos) = (-TAU / width as f32).sin_cos();
        for base in (0..n).step_by(width) {
            let (mut wr, mut wi) = (1.0, 0.0);
            for offset in 0..width / 2 {
                let a = base + offset;
                let b = a + width / 2;
                let tr = wr * real[b] - wi * imag[b];
                let ti = wr * imag[b] + wi * real[b];
                real[b] = real[a] - tr;
                imag[b] = imag[a] - ti;
                real[a] += tr;
                imag[a] += ti;
                (wr, wi) = (wr * cos - wi * sin, wr * sin + wi * cos);
            }
        }
        width *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays a sine wave for each of `frequencies` and returns the last frame.
    fn tone(analyzer: &mut Analyzer, frequencies: &[f32], amplitude: f32) -> AudioFrame {
        let mut frame = AudioFrame::default();
        for i in 0..(WINDOW * 12) {
            let t = i as f32 / SAMPLE_RATE;
            let sample: f32 = frequencies.iter().map(|f| (TAU * f * t).sin()).sum();
            if let Some(next) = analyzer.push(sample * amplitude) {
                frame = next;
            }
        }
        frame
    }

    #[test]
    fn silence_stays_silent_and_invalid_samples_are_safe() {
        let mut analyzer = Analyzer::default();
        assert_eq!(tone(&mut analyzer, &[110.0], 0.0), AudioFrame::default());
        for _ in 0..WINDOW * 2 {
            analyzer.push(f32::NAN);
        }
        assert_eq!(analyzer.frame, AudioFrame::default());
    }

    #[test]
    fn bass_and_treble_land_in_different_bands() {
        let bass = tone(&mut Analyzer::default(), &[110.0], 0.2);
        let treble = tone(&mut Analyzer::default(), &[6000.0], 0.2);
        assert!(bass.levels[0] > 0.7 && bass.levels[2] < 0.05, "{bass:?}");
        assert!(
            treble.levels[2] > 0.7 && treble.levels[0] < 0.05,
            "{treble:?}"
        );
        let peak = |f: AudioFrame| {
            f.spectrum
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        };
        assert!(peak(bass) < 12);
        assert!(peak(treble) > 25);
    }

    #[test]
    fn output_is_bounded_and_returns_to_silence() {
        let mut analyzer = Analyzer::default();
        let loud = tone(&mut analyzer, &[1000.0], 3.0);
        assert!(
            loud.levels
                .iter()
                .chain(loud.spectrum.iter())
                .chain(loud.chroma.iter())
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        );
        assert!(loud.levels[3] > 0.9);
        let quiet = tone(&mut analyzer, &[1000.0], 0.0);
        assert!(quiet.levels[3] < loud.levels[3] * 0.2);
    }

    #[test]
    fn chroma_names_notes_and_chords() {
        // The `count` strongest note names, in order from C.
        let strongest = |frame: &AudioFrame, count: usize| {
            let mut names: Vec<usize> = (0..NOTE_NAMES).collect();
            names.sort_by(|a, b| frame.chroma[*b].total_cmp(&frame.chroma[*a]));
            names.truncate(count);
            names.sort();
            names
        };
        let a = tone(&mut Analyzer::default(), &[440.0], 0.2);
        assert_eq!(strongest(&a, 1), [9], "{:?}", a.chroma);
        let low_g = tone(&mut Analyzer::default(), &[196.0], 0.2);
        assert_eq!(strongest(&low_g, 1), [7], "{:?}", low_g.chroma);
        let c_major = tone(&mut Analyzer::default(), &[261.63, 329.63, 392.0], 0.1);
        assert_eq!(strongest(&c_major, 3), [0, 4, 7], "{:?}", c_major.chroma);
    }
}
