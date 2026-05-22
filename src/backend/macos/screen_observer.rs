use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_app_kit::{NSScreen, NSWindow};
use objc2_av_foundation::AVPlayerLayer;
use objc2_foundation::{MainThreadMarker, NSNotification, NSRect};

use std::collections::HashMap;

pub struct MirrorSurface {
    pub name: String,
    pub window: Retained<NSWindow>,
    pub layer: Retained<AVPlayerLayer>,
}

pub struct ScreenObserverIvars {
    surfaces: Vec<MirrorSurface>,
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
    pub fn new(surfaces: Vec<MirrorSurface>) -> Retained<Self> {
        let ivars = ScreenObserverIvars { surfaces };
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

        for surface in &self.ivars().surfaces {
            let Some((frame, backing)) = by_name.get(&surface.name).copied() else {
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
    }
}
