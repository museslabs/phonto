//! Procedural wallpapers on screen: one [`ProceduralSource`] for each
//! wallpaper, and one [`ProceduralSurface`] for each screen that shows it.
//!
//! Each surface has a display link that fires at the refresh of its screen.
//! The source keeps the clock. Only a surface that has drawn or skipped the
//! newest frame of its source starts the next one. The source then starts at
//! most one frame for each refresh of its fastest screen. A slower screen
//! shows whichever frame is newest when its link fires.
//!
//! A [`Scene`] is one image and the pointer over it. Without `--span`, each
//! surface has its own. With `--span`, the source has one for the whole
//! desktop, and each surface shows its part of it.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{Message, sel};
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_foundation::{MainThreadMarker, NSRect, NSRunLoop, NSRunLoopCommonModes};
use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandBufferStatus, MTLDrawable, MTLTexture};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAMetalDrawable, CAMetalLayer};

use super::audio::AudioCapture;
use super::battery_observer;
use super::keyboard::Keyboard;
use super::pointer::DesktopPointer;
use super::procedural::{self, Gpu, GpuBuffer, GpuTexture, Program, SCREEN_FORMAT, Target};
use super::screen_observer::ScreenObserver;
use super::span;
use super::video_channel::{self, VideoChannel};
use crate::config::{ActionCommand, InputCommand, Location};
use crate::procedural::{
    ActionRunner, AudioFrame, Clock, CommandInputs, FrameInputs, FrameLimiter, Input, Pack,
    TextureWatcher, local_date, location,
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
    /// The videos of the channels, in the order of `Pack::videos`.
    videos: Vec<VideoChannel>,
    /// Counts the frames of the source. A surface draws when this changes.
    frame: u64,
    /// The keys, if a pass has a keyboard channel. The window of the screen
    /// that has the keyboard fills it.
    pub keyboard: Option<Rc<RefCell<Keyboard>>>,
    keyboard_texture: Option<GpuTexture>,
    /// The commands of the actions, if the pack has actions.
    actions: Option<ActionRunner>,
    /// True if the scenes read the pointer: for `iMouse` or `iPointer`, for
    /// the click that gives a keyboard channel the keys, or for the clicks
    /// that run actions.
    polls_pointer: bool,
    /// Set with `--span`.
    span: Option<Span>,
    /// True while none of its surfaces runs.
    paused: bool,
}

/// The desktop, with `--span`. The first surface that draws a frame of the
/// source encodes the scene. The others only show their part.
struct Span {
    /// The frames of the screens, in the order of `NSScreen::screens`, which
    /// puts the primary screen first.
    screens: Vec<NSRect>,
    scene: Scene,
}

/// An image of one or more screens, and the pointer over them.
struct Scene {
    target: Option<Target>,
    pointer: Option<DesktopPointer>,
    /// The last frame of the source that the target holds.
    frame: u64,
    /// The frames of the screens that the target shows.
    screens: Vec<NSRect>,
    /// The actions under presses, oldest first, until the GPU has read them.
    action_reads: Vec<ActionRead>,
}

/// The actions under the presses of one frame, as the GPU reads them.
struct ActionRead {
    /// The frame that reads them. The texels are ready when it completes.
    cmd: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    /// The texel under each press. Its red byte is the action.
    texels: GpuBuffer,
}

