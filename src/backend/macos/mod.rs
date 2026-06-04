mod battery_observer;
mod loop_observer;
mod screen_observer;
mod shuffle_observer;

use std::path::{Path, PathBuf};

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
use objc2_foundation::{
    MainThreadMarker, NSActivityOptions, NSNotificationCenter, NSProcessInfo, NSString, NSURL,
};
use objc2_quartz_core::CAAutoresizingMask;

use self::battery_observer::BatteryObserver;
use self::loop_observer::LoopObserver;
use self::screen_observer::ScreenObserver;
use self::shuffle_observer::{CrossfadeState, ShuffleObserver, schedule_timer};
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
        let root_layer = content_view
            .layer()
            .context("content view has no root layer")?;

        let gravity = video_gravity_for(scale);
        let cache_path = cache_current_path();
        let bounds = content_view.bounds();

        let (initial_path, shuffle): (PathBuf, Option<(Vec<PathBuf>, std::time::Duration, usize)>) =
            match source {
                PlaybackSource::Single(p) => (p, None),
                PlaybackSource::Shuffle { pool, interval } => {
                    if pool.is_empty() {
                        return Err(anyhow::anyhow!("shuffle pool is empty"));
                    }
                    use rand::Rng;
                    let first_idx = rand::rng().random_range(0..pool.len());
                    let first = pool[first_idx].clone();
                    (first, Some((pool, interval, first_idx)))
                }
            };

        if let Some(cache) = &cache_path {
            let _ = std::fs::write(cache, initial_path.to_string_lossy().as_bytes());
        }

        let (player_a, layer_a) =
            build_player_layer(mtm, &initial_path, bounds, backing_scale, gravity);
        root_layer.addSublayer(&layer_a);

        let notif_center = NSNotificationCenter::defaultCenter();
        let loop_a = LoopObserver::new(player_a.clone());
        register_loop_observer(&notif_center, &loop_a);

        // For shuffle mode we keep a second layer/player ready behind the
        // active one and crossfade with a Gaussian blur on every tick.
        let mut loop_observers = vec![loop_a];
        let mut players = vec![player_a.clone()];
        let mut layers = vec![layer_a.clone()];
        let mut shuffle_observer: Option<Retained<ShuffleObserver>> = None;
        let mut shuffle_timer = None;
        let mut crossfade_state: Option<CrossfadeState> = None;

        if let Some((pool, interval, first_idx)) = shuffle {
            let (player_b, layer_b) = build_player_layer_empty(mtm, bounds, backing_scale, gravity);
            layer_b.setOpacity(0.0);
            root_layer.addSublayer(&layer_b);

            let loop_b = LoopObserver::new(player_b.clone());
            register_loop_observer(&notif_center, &loop_b);

            let state = CrossfadeState::default();
            let observer = ShuffleObserver::new(
                pool,
                [layer_a.clone(), layer_b.clone()],
                [player_a.clone(), player_b.clone()],
                cache_path.clone(),
                first_idx,
                state.clone(),
            );
            shuffle_timer = Some(schedule_timer(&observer, interval.as_secs_f64()));
            shuffle_observer = Some(observer);
            crossfade_state = Some(state);

            loop_observers.push(loop_b);
            players.push(player_b);
            layers.push(layer_b);
        }

        let screen_observer = ScreenObserver::new(window.clone(), layers.clone());
        unsafe {
            notif_center.addObserver_selector_name_object(
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
            BatteryObserver::install(players.clone(), options.pause, crossfade_state)
        };
        if battery_observer.is_none() {
            for p in &players {
                unsafe { p.play() };
            }
        }

        log::info!(
            "macOS backend ready: {}x{} window at level {}",
            frame.size.width as u32,
            frame.size.height as u32,
            WALLPAPER_LEVEL,
        );

        app.run();
        drop(shuffle_timer);
        drop(shuffle_observer);
        drop(loop_observers);
        drop(screen_observer);
        drop(battery_observer);
        drop(layers);
        drop(players);

        Ok(())
    }
}

fn register_loop_observer(center: &NSNotificationCenter, observer: &LoopObserver) {
    unsafe {
        center.addObserver_selector_name_object(
            observer,
            sel!(itemEnded:),
            Some(AVPlayerItemDidPlayToEndTimeNotification),
            None,
        );
    }
}

fn build_player_layer(
    mtm: MainThreadMarker,
    video_path: &Path,
    bounds: objc2_foundation::NSRect,
    backing_scale: f64,
    gravity: Option<&'static AVLayerVideoGravity>,
) -> (Retained<AVPlayer>, Retained<AVPlayerLayer>) {
    // AVAsset resolves relative paths against the process cwd, which isn't
    // what users expect from `phonto ./video.mp4`.
    let abs = video_path
        .canonicalize()
        .unwrap_or_else(|_| video_path.to_path_buf());
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

    let layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(&player)) };
    if let Some(g) = gravity {
        unsafe { layer.setVideoGravity(g) };
    }
    layer.setFrame(bounds);
    layer.setAutoresizingMask(
        CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
    );
    layer.setContentsScale(backing_scale);

    (player, layer)
}

fn build_player_layer_empty(
    mtm: MainThreadMarker,
    bounds: objc2_foundation::NSRect,
    backing_scale: f64,
    gravity: Option<&'static AVLayerVideoGravity>,
) -> (Retained<AVPlayer>, Retained<AVPlayerLayer>) {
    let player = unsafe { AVPlayer::playerWithPlayerItem(None, mtm) };
    unsafe {
        player.setMuted(true);
        player.setAutomaticallyWaitsToMinimizeStalling(false);
    }

    let layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(&player)) };
    if let Some(g) = gravity {
        unsafe { layer.setVideoGravity(g) };
    }
    layer.setFrame(bounds);
    layer.setAutoresizingMask(
        CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
    );
    layer.setContentsScale(backing_scale);

    (player, layer)
}

fn cache_current_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let cache_dir = std::path::Path::new(&home).join(".cache/phonto");
    std::fs::create_dir_all(&cache_dir).ok()?;
    Some(cache_dir.join("current"))
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
