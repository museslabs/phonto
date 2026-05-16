use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::NSObject,
};
use objc2_app_kit::{NSScreen, NSWindow};
use objc2_av_foundation::AVPlayerLayer;
use objc2_foundation::{MainThreadMarker, NSNotification};

pub struct ScreenObserverIvars {
    window: Retained<NSWindow>,
    layer: Retained<AVPlayerLayer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ScreenObserverIvars]
    pub struct ScreenObserver;

    impl ScreenObserver {
        #[unsafe(method(screensChanged:))]
        fn _screens_changed(&self, _notif: &NSNotification) {
            self.apply_current_screen();
        }
    }
);

impl ScreenObserver {
    pub fn new(window: Retained<NSWindow>, layer: Retained<AVPlayerLayer>) -> Retained<Self> {
        let ivars = ScreenObserverIvars { window, layer };
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn apply_current_screen(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            return;
        };

        let frame = screen.frame();
        let backing_scale = screen.backingScaleFactor();

        let ivars = self.ivars();
        ivars.window.setFrame_display(frame, false);
        ivars.layer.setContentsScale(backing_scale);

        log::info!(
            "display reconfigured: {}x{} @ {}x backing",
            frame.size.width as u32,
            frame.size.height as u32,
            backing_scale,
        );
    }
}
