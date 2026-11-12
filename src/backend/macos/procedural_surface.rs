//! Procedural wallpapers on screen: one [`ProceduralSource`] for each
//! wallpaper, and one [`ProceduralSurface`] for each screen that shows it.
//!
//! Each surface has a display link that fires at the refresh of its screen.
//! The source keeps the clock. All surfaces of a source draw the same moment,
//! so mirrored screens stay in sync.
//!
//! With `--span`, the source draws one image for the whole desktop, and each
//! surface shows its part of it.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::sel;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_foundation::{MainThreadMarker, NSRect, NSRunLoop, NSRunLoopCommonModes};
use objc2_metal::{MTLCommandBuffer, MTLDrawable, MTLTexture};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAMetalDrawable, CAMetalLayer};

use super::audio::AudioCapture;
use super::battery_observer;
use super::keyboard::Keyboard;
use super::pointer::DesktopPointer;
use super::procedural::{self, Gpu, GpuTexture, Program, SCREEN_FORMAT, Target};
use super::screen_observer::ScreenObserver;
use super::span;
use crate::config::{InputCommand, Location};
use crate::procedural::{
    AudioFrame, Clock, CommandInputs, FrameInputs, FrameLimiter, Input, Pack, TextureWatcher,
    local_date, location,
};

/// How often `iBattery` reads the battery, as on Wayland.
const BATTERY_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// A procedural wallpaper, compiled, with its clock.
pub struct ProceduralSource {
    program: Program,
    fps: Option<f32>,
    clock: Clock,
    limiter: FrameLimiter,
    /// The inputs of the current frame that all screens share. Each surface
    /// adds its pointer and its display.
    inputs: FrameInputs,
    location: Option<Location>,
    /// The values of the inputs that the pack reads.
    commands: CommandInputs,
    /// True if the pack sets `audio = true`.
    reads_audio: bool,
    /// The system audio, if the pack reads it and phonto can read it.
    audio: Option<Rc<AudioCapture>>,
    /// `iBattery`, and when it was read.
    battery: [f32; 4],
    battery_read: Instant,
    watcher: Option<TextureWatcher>,
    /// Counts the frames of the source. A surface draws when this changes.
    frame: u64,
    /// The keys, if a pass has a keyboard channel. The window of the screen
    /// that has the keyboard fills it.
    pub keyboard: Option<Rc<RefCell<Keyboard>>>,
    keyboard_texture: Option<GpuTexture>,
    /// Set with `--span`.
    span: Option<Span>,
}

/// One image for the whole desktop, with `--span`. The first surface that
/// draws a frame of the source encodes it. The others only show their part.
struct Span {
    /// The frames of the screens, in the order of `NSScreen::screens`, which
    /// puts the primary screen first.
    screens: Vec<NSRect>,
    /// The rectangle that covers the screens.
    desktop: NSRect,
    target: Option<Target>,
    /// The pointer of the whole desktop.
    pointer: DesktopPointer,
    /// The last frame of the source that the target holds.
    frame: u64,
}

impl ProceduralSource {
    pub fn load(
        gpu: &Gpu,
        path: &str,
        location: Option<Location>,
        commands: &[InputCommand],
        span: bool,
    ) -> anyhow::Result<Self> {
        let pack = Pack::load(Path::new(path))?;
        log::info!(
            "procedural wallpaper `{}`: {} buffer pass(es), {} fps cap, render scale {}",
            pack.name,
            pack.buffers.len(),
            pack.fps.map_or_else(|| "no".to_string(), |f| f.to_string()),
            pack.render_scale
        );
        let reads_keys = pack
            .buffers
            .iter()
            .map(|b| &b.pass)
            .chain([&pack.image])
            .flat_map(|pass| &pass.channels)
            .any(|c| c.input == Input::Keyboard);
        Ok(Self {
            program: Program::compile(gpu, &pack)?,
            fps: pack.fps,
            clock: Clock::new(pack.fps),
            limiter: FrameLimiter::new(pack.fps),
            inputs: FrameInputs::default(),
            location,
            commands: CommandInputs::start(&pack.inputs, commands),
            reads_audio: pack.audio,
            audio: None,
            battery: battery_observer::uniform(),
            battery_read: Instant::now(),
            watcher: TextureWatcher::start(&pack.textures),
            frame: 0,
            keyboard: reads_keys.then(Rc::default),
            keyboard_texture: None,
            span: span.then(|| Span {
                screens: Vec::new(),
                desktop: span::desktop_rect(&[]),
                target: None,
                pointer: DesktopPointer::new(),
                frame: 0,
            }),
        })
    }

    pub fn reads_audio(&self) -> bool {
        self.reads_audio
    }

