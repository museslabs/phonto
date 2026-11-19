//! Reads the PulseAudio playback monitor. PipeWire also gives this monitor
//! through `pipewire-pulse`. The samples stay in memory. Only the smoothed
//! levels go to the drawing thread.
//!
//! The sound server connects the capture to the monitor of the output that
//! is the default when the capture starts. A thread asks the server for the
//! default output every 2 seconds. When the default output changes, the
//! capture connects again, so that it reads the output that you hear.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::Context;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::procedural::{Analyzer, AudioFrame, SAMPLE_RATE};

const OUTPUT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

pub struct AudioCapture {
    pipeline: gst::Pipeline,
    bus: gst::Bus,
    latest: Arc<Mutex<(AudioFrame, Instant)>>,
    warned: bool,
    /// Tells when the default output changes. It is `None` if the capture
    /// does not read the monitor of the default output, as in the tests.
    output: Option<OutputWatcher>,
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
        let mut capture = Self::from_source(source)?;
        log::info!("reading audio from the monitor of the default output");
        capture.output = OutputWatcher::start();
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
            output: None,
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
        if let Some(output) = &self.output {
            output.paused.store(paused, Ordering::Relaxed);
        }
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
        if self.output.as_mut().is_some_and(OutputWatcher::changed) {
            self.reconnect();
        }
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

    /// Connects the capture again. The sound server then gives it the
    /// monitor of the output that is the default now.
    fn reconnect(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Err(err) = self.pipeline.set_state(gst::State::Playing) {
            log::warn!(
                "cannot connect to the monitor of the new default output, so the wallpaper \
                 draws without audio: {err}"
            );
        }
        self.warned = false;
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Asks the sound server for the default output, from a thread of its own,
/// because each question takes some milliseconds.
struct OutputWatcher {
    /// The number of changes of the default output that the thread saw.
    changes: Arc<AtomicU64>,
    /// The number of changes that the capture followed.
    followed: u64,
    /// True while the wallpaper is paused. The thread then does not ask the
    /// server.
    paused: Arc<AtomicBool>,
    /// The thread stops when this sender drops.
    _stop: mpsc::Sender<()>,
}

impl OutputWatcher {
    /// Starts the thread. `None` if the GStreamer PulseAudio plugin has no
    /// device provider. The capture then stays on the output that was the
    /// default when it started.
    fn start() -> Option<Self> {
        let Some(factory) = gst::DeviceProviderFactory::find("pulsedeviceprovider") else {
            log::warn!(
                "phonto cannot see the default output, so the audio stays on the output that \
                 was the default at the start"
            );
            return None;
        };
        let changes = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread_changes = Arc::clone(&changes);
        let thread_paused = Arc::clone(&paused);
        let spawned = std::thread::Builder::new()
            .name("default-output".into())
            .spawn(move || {
                let mut last = default_output(&factory);
                // The wait ends with a timeout after each interval, and with
                // a disconnection when the watcher drops.
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(OUTPUT_CHECK_INTERVAL)
                {
                    if thread_paused.load(Ordering::Relaxed) {
                        continue;
                    }
                    let Some(now) = default_output(&factory) else {
                        continue;
                    };
                    if last.as_ref().is_some_and(|last| *last != now) {
                        log::info!("the default output is now {now}, so phonto reads its monitor");
                        thread_changes.fetch_add(1, Ordering::Relaxed);
                    }
                    last = Some(now);
                }
            });
        if let Err(err) = spawned {
            log::warn!("cannot start the thread that watches the default output: {err}");
            return None;
        }
        Some(Self {
            changes,
            followed: 0,
            paused,
            _stop: stop,
        })
    }

    /// True once after each change of the default output.
    fn changed(&mut self) -> bool {
        let changes = self.changes.load(Ordering::Relaxed);
        let changed = changes != self.followed;
        self.followed = changes;
        changed
    }
}

/// The name of the default output of the sound server. Each call starts a
/// new provider, because a provider asks the server for the default output
/// only when it starts. It does not report a change later.
fn default_output(factory: &gst::DeviceProviderFactory) -> Option<String> {
    let provider = factory.get()?;
    provider.start().ok()?;
    let name = provider
        .devices()
        .into_iter()
        .filter(|d| d.device_class() == "Audio/Sink")
        .filter(|d| d.find_property("internal-name").is_some())
        .find(|d| {
            d.properties()
                .is_some_and(|p| p.get::<bool>("is-default").unwrap_or(false))
        })
        .map(|d| d.property::<String>("internal-name"));
    provider.stop();
    name
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
