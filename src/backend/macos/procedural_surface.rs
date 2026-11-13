//! Procedural wallpapers on screen: one [`ProceduralSource`] for each
//! wallpaper, and one [`ProceduralSurface`] for each screen that shows it.
//!
//! Each surface has a display link that fires at the refresh of its screen.
//! The source keeps the clock. All surfaces of a source draw the same moment,
//! so mirrored screens stay in sync.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::sel;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_foundation::{NSRect, NSRunLoop, NSRunLoopCommonModes};
use objc2_metal::{MTLCommandBuffer, MTLDrawable};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAMetalDrawable, CAMetalLayer};

use super::procedural::{self, Gpu, Program, SCREEN_FORMAT, Target};
use crate::config::{InputCommand, Location};
use crate::procedural::{
    Clock, CommandInputs, FrameInputs, FrameLimiter, Pack, TextureWatcher, local_date, location,
};

/// A procedural wallpaper, compiled, with its clock.
pub struct ProceduralSource {
    program: Program,
    fps: Option<f32>,
    clock: Clock,
    limiter: FrameLimiter,
    /// The inputs of the current frame that all screens share. Each surface
    /// adds its display.
    inputs: FrameInputs,
    location: Option<Location>,
    /// The values of the inputs that the pack reads.
    commands: CommandInputs,
    watcher: Option<TextureWatcher>,
    /// Counts the frames of the source. A surface draws when this changes.
    frame: u64,
}

impl ProceduralSource {
    pub fn load(
        gpu: &Gpu,
        path: &str,
        location: Option<Location>,
        commands: &[InputCommand],
    ) -> anyhow::Result<Self> {
        let pack = Pack::load(Path::new(path))?;
        log::info!(
            "procedural wallpaper `{}`: {} buffer pass(es), {} fps cap, render scale {}",
            pack.name,
            pack.buffers.len(),
            pack.fps.map_or_else(|| "no".to_string(), |f| f.to_string()),
            pack.render_scale
        );
        if pack.audio {
            log::warn!(
                "`{}` reads the system audio, which phonto does not read on macOS yet. It draws without audio.",
                pack.name
            );
        }
        Ok(Self {
            program: Program::compile(gpu, &pack)?,
            fps: pack.fps,
            clock: Clock::new(pack.fps),
            limiter: FrameLimiter::new(pack.fps),
            inputs: FrameInputs::default(),
            location,
            commands: CommandInputs::start(&pack.inputs, commands),
            watcher: TextureWatcher::start(&pack.textures),
            frame: 0,
        })
    }

    /// Starts the next frame if the frame-rate cap allows it. `slack` lets a
    /// display link that fires a little early still count. A cap that divides
    /// the refresh rate then keeps an even cadence.
    fn advance(&mut self, gpu: &Gpu, now: Instant, slack: Duration) -> anyhow::Result<()> {
        if self.limiter.next_frame().is_some_and(|t| t > now + slack) {
            return Ok(());
        }
        // Upload the new images of watched files.
        for (index, texture) in self.watcher.iter().flat_map(TextureWatcher::take) {
            if let Err(err) = self.program.replace_texture(gpu, index, &texture) {
                log::warn!("{err:#}");
            }
        }
        let tick = self.clock.tick(now);
        let wall = SystemTime::now();
        self.inputs = FrameInputs {
            time: tick.time,
            time_delta: tick.delta,
            frame_rate: tick.frame_rate,
            date: local_date(wall),
            location: location(wall, self.location),
            inputs: self.commands.values(),
            ..FrameInputs::default()
        };
        self.limiter.rendered(now);
        self.frame += 1;
        Ok(())
    }

    /// Stops running the input commands while the wallpaper is paused.
    pub fn set_paused(&self, paused: bool) {
        self.commands.set_paused(paused);
    }
}

/// The layer of one screen that shows a procedural wallpaper.
pub struct ProceduralSurface {
    pub layer: Retained<CAMetalLayer>,
    /// The index of the source in the observer's list.
    pub source: usize,
    link: Option<Retained<CADisplayLink>>,
    target: Option<Target>,
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
            frame: 0,
        })
    }

    /// Starts a display link on `view` that calls `frame:` on `target`.
    pub fn start(
        &mut self,
        view: &NSView,
        target: &AnyObject,
        source: &ProceduralSource,
        paused: bool,
    ) {
        if let Some(old) = self.link.take() {
            old.invalidate();
        }
        let link = unsafe { view.displayLinkWithTarget_selector(target, sel!(frame:)) };
        if let Some(fps) = source.fps {
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
    /// frame of the screen in global coordinates.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        source: &mut ProceduralSource,
        frame: NSRect,
        link: &CADisplayLink,
    ) -> anyhow::Result<()> {
        let slack = Duration::from_secs_f64(link.duration() / 2.0);
        source.advance(gpu, Instant::now(), slack)?;
        if self.frame == source.frame {
            return Ok(());
        }
        self.frame = source.frame;

        let surface = (
            (frame.size.width.round() as u32).max(1),
            (frame.size.height.round() as u32).max(1),
        );
        let dims = source.program.target_dims(surface);
        if self.target.as_ref().map(Target::dims) != Some(dims) {
            self.target = Some(source.program.create_target(gpu, dims, false)?);
            // The layer scales the drawable to the screen.
            self.layer
                .setDrawableSize(CGSize::new(f64::from(dims.0), f64::from(dims.1)));
        }
        let target = self.target.as_mut().expect("target created above");

        let (displays, display_count) = procedural::one_display(dims);
        let inputs = FrameInputs {
            displays,
            display_count,
            ..source.inputs
        };

        let cmd = gpu.command_buffer()?;
        source.program.encode(&cmd, target, &inputs);
        // Take the drawable last. It can wait until the screen frees one.
        if let Some(drawable) = self.layer.nextDrawable() {
            gpu.encode_present(&cmd, target, &drawable.texture());
            cmd.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(&*drawable));
        }
        cmd.commit();
        Ok(())
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
