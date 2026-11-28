//! Reads the PulseAudio playback monitor. PipeWire also gives this monitor
//! through `pipewire-pulse`. The samples stay in memory. Only the smoothed
//! levels go to the drawing thread.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::procedural::{Analyzer, AudioFrame, SAMPLE_RATE};

pub struct AudioCapture {
    pipeline: gst::Pipeline,
    bus: gst::Bus,
    latest: Arc<Mutex<(AudioFrame, Instant)>>,
    warned: bool,
}

impl AudioCapture {
    pub fn start() -> anyhow::Result<Self> {
        gst::init().context("initialize GStreamer for system audio")?;
        // Name the monitor of the default output. The default source can be
        // a microphone.
        let source = gst::ElementFactory::make("pulsesrc")
            .property("device", "@DEFAULT_MONITOR@")
            .property("client-name", "phonto visualizer")
            .property("buffer-time", 80_000i64)
            .property("latency-time", 20_000i64)
            .build()
            .context("create pulsesrc (install the GStreamer PulseAudio plugin)")?;
        let capture = Self::from_source(source)?;
        log::info!("reading audio from the monitor of the default output");
        Ok(capture)
    }

    fn from_source(source: gst::Element) -> anyhow::Result<Self> {
        let convert = gst::ElementFactory::make("audioconvert").build()?;
        let resample = gst::ElementFactory::make("audioresample").build()?;
        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("channels", 1i32)
            .field("rate", SAMPLE_RATE as i32)
            .build();
        let sink = gst_app::AppSink::builder()
            .caps(&caps)
            .sync(false)
            .max_buffers(2)
            .drop(true)
            .build();
        let pipeline = gst::Pipeline::default();
        pipeline.add_many([&source, &convert, &resample, sink.upcast_ref()])?;
        gst::Element::link_many([&source, &convert, &resample, sink.upcast_ref()])?;

        let latest = Arc::new(Mutex::new((AudioFrame::default(), Instant::now())));
        let shared = Arc::clone(&latest);
        let mut analyzer = Analyzer::default();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    for bytes in map.as_slice().as_chunks::<4>().0 {
                        let value = f32::from_le_bytes(*bytes);
                        if let Some(frame) = analyzer.push(value)
                            && let Ok(mut state) = shared.lock()
                        {
                            *state = (frame, Instant::now());
                        }
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let capture = Self {
            bus: pipeline.bus().context("audio pipeline has no bus")?,
            pipeline,
            latest,
            warned: false,
        };
        capture
            .pipeline
            .set_state(gst::State::Playing)
            .context("start system audio monitor")?;
        capture
            .pipeline
            .state(gst::ClockTime::from_seconds(2))
            .0
            .context("connect to system audio monitor")?;
        Ok(capture)
    }

    /// Stops reading while the wallpaper is paused. While a stream reads the
    /// monitor, the sound server keeps the output device awake.
    pub fn set_paused(&self, paused: bool) {
        let state = if paused {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        if let Err(err) = self.pipeline.set_state(state) {
            log::warn!("set system audio monitor to {state:?}: {err}");
        }
    }

    pub fn snapshot(&mut self) -> AudioFrame {
        for message in self.bus.iter() {
            if let gst::MessageView::Error(error) = message.view()
                && !self.warned
            {
                log::warn!(
                    "the system audio monitor stopped, so the wallpaper draws without audio: {}",
                    error.error()
                );
                self.warned = true;
            }
        }
        let Ok(state) = self.latest.lock() else {
            return AudioFrame::default();
        };
        let (mut frame, updated) = *state;
        // If the sound server stops or suspends, the levels fade out. Then the
        // scene does not stop on a beat.
        let fade = (-6.0 * (updated.elapsed().as_secs_f32() - 0.15).max(0.0)).exp();
        for value in frame
            .levels
            .iter_mut()
            .chain(frame.spectrum.iter_mut())
            .chain(frame.chroma.iter_mut())
        {
            *value *= fade;
        }
        frame
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_pipeline_delivers_audio_and_fades_after_silence() {
        gst::init().unwrap();
        // A test tone goes through the real conversion and appsink path. The
        // test does not need a speaker or a sound server.
        let source = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .property("freq", 110.0f64)
            .property("volume", 0.2f64)
            .build()
            .unwrap();
        let mut capture = AudioCapture::from_source(source.clone()).unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let frame = capture.snapshot();
            if frame.levels[0] > 0.6 && frame.levels[3] > 0.5 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "capture did not deliver the tone: {frame:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        source.set_property("volume", 0.0f64);
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let frame = capture.snapshot();
            if frame.levels.iter().all(|v| *v < 0.04) {
                break;
            }
            assert!(Instant::now() < deadline, "capture did not fade: {frame:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
