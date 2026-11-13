use objc2::runtime::AnyObject;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::NSObject,
};
use objc2_app_kit::{NSEvent, NSScreen, NSWindow};
use objc2_av_foundation::{AVPlayer, AVPlayerLayer};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSKeyValueChangeKey, NSNotification, NSRect, NSString,
};
use objc2_quartz_core::CADisplayLink;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::rc::Rc;

use super::color_scheme;
use super::displays::{assigned, display_id};
use super::keyboard::Keyboard;
use super::pointer::contains;
use super::procedural::Gpu;
use super::procedural_surface::{ProceduralSource, ProceduralSurface};
use crate::scale::ScaleMode;

pub struct MirrorSurface {
    pub name: String,
    /// Ties the surface to its screen. Two screens can share a name, but not a
    /// display ID.
    pub display_id: u32,
    pub window: Retained<NSWindow>,
    pub layer: SurfaceLayer,
}

/// What a surface draws with.
pub enum SurfaceLayer {
    Video(Retained<AVPlayerLayer>),
    Procedural(ProceduralSurface),
}

/// What a screen shows.
#[derive(Clone)]
pub enum Content {
    Video(Retained<AVPlayer>),
    Procedural {
        /// The index of a procedural source of the observer.
        source: usize,
        /// The keys of the source, if it reads the keyboard.
        keyboard: Option<Rc<RefCell<Keyboard>>>,
    },
}

pub enum AttachPolicy {
    /// Every screen, including ones that appear later, gets a surface that
    /// shows this content.
    Mirror(Content),
    /// Only screens that a key in the map names, by UUID or by name, get a
    /// surface, showing that key's content. Unmatched screens are left alone.
    PerDisplay(HashMap<String, Content>),
}

pub struct ScreenObserverIvars {
    surfaces: RefCell<Vec<MirrorSurface>>,
    policy: AttachPolicy,
    scale: ScaleMode,
    /// Only set if a screen shows a procedural wallpaper.
    gpu: Option<Gpu>,
    sources: RefCell<Vec<ProceduralSource>>,
    power_paused: Cell<bool>,
    /// `iColorScheme`. It changes only when the appearance does.
    color_scheme: Cell<f32>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ScreenObserverIvars]
    pub struct ScreenObserver;

    impl ScreenObserver {
        #[unsafe(method(screensChanged:))]
        fn _screens_changed(&self, _notif: &NSNotification) {
            self.reapply();
        }

        /// The display link of a procedural surface fired.
        #[unsafe(method(frame:))]
        fn _frame(&self, link: &CADisplayLink) {
            self.frame(link);
        }

        /// The appearance of the app changed. It is the only key path that
        /// the observer follows.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn _observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            self.appearance_changed();
        }
    }
);

impl Drop for ScreenObserver {
    fn drop(&mut self) {
        color_scheme::unobserve(self);
    }
}

