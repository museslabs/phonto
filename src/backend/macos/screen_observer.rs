use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_app_kit::{NSScreen, NSWindow};
use objc2_av_foundation::{AVPlayer, AVPlayerLayer};
use objc2_foundation::{MainThreadMarker, NSNotification};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::displays::display_id;
use crate::scale::ScaleMode;

pub struct MirrorSurface {
    pub name: String,
    /// Ties the surface to its screen. Two screens can share a name, but not a
    /// display ID.
    pub display_id: u32,
    pub window: Retained<NSWindow>,
    pub layer: Retained<AVPlayerLayer>,
}

pub enum AttachPolicy {
    /// Every screen, including ones that appear later, gets a surface sharing
    /// this player.
    Mirror(Retained<AVPlayer>),
    /// Only screens whose localizedName matches a key in the map get a surface,
    /// attached to that key's player. Unmatched screens are left alone.
    PerDisplay(HashMap<String, Retained<AVPlayer>>),
}

pub struct ScreenObserverIvars {
    surfaces: RefCell<Vec<MirrorSurface>>,
    policy: AttachPolicy,
    scale: ScaleMode,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ScreenObserverIvars]
    pub struct ScreenObserver;

    impl ScreenObserver {
        #[unsafe(method(screensChanged:))]
        fn _screens_changed(&self, _notif: &NSNotification) {
            self.reapply();
        }
    }
);

impl ScreenObserver {
    pub fn new(
        surfaces: Vec<MirrorSurface>,
        policy: AttachPolicy,
        scale: ScaleMode,
    ) -> Retained<Self> {
        let ivars = ScreenObserverIvars {
            surfaces: RefCell::new(surfaces),
            policy,
            scale,
        };
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn reapply(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
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
            surface.layer.setContentsScale(backing);
            let name = screen.localizedName().to_string();
            if name != surface.name {
                // macOS renames a screen when a monitor of the same model
                // connects or disconnects. It then stops drawing our window on
                // it until it's ordered front again.
                surface.window.makeKeyAndOrderFront(None);
                surface.name = name;
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
            let player = match &self.ivars().policy {
                AttachPolicy::Mirror(p) => p,
                AttachPolicy::PerDisplay(map) => match map.get(&name) {
                    Some(p) => p,
                    None => continue,
                },
            };
            match super::build_surface(mtm, &screen, player, self.ivars().scale) {
                Ok(surface) => {
                    log::info!("attached new display: {name}");
                    self.ivars().surfaces.borrow_mut().push(surface);
                }
                Err(e) => log::warn!("failed to attach new display {name}: {e:#}"),
            }
        }
    }
}
