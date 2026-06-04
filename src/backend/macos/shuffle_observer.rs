use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
    sel,
};
use objc2_av_foundation::{AVPlayer, AVPlayerItem, AVPlayerLayer};
use objc2_core_image::CIFilter;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNumber, NSObjectNSDelayedPerforming, NSObjectNSKeyValueCoding,
    NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer, NSURL,
};
use objc2_quartz_core::{
    CABasicAnimation, CAMediaTiming, CAMediaTimingFunction, CATransaction,
    kCAMediaTimingFunctionEaseInEaseOut,
};
use rand::Rng;

const TRANSITION_DURATION: f64 = 0.25;
const WARMUP_DELAY: f64 = 0.15;
const BLUR_RADIUS: f64 = 10.0;

/// State the shuffle observer shares with the battery observer so power
/// changes don't tear a crossfade apart. All access is single-threaded
/// (NSTimer + CFRunLoopSource both fire from the main run loop), so `Rc`
/// and `Cell` are sufficient.
#[derive(Clone, Default)]
pub struct CrossfadeState {
    pub active: Rc<Cell<usize>>,
    pub pause_gate: Rc<Cell<bool>>,
    pub transition_pending: Rc<Cell<bool>>,
}

pub struct ShuffleObserverIvars {
    pool: Vec<PathBuf>,
    layers: [Retained<AVPlayerLayer>; 2],
    players: [Retained<AVPlayer>; 2],
    last_index: Cell<usize>,
    state: CrossfadeState,
    cache_path: Option<PathBuf>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ShuffleObserverIvars]
    pub struct ShuffleObserver;

    impl ShuffleObserver {
        #[unsafe(method(tick:))]
        fn _tick(&self, _timer: &AnyObject) {
            let ivars = self.ivars();
            let Some(mtm) = MainThreadMarker::new() else { return };
            // The battery observer already paused both players. Skipping the
            // tick keeps us from calling play() and undoing that.
            if ivars.state.pause_gate.get() {
                log::debug!("shuffle tick skipped while paused");
                return;
            }
            if ivars.state.transition_pending.get() {
                log::debug!("shuffle tick skipped while previous transition is pending");
                return;
            }

            let last = ivars.last_index.get();
            let next_idx = match ivars.pool.len() {
                0 => return,
                1 => 0,
                2 => 1 - last,
                n => {
                    // Sample from a pool of size n-1, then skip over `last`
                    // to avoid playing the same video twice in a row.
                    let mut i = rand::rng().random_range(0..n - 1);
                    if i >= last {
                        i += 1;
                    }
                    i
                }
            };
            ivars.last_index.set(next_idx);

            let next = ivars.pool[next_idx].clone();
            let abs = next.canonicalize().unwrap_or_else(|_| next.clone());
            let path_ns = NSString::from_str(&abs.to_string_lossy());
            let url = NSURL::fileURLWithPath(&path_ns);
            let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };

            let active = ivars.state.active.get();
            let inactive = 1 - active;

            unsafe {
                // Selector+object form matches the argument using the isEqual:
                // selector. The pausePlayer:/clearFilters: performs were
                // scheduled with non-nil args, so a pass-by-nil cancel
                // wouldn't match them.
                NSObject::cancelPreviousPerformRequestsWithTarget(self);
                ivars.players[inactive].replaceCurrentItemWithPlayerItem(Some(&item));
                ivars.players[inactive].setMuted(true);
                ivars.players[inactive].playImmediatelyAtRate(1.0);
            }
            apply_transition_blur(&ivars.layers[inactive], BLUR_RADIUS);
            ivars.state.transition_pending.set(true);

            if let Some(cache) = &ivars.cache_path {
                let _ = std::fs::write(cache, abs.to_string_lossy().as_bytes());
            }
            log::info!("shuffle -> {}", abs.display());

            // Give AVPlayer a beat to produce the first decoded frame before
            // we crossfade it in. This keeps overlap short without waiting a
            // full preroll window.
            unsafe {
                self.performSelector_withObject_afterDelay(sel!(animate:), None, WARMUP_DELAY);
            }
        }

        #[unsafe(method(animate:))]
        fn _animate(&self, _arg: *mut AnyObject) {
            let ivars = self.ivars();
            let active = ivars.state.active.get();
            let inactive = 1 - active;
            animate_swap(&ivars.layers[active], &ivars.layers[inactive]);
            ivars.state.active.set(inactive);
            ivars.state.transition_pending.set(false);

            unsafe {
                // Clear blur on the incoming (now-active) layer. Pause the
                // outgoing player once the fade has fully covered it.
                self.performSelector_withObject_afterDelay(
                    sel!(clearFilters:),
                    Some(ivars.layers[inactive].as_ref()),
                    TRANSITION_DURATION,
                );
                self.performSelector_withObject_afterDelay(
                    sel!(pausePlayer:),
                    Some(ivars.players[active].as_ref()),
                    TRANSITION_DURATION,
                );
            }

            // If a pause was requested mid-transition, the battery observer
            // deferred to us. Now that the fade has settled, honor it.
            if ivars.state.pause_gate.get() {
                for p in &ivars.players {
                    unsafe { p.pause() };
                }
            }
        }

        #[unsafe(method(pausePlayer:))]
        fn _pause_player(&self, player: &AnyObject) {
            let player: &AVPlayer = unsafe { &*(player as *const AnyObject).cast::<AVPlayer>() };
            unsafe {
                player.pause();
            }
        }

        #[unsafe(method(clearFilters:))]
        fn _clear_filters(&self, layer: &AnyObject) {
            let layer: &AVPlayerLayer =
                unsafe { &*(layer as *const AnyObject).cast::<AVPlayerLayer>() };
            unsafe {
                layer.setFilters(None);
            }
        }
    }
);

