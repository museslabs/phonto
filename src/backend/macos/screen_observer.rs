use objc2::runtime::AnyObject;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::NSObject, sel,
};
use objc2_app_kit::{
    NSEvent, NSScreen, NSWindow, NSWindowDidChangeOcclusionStateNotification, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceScreensDidSleepNotification, NSWorkspaceScreensDidWakeNotification,
    NSWorkspaceSessionDidBecomeActiveNotification, NSWorkspaceSessionDidResignActiveNotification,
};
use objc2_av_foundation::{AVPlayer, AVPlayerLayer};
use objc2_core_graphics::CGDisplayIsAsleep;
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSKeyValueChangeKey, NSNotification, NSNotificationCenter,
    NSRect, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
};
use objc2_quartz_core::CADisplayLink;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::color_scheme;
use super::cover::{self, Cover};
use super::displays::{assigned, display_id};
use super::keyboard::Keyboard;
use super::pointer::contains;
use super::procedural::Gpu;
use super::procedural_surface::{ProceduralSource, ProceduralSurface};
use crate::plan::PauseBehindPlan;
use crate::scale::ScaleMode;

pub struct MirrorSurface {
    pub name: String,
    /// Ties the surface to its screen. Two screens can share a name, but not a
    /// display ID.
    pub display_id: u32,
    pub window: Retained<NSWindow>,
    pub layer: SurfaceLayer,
    pub cover: Cover,
}

/// What a surface draws with.
pub enum SurfaceLayer {
    Video(Retained<AVPlayerLayer>),
    Procedural(Box<ProceduralSurface>),
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
    /// Each video player, and whether it plays.
    players: Vec<(Retained<AVPlayer>, Cell<bool>)>,
    pause_behind: PauseBehindPlan,
    on_battery: Cell<bool>,
    /// True if `--pause-on-battery` or `--pause-below` pauses everything.
    power_paused: Cell<bool>,
    /// True while another user's session is in front.
    switched_out: Cell<bool>,
    /// The next read of the cover.
    timer: RefCell<Option<Retained<NSTimer>>>,
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

        #[unsafe(method(poll:))]
        fn _poll(&self, _timer: &NSTimer) {
            self.update_cover();
        }

        /// A window of ours changed its occlusion, another app or Space
        /// came to the front, or the screens slept or woke.
        #[unsafe(method(coverChanged:))]
        fn _cover_changed(&self, _notif: &NSNotification) {
            self.update_cover();
        }

        #[unsafe(method(sessionDidResignActive:))]
        fn _session_did_resign_active(&self, _notif: &NSNotification) {
            self.ivars().switched_out.set(true);
            self.update_cover();
        }

