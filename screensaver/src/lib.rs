use std::cell::RefCell;

use objc2::rc::{Allocated, Retained};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_av_foundation::{
    AVLayerVideoGravityResizeAspectFill, AVPlayer, AVPlayerItem, AVPlayerLayer,
};
use objc2_foundation::{MainThreadMarker, NSRect, NSString, NSURL};
use objc2_quartz_core::CAAutoresizingMask;
use objc2_screen_saver::ScreenSaverView;

#[derive(Default)]
pub struct Ivars {
    player: RefCell<Option<Retained<AVPlayer>>>,
    layer: RefCell<Option<Retained<AVPlayerLayer>>>,
}

define_class!(
    #[unsafe(super(ScreenSaverView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PhontoScreenSaverView"]
    #[ivars = Ivars]
    pub struct PhontoScreenSaverView;

    impl PhontoScreenSaverView {
        #[unsafe(method_id(initWithFrame:isPreview:))]
        fn _init_with_frame(
            this: Allocated<Self>,
            frame: NSRect,
            is_preview: bool,
        ) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars::default());
            let this: Option<Retained<Self>> =
                unsafe { msg_send![super(this), initWithFrame: frame, isPreview: is_preview] };
            // When legacyScreenSaver hosts us as a wallpaper (via the
            // com.apple.wallpaper.choice.screen-saver provider), it instantiates
            // the view but never calls startAnimation — the view is treated as
            // a static surface and you only see the first decoded frame. Kick
            // playback off here so playback starts regardless of whether the
            // host lifecycle is "screen saver" or "wallpaper".
            if let Some(ref view) = this {
                view.install_player();
            }
            this
        }

        #[unsafe(method(startAnimation))]
        fn _start_animation(&self) {
            let _: () = unsafe { msg_send![super(self), startAnimation] };
            self.install_player();
        }

        #[unsafe(method(stopAnimation))]
        fn _stop_animation(&self) {
            self.tear_down_player();
            let _: () = unsafe { msg_send![super(self), stopAnimation] };
        }
    }
);

impl PhontoScreenSaverView {
    fn install_player(&self) {
        // We get called from both initWithFrame: (so wallpaper-hosting works)
        // and startAnimation (so the actual screensaver path works). Bail if
        // we've already set up — otherwise we'd leak a player + decoder.
        if self.ivars().player.borrow().is_some() {
            return;
        }
        let Some(url) = wallpaper_url() else {
            return;
        };

        let mtm = MainThreadMarker::from(self);
        let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
        let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
        unsafe { player.setMuted(true) };

        let layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(&player)) };
        if let Some(gravity) = unsafe { AVLayerVideoGravityResizeAspectFill } {
            unsafe { layer.setVideoGravity(gravity) };
        }
        layer.setFrame(self.bounds());
        layer.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );

        // Layer-hosting view: setLayer BEFORE setWantsLayer makes our layer
        // the view's backing rather than a sublayer of an AppKit-owned one.
        // Avoids the race where self.layer() is None at startAnimation time
        // because the view isn't attached to its host window yet.
        self.setLayer(Some(&layer));
        self.setWantsLayer(true);

        unsafe { player.play() };

        *self.ivars().player.borrow_mut() = Some(player);
        *self.ivars().layer.borrow_mut() = Some(layer);
    }

    fn tear_down_player(&self) {
        // legacyScreenSaver on macOS Sonoma+ keeps old view instances around
        // (stopAnimation isn't called reliably and dealloc may never run).
        // Just dropping the Retaineds isn't enough — the AVPlayerLayer still
        // references the AVPlayer, which still holds the hardware decoder.
        // Fully sever the chain so the next view instance can claim a decoder.
        if let Some(layer) = self.ivars().layer.borrow_mut().take() {
            unsafe { layer.setPlayer(None) };
        }
        if let Some(player) = self.ivars().player.borrow_mut().take() {
            unsafe { player.pause() };
            unsafe { player.replaceCurrentItemWithPlayerItem(None) };
        }
    }
}

// dladdr lets us find the path of the dylib we're running inside without
// going through NSBundle. The dylib lives at
// `…/Phonto.saver/Contents/MacOS/PhontoScreenSaver`, so the wallpaper sits two
// directories up + `Resources/wallpaper.mp4`. This is more robust than
// `bundleWithIdentifier` / `bundleForClass`, both of which proved flaky for
// objc2-defined classes inside legacyScreenSaver's host.
#[repr(C)]
struct DlInfo {
    dli_fname: *const std::ffi::c_char,
    dli_fbase: *mut std::ffi::c_void,
    dli_sname: *const std::ffi::c_char,
    dli_saddr: *mut std::ffi::c_void,
}

unsafe extern "C" {
    fn dladdr(addr: *const std::ffi::c_void, info: *mut DlInfo) -> std::ffi::c_int;
}

fn wallpaper_url() -> Option<Retained<NSURL>> {
    let mut info: DlInfo = unsafe { std::mem::zeroed() };
    let probe = wallpaper_url as *const std::ffi::c_void;
    if unsafe { dladdr(probe, &mut info) } == 0 {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
    let dylib = std::path::PathBuf::from(cstr.to_str().ok()?);
    let video = dylib.parent()?.parent()?.join("Resources/wallpaper.mp4");
    let s = NSString::from_str(video.to_str()?);
    Some(unsafe { NSURL::fileURLWithPath(&s) })
}

// Cocoa resolves NSPrincipalClass via objc_lookUpClass(name) after dlopen.
// objc2 registers classes lazily on first ::class() call, so force it now —
// constructor functions run inside dlopen, before principalClass query.
#[ctor::ctor]
fn register_principal_class() {
    let _ = PhontoScreenSaverView::class();
}