    /// Gives the wallpaper the system audio, if it reads it.
    pub fn set_audio(&mut self, capture: &Rc<AudioCapture>) {
        if self.reads_audio {
            self.audio = Some(Rc::clone(capture));
        }
    }

    pub fn spans(&self) -> bool {
        self.span.is_some()
    }

    /// Sets the frames of the screens that a spanned wallpaper covers, in the
    /// order of `NSScreen::screens`. A new size of the desktop makes a new
    /// target, and `iFrame` starts at 0 again.
    pub fn set_screens(&mut self, screens: &[NSRect]) {
        if let Some(span) = &mut self.span {
            span.desktop = span::desktop_rect(screens);
            span.screens = screens.to_vec();
        }
    }

    /// Starts the next frame if the frame-rate cap allows it. `slack` lets a
    /// display link that fires a little early still count. A cap that divides
    /// the refresh rate then keeps an even cadence.
    fn advance(
        &mut self,
        gpu: &Gpu,
        now: Instant,
        slack: Duration,
        color_scheme: f32,
    ) -> anyhow::Result<()> {
        if self.limiter.next_frame().is_some_and(|t| t > now + slack) {
            return Ok(());
        }
        if let Some(keyboard) = &self.keyboard
            && let Some(rows) = keyboard.borrow_mut().take_frame()
        {
            self.keyboard_texture = Some(procedural::keyboard_texture(gpu, &rows)?);
        }
        // Upload the new images of watched files.
        for (index, texture) in self.watcher.iter().flat_map(TextureWatcher::take) {
            if let Err(err) = self.program.replace_texture(gpu, index, &texture) {
                log::warn!("{err:#}");
            }
        }
        if now.saturating_duration_since(self.battery_read) >= BATTERY_POLL_INTERVAL {
            self.battery = battery_observer::uniform();
            self.battery_read = now;
        }
        let tick = self.clock.tick(now);
        let wall = SystemTime::now();
        self.inputs = FrameInputs {
            time: tick.time,
            time_delta: tick.delta,
            frame_rate: tick.frame_rate,
            date: local_date(wall),
            audio: self
                .audio
                .as_deref()
                .map_or_else(AudioFrame::default, AudioCapture::snapshot),
            color_scheme,
            battery: self.battery,
            location: location(wall, self.location),
            inputs: self.commands.values(),
            ..FrameInputs::default()
        };
        self.limiter.rendered(now);
        self.frame += 1;
        Ok(())
    }

    /// Stops running the input commands and reading the audio while the
    /// wallpaper is paused.
    pub fn set_paused(&self, paused: bool) {
        self.commands.set_paused(paused);
        if let Some(audio) = &self.audio {
            audio.set_paused(paused);
        }
    }
}

/// The layer of one screen that shows a procedural wallpaper.
pub struct ProceduralSurface {
    pub layer: Retained<CAMetalLayer>,
    /// The index of the source in the observer's list.
    pub source: usize,
    link: Option<Retained<CADisplayLink>>,
    target: Option<Target>,
    pointer: DesktopPointer,
    /// The last frame of the source that this surface drew.
    frame: u64,
}