        #[unsafe(method(sessionDidBecomeActive:))]
        fn _session_did_become_active(&self, _notif: &NSNotification) {
            self.ivars().switched_out.set(false);
            self.update_cover();
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
        pause_behind: PauseBehindPlan,
    ) -> Retained<Self> {
        let contents = match &policy {
            AttachPolicy::Mirror(content) => vec![content],
            AttachPolicy::PerDisplay(map) => map.values().collect(),
        };
        let players = contents
            .into_iter()
            .filter_map(|content| match content {
                Content::Video(player) => Some((player.clone(), Cell::new(false))),
                Content::Procedural { .. } => None,
            })
            .collect();
        let ivars = ScreenObserverIvars {
            surfaces: RefCell::new(surfaces),
            policy,
            scale,
            gpu,
            sources: RefCell::new(sources),
            players,
            pause_behind,
            on_battery: Cell::new(false),
            power_paused: Cell::new(false),
            switched_out: Cell::new(false),
            timer: RefCell::new(None),
            color_scheme: Cell::new(color_scheme::uniform(mtm)),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        color_scheme::observe(&this);
        this.observe_cover();
        this.update_span(mtm);
        for surface in this.ivars().surfaces.borrow_mut().iter_mut() {
            this.start(surface);
        }
        this
    }

    /// Reads the cover again on the events that can change it.
    fn observe_cover(&self) {
        let center = NSNotificationCenter::defaultCenter();
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: Each selector takes the notification.
        unsafe {
            // Fires for each window of ours, the sentinels too.
            center.addObserver_selector_name_object(
                self,
                sel!(coverChanged:),
                Some(NSWindowDidChangeOcclusionStateNotification),
                None,
            );
            for (selector, name) in [
                (
                    sel!(coverChanged:),
                    NSWorkspaceDidActivateApplicationNotification,
                ),
                (
                    sel!(coverChanged:),
                    NSWorkspaceActiveSpaceDidChangeNotification,
                ),
                (sel!(coverChanged:), NSWorkspaceScreensDidSleepNotification),
                (sel!(coverChanged:), NSWorkspaceScreensDidWakeNotification),
                (
                    sel!(sessionDidResignActive:),
                    NSWorkspaceSessionDidResignActiveNotification,
                ),
                (
                    sel!(sessionDidBecomeActive:),
                    NSWorkspaceSessionDidBecomeActiveNotification,
                ),
            ] {
                workspace.addObserver_selector_name_object(self, selector, Some(name), None);
            }
        }
    }

    /// Takes the power source, and whether `--pause-on-battery` or
    /// `--pause-below` pauses everything. Time stops during the pause.
    pub fn set_power(&self, on_battery: bool, paused: bool) {
        // The power source reports every change of the charge.
        let switched = self.ivars().on_battery.replace(on_battery) != on_battery;
        let toggled = self.ivars().power_paused.replace(paused) != paused;
        if switched || toggled {
            self.update_cover();
        }
    }

    /// Reads which screens are covered, runs and pauses what that changes,
    /// and plans the next read. Without power pausing everything, `any`
    /// reads its screen on a timer, and a screen that has just been covered
    /// is read again until it pauses.
    pub fn update_cover(&self) {
        let ivars = self.ivars();
        let switched_out = ivars.switched_out.get();
        let on_battery = ivars.on_battery.get();
        let now = Instant::now();
        let screens = NSScreen::screens(self.mtm());
        let windows = OnceCell::new();
        let next = ivars
            .surfaces
            .borrow_mut()
            .iter_mut()
            .filter_map(|surface| {
                let screen = screens
                    .iter()
                    .find(|screen| display_id(screen) == surface.display_id)?;
                let hidden = switched_out
                    || CGDisplayIsAsleep(surface.display_id)
                    || !cover::shows(&surface.window);
                surface
                    .cover
                    .read(&screen, hidden, on_battery, &windows, now)
            })
            .min();
        self.apply();
        self.schedule(next.filter(|_| !ivars.power_paused.get()));
    }

    /// Runs each surface that power and its cover let run, and each source
    /// and player that a running surface shows. Pauses the rest. Only a
    /// change reaches a display link, a source or a player.
    fn apply(&self) {
        let ivars = self.ivars();
        let power_paused = ivars.power_paused.get();
        let mut sources = ivars.sources.borrow_mut();
        let mut sources_run = vec![false; sources.len()];
        let mut players_run = Vec::new();
        for surface in ivars.surfaces.borrow_mut().iter_mut() {
            let runs = !power_paused && !surface.cover.covered();
            match &mut surface.layer {
                SurfaceLayer::Procedural(p) => {
                    p.set_paused(!runs);
                    sources_run[p.source] |= runs;
                }
                SurfaceLayer::Video(layer) => {
                    if runs && let Some(player) = unsafe { layer.player(self.mtm()) } {
                        players_run.push(player);
                    }
                }
            }
        }
        for (source, runs) in sources.iter_mut().zip(&sources_run) {
            source.set_paused(!runs);
        }
        // The sources share one capture of the audio.
        let audio_runs = sources
            .iter()
            .zip(&sources_run)
            .any(|(source, runs)| *runs && source.audio().is_some());
        if let Some(audio) = sources.iter().find_map(ProceduralSource::audio) {
            audio.set_paused(!audio_runs);
        }
        for (player, playing) in &ivars.players {
            let runs = players_run.contains(player);
            if playing.replace(runs) != runs {
                if runs {
                    unsafe { player.play() };
                } else {
                    unsafe { player.pause() };
                }
            }
        }
    }

    /// Reads the cover again `after` this long, or not until an event.
    fn schedule(&self, after: Option<Duration>) {
        if let Some(timer) = self.ivars().timer.take() {
            timer.invalidate();
        }
        let Some(after) = after else {
            return;
        };
        let seconds = after.as_secs_f64();
        // SAFETY: `ScreenObserver` answers `poll:`, which takes the timer.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                seconds,
                self,
                sel!(poll:),
                None,
                false,
            )
        };
        // The slack lets macOS fire the timer with others.
        timer.setTolerance(seconds / 10.0);
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        self.ivars().timer.replace(Some(timer));
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
        let pause_behind = cover::pause_behind(screen, &self.ivars().pause_behind);
        let mut surface =
            super::build_surface(mtm, screen, content, self.ivars().scale, gpu, pause_behind)?;
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
            // The sentinel follows the visible frame, and a new name can
            // match another entry.
            let pause_behind = cover::pause_behind(screen, &self.ivars().pause_behind);
            surface.cover.place(mtm, screen, pause_behind);
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
        self.update_cover();
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
