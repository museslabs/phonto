mod battery_observer;
mod loop_observer;
mod screen_observer;

use std::path::Path;

use anyhow::Context;
use objc2::rc::Retained;
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
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSString, NSURL};
use objc2_quartz_core::CAAutoresizingMask;

use self::battery_observer::BatteryObserver;
use self::loop_observer::LoopObserver;
use self::screen_observer::ScreenObserver;
use super::{Backend, PauseMode, RunOptions};
use crate::scale::ScaleMode;

// One below kCGDesktopWindowLevel so a static system wallpaper sits on top of us.
const WALLPAPER_LEVEL: isize = -2_147_483_624;

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
    fn run(self, video_path: String, options: RunOptions) -> anyhow::Result<()> {
        let mtm = self.mtm;
        let scale = options.scale;

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        let abs = Path::new(&video_path)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(&video_path).to_path_buf());
        let path_ns = NSString::from_str(&abs.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path_ns);

        let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
        let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
        unsafe {
            player.setMuted(true);
        }

        let screens = NSScreen::screens(mtm);
        let screen_count = screens.count();
        if screen_count == 0 {
            return Err(anyhow::anyhow!("no screens found"));
        }

        let gravity = video_gravity_for(scale);
        let mut windows = Vec::with_capacity(screen_count);
        let mut player_layers = Vec::with_capacity(screen_count);
        for i in 0..screen_count {
            let screen = screens.objectAtIndex(i);
            let (window, layer) = build_wallpaper_window(mtm, &screen, &player, gravity)?;
            windows.push(window);
            player_layers.push(layer);
        }

        let loop_observer = LoopObserver::new(player.clone());
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &loop_observer,
                sel!(itemEnded:),
                Some(AVPlayerItemDidPlayToEndTimeNotification),
                None,
            );
        }

        let screen_observer = ScreenObserver::new(windows, player_layers);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &screen_observer,
                sel!(screensChanged:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
        }

        let battery_observer = if matches!(options.pause, PauseMode::Never) {
            None
        } else {
            BatteryObserver::install(player.clone(), options.pause)
        };
        if battery_observer.is_none() {
            unsafe { player.play() };
        }

        log::info!(
            "macOS backend ready: {} screen(s) at level {}",
            screen_count,
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

fn build_wallpaper_window(
    mtm: MainThreadMarker,
    screen: &NSScreen,
    player: &AVPlayer,
    gravity: Option<&'static AVLayerVideoGravity>,
) -> anyhow::Result<(Retained<NSWindow>, Retained<AVPlayerLayer>)> {
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
    window.setSharingType(NSWindowSharingType::ReadOnly);
    window.setIgnoresMouseEvents(true);

    let content_view = NSView::initWithFrame(mtm.alloc::<NSView>(), frame);
    content_view.setWantsLayer(true);
    window.setContentView(Some(&content_view));

    let layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(player)) };
    if let Some(gravity) = gravity {
        unsafe { layer.setVideoGravity(gravity) };
    }
    layer.setFrame(content_view.bounds());
    layer.setAutoresizingMask(
        CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
    );
    layer.setContentsScale(backing_scale);

    let root_layer = content_view
        .layer()
        .context("content view has no root layer")?;
    root_layer.addSublayer(&layer);

    window.makeKeyAndOrderFront(None);
    Ok((window, layer))
}