impl ScreenObserver {
    pub fn new(
        mtm: MainThreadMarker,
        surfaces: Vec<MirrorSurface>,
        policy: AttachPolicy,
        scale: ScaleMode,
        gpu: Option<Gpu>,
        sources: Vec<ProceduralSource>,
    ) -> Retained<Self> {
        let ivars = ScreenObserverIvars {
            surfaces: RefCell::new(surfaces),
            policy,
            scale,
            gpu,
            sources: RefCell::new(sources),
            power_paused: Cell::new(false),
            color_scheme: Cell::new(color_scheme::uniform(mtm)),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        color_scheme::observe(&this);
        this.update_span(mtm);
        for surface in this.ivars().surfaces.borrow_mut().iter_mut() {
            this.start(surface);
        }
        this
    }

    /// Pauses or resumes the procedural wallpapers. Their time stops during
    /// the pause.
    pub fn set_power_paused(&self, paused: bool) {
        // The power source reports every change of the charge.
        if self.ivars().power_paused.replace(paused) == paused {
            return;
        }
        for surface in self.ivars().surfaces.borrow_mut().iter_mut() {
            if let SurfaceLayer::Procedural(p) = &mut surface.layer {
                p.set_paused(paused);
            }
        }
        for source in self.ivars().sources.borrow_mut().iter_mut() {
            source.set_paused(paused);
        }
    }

    fn appearance_changed(&self) {
        let value = color_scheme::uniform(self.mtm());
        if value != self.ivars().color_scheme.get() {
            log::info!("appearance changed: dark={}", value == 1.0);
            self.ivars().color_scheme.set(value);
        }
    }

    /// Starts the display link of a procedural surface.
    fn start(&self, surface: &mut MirrorSurface) {
        let SurfaceLayer::Procedural(p) = &mut surface.layer else {
            return;
        };
        let Some(view) = surface.window.contentView() else {
            log::warn!("display {} has no content view", surface.name);
            return;
        };
        let sources = self.ivars().sources.borrow();
        p.start(
            &view,
            self,
            &sources[p.source],
            self.ivars().power_paused.get(),
        );
    }

    fn build(
        &self,
        mtm: MainThreadMarker,
        screen: &NSScreen,
        content: &Content,
    ) -> anyhow::Result<MirrorSurface> {
        let gpu = self.ivars().gpu.as_ref();
        let mut surface = super::build_surface(mtm, screen, content, self.ivars().scale, gpu)?;
        self.start(&mut surface);
        Ok(surface)
    }

    fn frame(&self, link: &CADisplayLink) {
        let Some(gpu) = &self.ivars().gpu else {
            return;
        };
        let mut surfaces = self.ivars().surfaces.borrow_mut();
        let Some(index) = surfaces
            .iter()
            .position(|s| matches!(&s.layer, SurfaceLayer::Procedural(p) if p.is_driven_by(link)))
        else {
            return;
        };
        let surface = &mut surfaces[index];
        let frame = surface.window.frame();
        let SurfaceLayer::Procedural(p) = &mut surface.layer else {
            return;
        };
        let source_index = p.source;
        let mut sources = self.ivars().sources.borrow_mut();
        let source = &mut sources[source_index];
        let color_scheme = self.ivars().color_scheme.get();
        let clicked_desktop = match p.draw(self.mtm(), gpu, source, frame, link, color_scheme) {
            Ok(clicked) => {
                p.failing = false;
                clicked
            }
            // The screen stops changing until a frame draws. Say so once.
            Err(e) => {
                if !p.failing {
                    log::error!("procedural frame on {}: {e:#}", surface.name);
                    p.failing = true;
                }
                return;
            }
        };
        if !clicked_desktop || source.keyboard.is_none() {
            return;
        }
        // A click on the desktop gives a wallpaper with a keyboard channel
        // the keys, as a click on a canvas does on Shadertoy. With --span,
        // the click can be on any screen of the wallpaper.
        let clicked = if source.spans() {
            let at = NSEvent::mouseLocation();
            surfaces.iter().find(|s| {
                matches!(&s.layer, SurfaceLayer::Procedural(p) if p.source == source_index)
                    && contains(s.window.frame(), at)
            })
        } else {
            surfaces.get(index)
        };
        if let Some(surface) = clicked
            && !surface.window.isKeyWindow()
        {
            surface.window.makeKeyWindow();
        }
    }

    /// Gives the spanned wallpapers the frames of the screens.
    fn update_span(&self, mtm: MainThreadMarker) {
        let screens: Vec<NSRect> = NSScreen::screens(mtm).iter().map(|s| s.frame()).collect();
        for source in self.ivars().sources.borrow_mut().iter_mut() {
            source.set_screens(&screens);
        }
    }

    fn reapply(&self) {
        let mtm = self.mtm();
        let screens = NSScreen::screens(mtm);

        let by_id: HashMap<u32, Retained<NSScreen>> = screens
            .iter()
            .map(|screen| (display_id(&screen), screen))
            .collect();

        // Update surfaces whose screen is still connected.
        for surface in self.ivars().surfaces.borrow_mut().iter_mut() {
            let Some(screen) = by_id.get(&surface.display_id) else {
                continue;
            };
            let frame = screen.frame();
            let backing = screen.backingScaleFactor();
            surface.window.setFrame_display(frame, false);
            // A procedural layer sizes its drawable on the next frame.
            if let SurfaceLayer::Video(layer) = &surface.layer {
                layer.setContentsScale(backing);
            }
            let name = screen.localizedName().to_string();
            if name != surface.name {
                // macOS renames a screen when a monitor of the same model
                // connects or disconnects. It then stops drawing our window on
                // it until it's ordered front again.
                surface.window.orderFront(None);
                surface.name = name;
                // Show what the screen is now assigned, as a restart would.
                // With no assignment it keeps its wallpaper.
                if let AttachPolicy::PerDisplay(map) = &self.ivars().policy
                    && let Some(content) = assigned(screen, map)
                    && !shows(surface, content, mtm)
                {
                    self.switch(mtm, surface, screen, content);
                }
            }
            log::info!(
                "display reconfigured: {} -> {}x{} @ {}x",
                surface.name,
                frame.size.width as u32,
                frame.size.height as u32,
                backing,
            );
        }

        // Drop surfaces whose screen is no longer connected.
        self.ivars().surfaces.borrow_mut().retain(|surface| {
            if by_id.contains_key(&surface.display_id) {
                true
            } else {
                surface.window.orderOut(None);
                log::info!("detached display: {}", surface.name);
                false
            }
        });

        // Attach surfaces for newly-connected screens (subject to attach policy).
        let known: HashSet<u32> = self
            .ivars()
            .surfaces
            .borrow()
            .iter()
            .map(|s| s.display_id)
            .collect();
        for screen in screens.iter() {
            if known.contains(&display_id(&screen)) {
                continue;
            }
            let name = screen.localizedName().to_string();
            let content = match &self.ivars().policy {
                AttachPolicy::Mirror(c) => c,
                AttachPolicy::PerDisplay(map) => match assigned(&screen, map) {
                    Some(c) => c,
                    None => continue,
                },
            };
            match self.build(mtm, &screen, content) {
                Ok(surface) => {
                    log::info!("attached new display: {name}");
                    self.ivars().surfaces.borrow_mut().push(surface);
                }
                Err(e) => log::warn!("failed to attach new display {name}: {e:#}"),
            }
        }

        // The screens can move, appear, or go away.
        self.update_span(mtm);
    }

    /// Makes `surface` show `content`. A video layer only changes its player.
    /// Any other change builds a new surface.
    fn switch(
        &self,
        mtm: MainThreadMarker,
        surface: &mut MirrorSurface,
        screen: &NSScreen,
        content: &Content,
    ) {
        if let (SurfaceLayer::Video(layer), Content::Video(player)) = (&surface.layer, content) {
            unsafe { layer.setPlayer(Some(&**player)) };
            log::info!("switched video on renamed display: {}", surface.name);
            return;
        }
        match self.build(mtm, screen, content) {
            Ok(new) => {
                let old = std::mem::replace(surface, new);
                old.window.orderOut(None);
                log::info!("switched wallpaper on renamed display: {}", surface.name);
            }
            Err(e) => log::warn!(
                "failed to switch wallpaper on renamed display {}: {e:#}",
                surface.name
            ),
        }
    }
}

/// True if `surface` already shows `content`.
fn shows(surface: &MirrorSurface, content: &Content, mtm: MainThreadMarker) -> bool {
    match (&surface.layer, content) {
        (SurfaceLayer::Video(layer), Content::Video(player)) => {
            unsafe { layer.player(mtm) }.as_ref() == Some(player)
        }
        (SurfaceLayer::Procedural(p), Content::Procedural { source, .. }) => p.source == *source,
        _ => false,
    }
}