impl ProceduralSurface {
    /// A layer that fills `view`. It draws nothing until [`Self::start`].
    pub fn new(gpu: &Gpu, view: &NSView, source: usize) -> anyhow::Result<Self> {
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&gpu.device));
        layer.setPixelFormat(SCREEN_FORMAT);
        layer.setFramebufferOnly(true);
        layer.setOpaque(true);
        // The shaders write sRGB, as on Shadertoy. Without a color space, a
        // wide-gamut display shows the values as its own, more saturated colors.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
        layer.setColorspace(srgb.as_deref());
        layer.setFrame(view.bounds());
        layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
        view.layer()
            .context("content view has no root layer")?
            .addSublayer(&layer);
        Ok(Self {
            layer,
            source,
            link: None,
            target: None,
            pointer: DesktopPointer::new(),
            frame: 0,
        })
    }

    /// Starts a display link on `view` that calls `frame:` on `target`.
    pub fn start(
        &mut self,
        view: &NSView,
        target: &ScreenObserver,
        source: &ProceduralSource,
        paused: bool,
    ) {
        // SAFETY: `ScreenObserver` answers `frame:`, which takes the link.
        let link = unsafe { view.displayLinkWithTarget_selector(target, sel!(frame:)) };
        if let Some(fps) = source.fps {
            // Core Animation ignores a range below 1 Hz and fires at the full
            // refresh. The limiter still holds the cap.
            let fps = fps.max(1.0);
            link.setPreferredFrameRateRange(CAFrameRateRange {
                minimum: fps,
                maximum: fps,
                preferred: fps,
            });
        }
        link.setPaused(paused);
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        self.link = Some(link);
    }

    pub fn is_driven_by(&self, link: &CADisplayLink) -> bool {
        self.link.as_deref().is_some_and(|l| std::ptr::eq(l, link))
    }

    pub fn set_paused(&self, paused: bool) {
        if let Some(link) = &self.link {
            link.setPaused(paused);
        }
    }

    /// Draws the next frame of `source`, if there is one. `frame` is the
    /// frame of the screen in global coordinates. Returns true if the user
    /// clicked the desktop since the last frame: on this screen, or with
    /// `--span` on any screen.
    pub fn draw(
        &mut self,
        mtm: MainThreadMarker,
        gpu: &Gpu,
        source: &mut ProceduralSource,
        frame: NSRect,
        link: &CADisplayLink,
        color_scheme: f32,
    ) -> anyhow::Result<bool> {
        let slack = Duration::from_secs_f64(link.duration() / 2.0);
        source.advance(gpu, Instant::now(), slack, color_scheme)?;
        if self.frame == source.frame {
            return Ok(false);
        }
        self.frame = source.frame;

        let cmd = gpu.command_buffer()?;
        let keyboard = source.keyboard_texture.as_deref();
        let (target, crop, clicked_desktop) = match &mut source.span {
            Some(span) => {
                let mut clicked = false;
                if span.frame != source.frame {
                    clicked =
                        span.encode(mtm, gpu, &cmd, &source.program, &source.inputs, keyboard)?;
                    span.frame = source.frame;
                }
                let target = span.target.as_ref().context("no spanned frame yet")?;
                (target, span::crop(span.desktop, frame), clicked)
            }
            None => {
                let surface = (
                    (frame.size.width.round() as u32).max(1),
                    (frame.size.height.round() as u32).max(1),
                );
                let dims = source.program.target_dims(surface);
                if self.target.as_ref().map(Target::dims) != Some(dims) {
                    self.target = Some(source.program.create_target(gpu, dims, false)?);
                }
                let target = self.target.as_mut().expect("target created above");

                let clicked = self.pointer.poll(mtm, frame);
                let (mouse, pointer) = self.pointer.uniforms(surface, dims);
                let (displays, display_count) = procedural::one_display(dims);
                let inputs = FrameInputs {
                    mouse,
                    pointer,
                    displays,
                    display_count,
                    ..source.inputs
                };
                source.program.encode(&cmd, target, &inputs, keyboard);
                (&*target, [0.0, 0.0, 1.0, 1.0], clicked)
            }
        };

        // The drawable holds the part of the image that this screen shows.
        // The layer scales it to the screen.
        let dims = target.dims();
        let part =
            |fraction: f32, size: u32| f64::from(((fraction * size as f32).round() as u32).max(1));
        let drawable_size = CGSize::new(part(crop[2], dims.0), part(crop[3], dims.1));
        if self.layer.drawableSize() != drawable_size {
            self.layer.setDrawableSize(drawable_size);
        }
        // Take the drawable last. It can wait until the screen frees one.
        if let Some(drawable) = self.layer.nextDrawable() {
            gpu.encode_present(&cmd, target, &drawable.texture(), crop);
            cmd.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(&*drawable));
        }
        cmd.commit();
        Ok(clicked_desktop)
    }
}

impl Span {
    /// Encodes the next frame of the whole desktop into `cmd`. Returns true
    /// if the user clicked the desktop since the last frame.
    fn encode(
        &mut self,
        mtm: MainThreadMarker,
        gpu: &Gpu,
        cmd: &ProtocolObject<dyn MTLCommandBuffer>,
        program: &Program,
        shared: &FrameInputs,
        keyboard: Option<&ProtocolObject<dyn MTLTexture>>,
    ) -> anyhow::Result<bool> {
        let size = (
            (self.desktop.size.width.round() as u32).max(1),
            (self.desktop.size.height.round() as u32).max(1),
        );
        let dims = program.target_dims(size);
        if self.target.as_ref().map(Target::dims) != Some(dims) {
            self.target = Some(program.create_target(gpu, dims, false)?);
        }
        let target = self.target.as_mut().expect("target created above");

        // The pointer is read once for each frame of the source. A second
        // read in the same frame would not see the click.
        let clicked = self.pointer.poll(mtm, self.desktop);
        let (mouse, pointer) = self.pointer.uniforms(size, dims);
        let (displays, display_count) = span::display_rects(self.desktop, &self.screens, dims);
        let inputs = FrameInputs {
            mouse,
            pointer,
            displays,
            display_count,
            ..*shared
        };
        program.encode(cmd, target, &inputs, keyboard);
        Ok(clicked)
    }
}

impl Drop for ProceduralSurface {
    fn drop(&mut self) {
        // The run loop keeps the link, and the link keeps its target.
        if let Some(link) = self.link.take() {
            link.invalidate();
        }
    }
}
