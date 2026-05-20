mod battery_observer;
mod loop_observer;
mod screen_observer;

use std::path::Path;

use anyhow::Context;
use objc2::sel;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy,
    NSApplicationDidChangeScreenParametersNotification, NSBackingStoreType, NSColor, NSScreen,
    NSView, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_av_foundation::{
    AVLayerVideoGravity, AVLayerVideoGravityResize, AVLayerVideoGravityResizeAspect,
    AVLayerVideoGravityResizeAspectFill, AVPlayer, AVPlayerItem,
    AVPlayerItemDidPlayToEndTimeNotification, AVPlayerLayer,
};
use objc2_foundation::{
    MainThreadMarker, NSActivityOptions, NSNotificationCenter, NSProcessInfo, NSString, NSURL,
};
use objc2_quartz_core::CAAutoresizingMask;

use self::battery_observer::BatteryObserver;
use self::loop_observer::LoopObserver;
use self::screen_observer::ScreenObserver;
use super::{Backend, PauseMode, PlaybackSource, RunOptions};
use crate::scale::ScaleMode;

// Between kCGDesktopWindowLevel (the system wallpaper layer) and
// kCGDesktopIconWindowLevel (the Finder icons). Sitting below the wallpaper
// gets us geometrically occluded. AppKit marks us occlusionState=hidden and
// CoreAnimation stops compositing our window, so subsequent swaps update the
// model but never reach the screen. Above the wallpaper, occlusion stays
// visible and the desktop icons keep drawing on top.
const WALLPAPER_LEVEL: isize = -2_147_483_604;

pub struct MacosBackend {
    mtm: MainThreadMarker,
}

impl MacosBackend {
    pub fn new() -> anyhow::Result<Self> {
        let mtm = MainThreadMarker::new()
            .context("macOS backend must be constructed on the main thread")?;
        Ok(Self { mtm })
    }
}

impl Backend for MacosBackend {
    fn run(self, source: PlaybackSource, options: RunOptions) -> anyhow::Result<()> {
        let video_path = match source {
            PlaybackSource::Single(p) => p.to_string_lossy().into_owned(),
            PlaybackSource::Shuffle { .. } => {
                anyhow::bail!("--shuffle-every is not yet implemented on macos")
            }
        };

        let mtm = self.mtm;
        let scale = options.scale;

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        // App Nap suspends the run loop (timers stop, AVPlayer stalls) as soon
        // as a fullscreen window covers us. Holding a Background activity for
        // the lifetime of the process keeps the wallpaper ticking when it's
        // occluded. The returned token must outlive the app.run() call.
        let activity_reason = NSString::from_str("phonto wallpaper playback");
        let _activity = NSProcessInfo::processInfo()
            .beginActivityWithOptions_reason(NSActivityOptions::Background, &activity_reason);

        let screen = NSScreen::mainScreen(mtm).context("no main screen")?;
        let frame = screen.frame();
        let backing_scale = screen.backingScaleFactor();

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setLevel(WALLPAPER_LEVEL);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        // `ReadOnly` so screen-capture / screen-sharing can read us. `None` blocks them.
        window.setSharingType(NSWindowSharingType::ReadOnly);
        window.setIgnoresMouseEvents(true);

        let content_view = NSView::initWithFrame(mtm.alloc::<NSView>(), frame);
        content_view.setWantsLayer(true);
        window.setContentView(Some(&content_view));

        // AVAsset resolves relative paths against the process cwd, which isn't
        // what users expect from `phonto ./video.mp4`.
        let abs = Path::new(&video_path)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(&video_path).to_path_buf());
        let path_ns = NSString::from_str(&abs.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path_ns);

        let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
        let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
        unsafe {
            player.setMuted(true);
            // Without this, AVPlayer can drop to WaitingToPlayAtSpecifiedRate when
            // an overlapping window starves the display, and never recovers.
            player.setAutomaticallyWaitsToMinimizeStalling(false);
        }

        let player_layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(&player)) };
        if let Some(gravity) = video_gravity_for(scale) {
            unsafe { player_layer.setVideoGravity(gravity) };
        }
        player_layer.setFrame(content_view.bounds());
        player_layer.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        player_layer.setContentsScale(backing_scale);

        let root_layer = content_view
            .layer()
            .context("content view has no root layer")?;
        root_layer.addSublayer(&player_layer);

        // Loop: AVPlayer posts AVPlayerItemDidPlayToEndTimeNotification when the
        // item reaches its end. The observer seeks back to zero and resumes.
        let loop_observer = LoopObserver::new(player.clone());
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &loop_observer,
                sel!(itemEnded:),
                Some(AVPlayerItemDidPlayToEndTimeNotification),
                None,
            );
        }

        // Re-apply geometry on display reconfiguration.
        let screen_observer = ScreenObserver::new(window.clone(), player_layer.clone());
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &screen_observer,
                sel!(screensChanged:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
        }

        window.makeKeyAndOrderFront(None);

        let battery_observer = if matches!(options.pause, PauseMode::Never) {
            None
        } else {
            BatteryObserver::install(player.clone(), options.pause)
        };
        if battery_observer.is_none() {
            unsafe { player.play() };
        }

        log::info!(
            "macOS backend ready: {}x{} window at level {}",
            frame.size.width as u32,
            frame.size.height as u32,
            WALLPAPER_LEVEL,
        );

        app.run();
        drop(loop_observer);
        drop(screen_observer);
        drop(battery_observer);

        Ok(())
    }
}

fn video_gravity_for(scale: ScaleMode) -> Option<&'static AVLayerVideoGravity> {
    unsafe {
        match scale {
            ScaleMode::Stretch => AVLayerVideoGravityResize,
            ScaleMode::Fit => AVLayerVideoGravityResizeAspect,
            // AVPlayerLayer has no native "center at native size" mode; fall
            // back to aspect-fill for Center until that's wired up properly.
            ScaleMode::Fill | ScaleMode::Center => AVLayerVideoGravityResizeAspectFill,
        }
    }
}