impl ProceduralSource {
    pub fn load(
        gpu: &Gpu,
        path: &str,
        location: Option<Location>,
        commands: &[InputCommand],
        actions: &[ActionCommand],
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
        let program = Program::compile(gpu, &pack, actions)?;
        let videos = pack
            .videos
            .iter()
            .map(|path| VideoChannel::start(path, &gpu.device))
            .collect::<anyhow::Result<_>>()?;
        let polls_pointer = reads_keys || program.reads_pointer() || pack.actions.is_some();
        Ok(Self {
            program,
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
            videos,
            frame: 0,
            keyboard: reads_keys.then(Rc::default),
            keyboard_texture: None,
            actions: pack
                .actions
                .as_ref()
                .map(|a| ActionRunner::new(&a.names, actions)),
            polls_pointer,
            span: span.then(|| Span {
                screens: Vec::new(),
                scene: Scene::new(polls_pointer),
            }),
            paused: false,
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

    /// The system audio, if the wallpaper reads it.
    pub fn audio(&self) -> Option<&AudioCapture> {
        self.audio.as_deref()
    }

    pub fn spans(&self) -> bool {
        self.span.is_some()
    }

    /// Sets the frames of the screens that a spanned wallpaper covers, in the
    /// order of `NSScreen::screens`. A new size of the desktop makes a new
    /// target, and `iFrame` starts at 0 again.
    pub fn set_screens(&mut self, screens: &[NSRect]) {
        if let Some(span) = &mut self.span {
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
        self.advance_videos(gpu, f64::from(tick.delta))?;
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

    /// Moves the videos `delta` seconds on, and copies their new frames into
    /// their channels. With `--span`, this runs once for each frame of the
    /// source, however many screens show it.
    fn advance_videos(&mut self, gpu: &Gpu, delta: f64) -> anyhow::Result<()> {
        let wait = video_channel::catch_up_wait(delta);
        let mut cmd = None;
        for (index, video) in self.videos.iter_mut().enumerate() {
            let Some(frame) = video.advance(delta, wait)? else {
                continue;
            };
            let cmd = match &mut cmd {
                Some(cmd) => cmd,
                cmd => cmd.insert(gpu.command_buffer()?),
            };
            self.program
                .encode_video_frame(gpu, cmd, index, frame.texture())?;
            video.hold(frame, cmd);
        }
        // The scene's passes run after the copies, on the same queue.
        if let Some(cmd) = cmd {
            cmd.commit();
        }
        Ok(())
    }

    /// Stops running the input commands while the wallpaper is paused. A
    /// spanned wallpaper also forgets the clicks of the pause.
    pub fn set_paused(&mut self, paused: bool) {
        if std::mem::replace(&mut self.paused, paused) == paused {
            return;
        }
        self.commands.set_paused(paused);
        if !paused && let Some(span) = &mut self.span {
            span.scene.forget_clicks();
        }
    }
}

/// The layer of one screen that shows a procedural wallpaper.
pub struct ProceduralSurface {
    pub layer: Retained<CAMetalLayer>,
    /// The index of the source in the observer's list.
    pub source: usize,
    link: Option<Retained<CADisplayLink>>,
    /// The image of this screen, without `--span`. The first frame makes it.
    scene: Option<Scene>,
    /// The last frame of the source that this surface drew or skipped.
    frame: u64,
    /// True after a frame failed, until a frame draws.
    pub failing: bool,
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
            scene: None,
            frame: 0,
            failing: false,
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

    pub fn set_paused(&mut self, paused: bool) {
        let Some(link) = &self.link else {
            return;
        };
        if link.isPaused() == paused {
            return;
        }
        link.setPaused(paused);
        if !paused && let Some(scene) = &mut self.scene {
            scene.forget_clicks();
        }
    }

    /// Draws the next frame of `source`, if there is one. `frame` is the
    /// frame of the screen in global coordinates. Returns true if a click on
    /// the desktop ended since the last frame: on this screen, or with
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
        // A surface that lags only shows the newest frame. If every link
        // started one, two screens would start two frames in each refresh.
        if self.frame == source.frame {
            let slack = Duration::from_secs_f64(link.duration() / 2.0);
            source.advance(gpu, Instant::now(), slack, color_scheme)?;
            if self.frame == source.frame {
                return Ok(false);
            }
        }

        let (scene, screens) = match &mut source.span {
            Some(span) => (&mut span.scene, span.screens.as_slice()),
            None => (
                self.scene
                    .get_or_insert_with(|| Scene::new(source.polls_pointer)),
                std::slice::from_ref(&frame),
            ),
        };
        if scene.frame == source.frame && scene.screens != screens {
            // Another screen drew this frame before the screens changed. This
            // screen's part of it would be wrong. Keep the last image. This
            // surface may start the next frame, because the screen that drew
            // this one may be gone.
            self.frame = source.frame;
            return Ok(false);
        }
        let cmd = gpu.command_buffer()?;
        let mut clicked_desktop = false;
        if scene.frame != source.frame {
            if let Some(actions) = &source.actions {
                scene.run_actions(actions);
            }
            screens.clone_into(&mut scene.screens);
            let keyboard = source.keyboard_texture.as_deref();
            clicked_desktop =
                scene.encode(mtm, gpu, &cmd, &source.program, &source.inputs, keyboard)?;
            scene.frame = source.frame;
        }
        let target = scene.target.as_ref().context("no frame drawn yet")?;
        let crop = span::crop(span::desktop_rect(screens), frame);

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
        let mut copied = Ok(());
        if let Some(drawable) = self.layer.nextDrawable() {
            copied = gpu.encode_present(&cmd, target, &drawable.texture(), crop);
            if copied.is_ok() {
                cmd.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(&*drawable));
            }
        }
        // The passes run without the copy too. The scene has counted them.
        cmd.commit();
        self.frame = source.frame;
        copied.map(|()| clicked_desktop)
    }
}

impl Scene {
    fn new(polls_pointer: bool) -> Self {
        Self {
            target: None,
            pointer: polls_pointer.then(DesktopPointer::new),
            frame: 0,
            screens: Vec::new(),
            action_reads: Vec::new(),
        }
    }

    /// Encodes the next frame of the screens into `cmd`. Returns true if a
    /// click on their desktop ended since the last frame.
    fn encode(
        &mut self,
        mtm: MainThreadMarker,
        gpu: &Gpu,
        cmd: &ProtocolObject<dyn MTLCommandBuffer>,
        program: &Program,
        shared: &FrameInputs,
        keyboard: Option<&ProtocolObject<dyn MTLTexture>>,
    ) -> anyhow::Result<bool> {
        let rect = span::desktop_rect(&self.screens);
        let size = (
            (rect.size.width.round() as u32).max(1),
            (rect.size.height.round() as u32).max(1),
        );
        let dims = program.target_dims(size);
        if self.target.as_ref().map(Target::dims) != Some(dims) {
            self.target = Some(program.create_target(gpu, dims, false)?);
        }
        let target = self.target.as_mut().expect("target created above");

        // The pointer is read once for each frame of the scene. A second
        // read in the same frame would not see the click. A new target has
        // no frame to read actions from. Its clicks wait for its first frame,
        // and the action and the shader then see a click on the same frame.
        let drawn = target.has_drawn();
        let (clicked, (mouse, pointer), read) = match &mut self.pointer {
            Some(p) => {
                let clicked = p.poll(mtm, rect);
                let read = if drawn {
                    program.encode_action_reads(gpu, cmd, target, &p.take_presses(size))?
                } else {
                    None
                };
                (clicked, p.uniforms(size, dims, drawn), read)
            }
            None => (false, (shared.mouse, shared.pointer), None),
        };
        let (displays, display_count) = span::display_rects(rect, &self.screens, dims);
        let inputs = FrameInputs {
            mouse,
            pointer,
            displays,
            display_count,
            ..*shared
        };
        // The next frame starts the buffers again from zero.
        program
            .encode(cmd, target, &inputs, keyboard)
            .inspect_err(|_| self.target = None)?;
        if let Some(texels) = read {
            self.action_reads.push(ActionRead {
                cmd: cmd.retain(),
                texels,
            });
        }
        Ok(clicked)
    }

    /// Runs the actions under the presses of the frames that the GPU has
    /// finished. A display link callback never waits for the GPU.
    fn run_actions(&mut self, actions: &ActionRunner) {
        // The frames of one queue complete in order.
        let done = self
            .action_reads
            .iter()
            .take_while(|r| {
                matches!(
                    r.cmd.status(),
                    MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
                )
            })
            .count();
        for read in self.action_reads.drain(..done) {
            if read.cmd.status() == MTLCommandBufferStatus::Error {
                log::warn!("the GPU failed a frame, so the actions of its clicks do not run");
                continue;
            }
            // SAFETY: The GPU has finished writing the buffer, and nothing
            // writes it again. It holds `length` bytes.
            let texels = unsafe {
                std::slice::from_raw_parts(
                    read.texels.contents().cast::<u8>().as_ptr(),
                    read.texels.length(),
                )
            };
            for [red, ..] in texels.as_chunks::<4>().0 {
                if let Some(index) = actions.index(*red) {
                    actions.run(index);
                }
            }
        }
    }

    /// Forgets the clicks of a pause, when the wallpaper resumes. A click
    /// that the GPU read just before the pause would run minutes late.
    fn forget_clicks(&mut self) {
        if let Some(pointer) = &mut self.pointer {
            pointer.forget_clicks();
        }
        self.action_reads.clear();
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
