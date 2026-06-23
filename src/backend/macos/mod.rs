mod battery_observer;
mod displays;
mod loop_observer;
mod screen_observer;

use std::collections::HashMap;
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
use self::screen_observer::{AttachPolicy, MirrorSurface, ScreenObserver};
use super::{Backend, PauseMode, RunOptions};
use crate::displays::DisplayInfo;
use crate::plan::Playback;
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
    fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
        displays::list_displays()
    }

    fn dump(self, _path: String, _at: f64, _out: std::path::PathBuf) -> anyhow::Result<()> {
        anyhow::bail!("dump is not implemented on macos at the moment")
    }

    fn run(self, playback: Playback, options: RunOptions) -> anyhow::Result<()> {
        let mtm = self.mtm;
        let scale = options.scale;

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        let screens = NSScreen::screens(mtm);

        let mut surfaces: Vec<MirrorSurface> = Vec::new();
        let mut players: Vec<Retained<AVPlayer>> = Vec::new();
        let mut loop_observers: Vec<Retained<LoopObserver>> = Vec::new();

        let policy = match playback {
            Playback::Mirror(path) => {
                let (item, player) = make_player(mtm, &path)?;
                loop_observers.push(install_loop_observer(player.clone(), &item));
                players.push(player.clone());

                for screen in screens.iter() {
                    surfaces.push(build_surface(mtm, &screen, &player, scale)?);
                }
                if surfaces.is_empty() {
                    anyhow::bail!("no screens detected");
                }

                AttachPolicy::Mirror(player)
            }
            Playback::PerDisplay(assignments) => {
                let mut by_id: HashMap<String, Retained<AVPlayer>> = HashMap::new();
                for a in &assignments {
                    let (item, player) = make_player(mtm, &a.path)?;
                    loop_observers.push(install_loop_observer(player.clone(), &item));
                    players.push(player.clone());
                    by_id.insert(a.native_id.clone(), player);
                }

                for screen in screens.iter() {
                    let name = screen.localizedName().to_string();
                    if let Some(player) = by_id.get(&name) {
                        surfaces.push(build_surface(mtm, &screen, player, scale)?);
                    } else {
                        log::info!("display {name:?} not in assignments; leaving alone");
                    }
                }

                // It's fine if no surfaces attach now — configured displays may
                // appear later via hot-plug.
                AttachPolicy::PerDisplay(by_id)
            }
        };

        // Re-apply geometry on display reconfiguration, and attach new
        // surfaces when a fresh display appears (respecting the policy).
        let screen_observer = ScreenObserver::new(surfaces, policy, scale);
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
            BatteryObserver::install(players.clone(), options.pause)
        };
        if battery_observer.is_none() {
            for player in &players {
                unsafe { player.play() };
            }
        }

        log::info!("macOS backend ready at level {}", WALLPAPER_LEVEL);

        app.run();
        drop(loop_observers);
        drop(screen_observer);
        drop(battery_observer);

        Ok(())
    }
}

fn make_player(
    mtm: MainThreadMarker,
    video_path: &str,
) -> anyhow::Result<(Retained<AVPlayerItem>, Retained<AVPlayer>)> {
    let is_url = crate::config::is_url(video_path);

    let url = if is_url {
        let s = NSString::from_str(video_path);
        NSURL::URLWithString(&s).context("invalid URL")?
    } else {
        // AVAsset resolves relative paths against the process cwd, which isn't
        // what users expect from `phonto ./video.mp4`.
        let abs = Path::new(video_path)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(video_path).to_path_buf());
        let path_ns = NSString::from_str(&abs.to_string_lossy());
        NSURL::fileURLWithPath(&path_ns)
    };

    let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    unsafe {
        player.setMuted(true);
    }
    Ok((item, player))
}

fn install_loop_observer(
    player: Retained<AVPlayer>,
    item: &AVPlayerItem,
) -> Retained<LoopObserver> {
    let observer = LoopObserver::new(player);
    // Filter by `item` so multi-player setups don't cross-seek each other.
    let item_obj: &objc2::runtime::AnyObject = item;
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &observer,
            sel!(itemEnded:),
            Some(AVPlayerItemDidPlayToEndTimeNotification),
            Some(item_obj),
        );
    }
    observer
}

pub(super) fn build_surface(
    mtm: MainThreadMarker,
    screen: &NSScreen,
    player: &AVPlayer,
    scale: ScaleMode,
) -> anyhow::Result<MirrorSurface> {
    let name = screen.localizedName().to_string();
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

    let player_layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(Some(player)) };
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

    window.makeKeyAndOrderFront(None);

    log::info!(
        "surface ready on {}: {}x{} @ {}x",
        name,
        frame.size.width as u32,
        frame.size.height as u32,
        backing_scale,
    );

    Ok(MirrorSurface {
        name,
        window,
        layer: player_layer,
    })
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
