//! Reads what macOS plays, through a Core Audio process tap. The tap mixes
//! the output of all processes to mono. A private aggregate device reads it
//! at the sample rate of `Analyzer`, so Core Audio does the resampling. The
//! samples stay in memory. Only the smoothed levels go to the drawing.
//!
//! The first read asks for the System Audio Recording permission, for the
//! app that started phonto, such as the terminal. Without the permission,
//! the tap gives silence.

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::bail;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData, CATapDescription, CATapMuteBehavior,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyNominalSampleRate,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectUnknown,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::procedural::{Analyzer, AudioFrame, SAMPLE_RATE};

pub struct AudioCapture {
    tap: AudioObjectID,
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    /// The state of the IO callback. It is freed after the callback is gone.
    reader: *mut Reader,
    latest: Arc<Mutex<(AudioFrame, Instant)>>,
    running: Cell<bool>,
}

/// What the IO callback owns. It runs on the IO thread of Core Audio.
struct Reader {
    analyzer: Analyzer,
    latest: Arc<Mutex<(AudioFrame, Instant)>>,
}

impl AudioCapture {
    pub fn start() -> anyhow::Result<Self> {
        let latest = Arc::new(Mutex::new((AudioFrame::default(), Instant::now())));
        // The first lock allocates the mutex. Take it here, not on the IO thread.
        drop(latest.lock());
        // Drop frees what exists so far if a step fails.
        let mut capture = Self {
            tap: kAudioObjectUnknown,
            device: kAudioObjectUnknown,
            proc_id: None,
            reader: std::ptr::null_mut(),
            latest: Arc::clone(&latest),
            running: Cell::new(false),
        };

        let description = unsafe {
            CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &NSArray::new(),
            )
        };
        unsafe {
            description.setName(&NSString::from_str("phonto"));
            // Other processes do not see the tap. The sound still plays.
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
        }
        check(
            unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut capture.tap) },
            "create the audio tap",
        )?;
        let tap_uid = unsafe { description.UUID() }.UUIDString();

        let tap_list = NSArray::from_retained_slice(&[dictionary(&[
            (kAudioSubTapUIDKey, &tap_uid),
            (kAudioSubTapDriftCompensationKey, &NSNumber::new_bool(true)),
        ])]);
        let aggregate = dictionary(&[
            (kAudioAggregateDeviceNameKey, &NSString::from_str("phonto")),
            (kAudioAggregateDeviceUIDKey, &NSUUID::UUID().UUIDString()),
            (kAudioAggregateDeviceIsPrivateKey, &NSNumber::new_bool(true)),
            // After each start, the device reads nothing until a sound plays.
            // AudioDeviceStart still returns at once.
            (
                kAudioAggregateDeviceTapAutoStartKey,
                &NSNumber::new_bool(true),
            ),
            (kAudioAggregateDeviceTapListKey, &tap_list),
        ]);
        let aggregate: &CFDictionary<_, _> = (*aggregate).as_ref();
        check(
            unsafe {
                AudioHardwareCreateAggregateDevice(
                    aggregate.as_opaque(),
                    NonNull::from(&mut capture.device),
                )
            },
            "create the audio device of the tap",
        )?;

        // The device reads the tap at this rate. Core Audio resamples it.
        let rate = f64::from(SAMPLE_RATE);
        check(
            unsafe {
                AudioObjectSetPropertyData(
                    capture.device,
                    NonNull::from(&address(kAudioDevicePropertyNominalSampleRate)),
                    0,
                    std::ptr::null(),
                    size_of_val(&rate) as u32,
                    NonNull::from(&rate).cast(),
                )
            },
            "set the sample rate of the audio tap",
        )?;
        let mut actual = 0.0;
        let mut size = size_of_val(&actual) as u32;
        check(
            unsafe {
                AudioObjectGetPropertyData(
                    capture.device,
                    NonNull::from(&address(kAudioDevicePropertyNominalSampleRate)),
                    0,
                    std::ptr::null(),
                    NonNull::from(&mut size),
                    NonNull::from(&mut actual).cast(),
                )
            },
            "read the sample rate of the audio tap",
        )?;
        if actual != rate {
            bail!("the audio tap runs at {actual} Hz, not {rate} Hz");
        }

        capture.reader = Box::into_raw(Box::new(Reader {
            analyzer: Analyzer::default(),
            latest,
        }));
        check(
            unsafe {
                AudioDeviceCreateIOProcID(
                    capture.device,
                    Some(read_samples),
                    capture.reader.cast(),
                    NonNull::from(&mut capture.proc_id),
                )
            },
            "add a reader to the audio tap",
        )?;
        capture.run(true)?;
        log::info!("reading the system audio through a Core Audio tap");
        Ok(capture)
    }

    /// Stops reading while the wallpaper is paused.
    pub fn set_paused(&self, paused: bool) {
        if let Err(err) = self.run(!paused) {
            log::warn!("{err:#}");
        }
    }

    fn run(&self, running: bool) -> anyhow::Result<()> {
        if self.running.get() == running {
            return Ok(());
        }
        if running {
            check(
                unsafe { AudioDeviceStart(self.device, self.proc_id) },
                "start the audio tap",
            )?;
        } else {
            check(
                unsafe { AudioDeviceStop(self.device, self.proc_id) },
                "stop the audio tap",
            )?;
        }
        self.running.set(running);
        Ok(())
    }

    pub fn snapshot(&self) -> AudioFrame {
        let Ok(state) = self.latest.lock() else {
            return AudioFrame::default();
        };
        let (frame, updated) = *state;
        faded(frame, updated.elapsed())
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        unsafe {
            if self.proc_id.is_some() {
                AudioDeviceStop(self.device, self.proc_id);
                AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            }
            if self.device != kAudioObjectUnknown {
                AudioHardwareDestroyAggregateDevice(self.device);
            }
            if self.tap != kAudioObjectUnknown {
                AudioHardwareDestroyProcessTap(self.tap);
            }
            // The callback is gone, so nothing else reads the state.
            if !self.reader.is_null() {
                drop(Box::from_raw(self.reader));
            }
        }
    }
}