impl ShuffleObserver {
    pub fn new(
        pool: Vec<PathBuf>,
        layers: [Retained<AVPlayerLayer>; 2],
        players: [Retained<AVPlayer>; 2],
        cache_path: Option<PathBuf>,
        initial_index: usize,
        state: CrossfadeState,
    ) -> Retained<Self> {
        let ivars = ShuffleObserverIvars {
            pool,
            layers,
            players,
            last_index: Cell::new(initial_index),
            state,
            cache_path,
        };
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

pub fn schedule_timer(observer: &ShuffleObserver, interval_secs: f64) -> Retained<NSTimer> {
    // Scheduled in CommonModes (not just default) so modal panels and event
    // tracking can't stop the wallpaper from advancing.
    unsafe {
        let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            interval_secs,
            observer,
            sel!(tick:),
            None,
            true,
        );
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        timer
    }
}

fn animate_swap(out_layer: &AVPlayerLayer, in_layer: &AVPlayerLayer) {
    // Promote the incoming layer above the outgoing one and keep the
    // outgoing layer fully opaque underneath. Fading both opacities at
    // once dips the composite below 100% (≈0.75 at midpoint) and exposes
    // the desktop behind our wallpaper-level window during the transition.
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    in_layer.setZPosition(1.0);
    out_layer.setZPosition(0.0);
    in_layer.setOpacity(0.0);
    out_layer.setOpacity(1.0);
    CATransaction::commit();

    add_anim(in_layer, "opacity", 0.0, 1.0);

    CATransaction::begin();
    CATransaction::setDisableActions(true);
    in_layer.setOpacity(1.0);
    CATransaction::commit();
}

fn add_anim(layer: &AVPlayerLayer, key_path: &str, from: f64, to: f64) {
    let kp = NSString::from_str(key_path);
    let anim = CABasicAnimation::animationWithKeyPath(Some(&kp));

    let from_num = NSNumber::numberWithDouble(from);
    let to_num = NSNumber::numberWithDouble(to);
    unsafe {
        anim.setFromValue(Some(&from_num));
        anim.setToValue(Some(&to_num));
    }
    anim.setDuration(TRANSITION_DURATION);

    let timing =
        unsafe { CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseInEaseOut) };
    anim.setTimingFunction(Some(&timing));

    layer.addAnimation_forKey(&anim, Some(&kp));
}

fn apply_transition_blur(layer: &AVPlayerLayer, radius: f64) {
    let name = NSString::from_str("CIGaussianBlur");
    let key = NSString::from_str("inputRadius");
    let radius = NSNumber::numberWithDouble(radius);
    let Some(filter) = (unsafe { CIFilter::filterWithName(&name) }) else {
        return;
    };
    unsafe {
        filter.setDefaults();
        filter.setValue_forKey(Some(radius.as_ref()), &key);
        let filters = NSArray::from_retained_slice(&[filter]);
        layer.setFilters(Some(filters.cast_unchecked::<AnyObject>()));
    }
}
