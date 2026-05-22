use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_app_kit::{NSScreen, NSWindow};
use objc2_av_foundation::{AVPlayer, AVPlayerLayer};
use objc2_foundation::{MainThreadMarker, NSNotification, NSRect};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::scale::ScaleMode;

pub struct MirrorSurface {
    pub name: String,
    pub window: Retained<NSWindow>,
    pub layer: Retained<AVPlayerLayer>,
}

pub struct ScreenObserverIvars {
    surfaces: RefCell<Vec<MirrorSurface>>,
    player: Retained<AVPlayer>,
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
        player: Retained<AVPlayer>,
        scale: ScaleMode,
    ) -> Retained<Self> {
        let ivars = ScreenObserverIvars {
            surfaces: RefCell::new(surfaces),
            player,
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

        let mut by_name: HashMap<String, (NSRect, f64)> = HashMap::new();
        for screen in screens.iter() {
            let name = screen.localizedName().to_string();
            by_name
                .entry(name)
                .or_insert((screen.frame(), screen.backingScaleFactor()));
        }

        // Update geometry for surfaces whose screen is still connected.
        for surface in self.ivars().surfaces.borrow().iter() {
            let Some(&(frame, backing)) = by_name.get(&surface.name) else {
                continue;
            };
            surface.window.setFrame_display(frame, false);
            surface.layer.setContentsScale(backing);
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
            if by_name.contains_key(&surface.name) {
                true
            } else {
                surface.window.orderOut(None);
                log::info!("detached display: {}", surface.name);
                false
            }
        });

        // Attach surfaces for newly-connected screens.
        let known: HashSet<String> = self
            .ivars()
            .surfaces
            .borrow()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for screen in screens.iter() {
            let name = screen.localizedName().to_string();
            if known.contains(&name) {
                continue;
            }
            match super::build_surface(
                mtm,
                &screen,
                &self.ivars().player,
                self.ivars().scale,
            ) {
                Ok(surface) => {
                    log::info!("attached new display: {name}");
                    self.ivars().surfaces.borrow_mut().push(surface);
                }
                Err(e) => log::warn!("failed to attach new display {name}: {e:#}"),
            }
        }
    }
}