/// The IO callback of the device. The input is one stream of 32-bit floats.
unsafe extern "C-unwind" fn read_samples(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let reader = unsafe { &mut *client.cast::<Reader>() };
    let list = unsafe { input.as_ref() };
    if list.mNumberBuffers == 0 {
        return 0;
    }
    let AudioBuffer {
        mNumberChannels: channels,
        mDataByteSize: bytes,
        mData: data,
    } = list.mBuffers[0];
    if data.is_null() {
        return 0;
    }
    let samples = unsafe {
        std::slice::from_raw_parts(data.cast::<f32>(), bytes as usize / size_of::<f32>())
    };
    // The tap is mono. With more channels, the first one is enough.
    for &sample in samples.iter().step_by(channels.max(1) as usize) {
        // The IO thread must not wait for the drawing. A frame that finds
        // the lock taken is dropped. The next one comes 23 ms later.
        if let Some(frame) = reader.analyzer.push(sample)
            && let Ok(mut latest) = reader.latest.try_lock()
        {
            *latest = (frame, Instant::now());
        }
    }
    0
}

/// `frame`, faded out if it is `age` old. If the sound stops, or the tap
/// stops delivering, the scene then does not stop on a beat.
fn faded(mut frame: AudioFrame, age: Duration) -> AudioFrame {
    let fade = (-6.0 * (age.as_secs_f32() - 0.15).max(0.0)).exp();
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

fn check(status: i32, what: &str) -> anyhow::Result<()> {
    if status != 0 {
        bail!("{what}: OSStatus {status}");
    }
    Ok(())
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// A dictionary for Core Audio, with C string keys.
fn dictionary(entries: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries
        .iter()
        .map(|(key, _)| NSString::from_str(&key.to_string_lossy()))
        .collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&keys, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_frame_fades_out() {
        let mut chroma = [0.0; 12];
        chroma[3] = 1.0;
        let frame = AudioFrame {
            levels: [1.0; 4],
            chroma,
            ..AudioFrame::default()
        };
        assert_eq!(faded(frame, Duration::from_millis(100)), frame);
        let old = faded(frame, Duration::from_secs(1));
        assert!(old.levels.iter().all(|&v| v < 0.01), "{old:?}");
        assert!(old.chroma[3] < 0.01);
    }
}
