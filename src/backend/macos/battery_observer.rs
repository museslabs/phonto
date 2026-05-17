use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_av_foundation::AVPlayer;
use objc2_core_foundation::{CFRetained, CFRunLoop, CFRunLoopSource, kCFRunLoopDefaultMode};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSGetProvidingPowerSourceType, IOPSNotificationCreateRunLoopSource,
};

use crate::backend::PauseMode;

// CFSTR(kIOPMBatteryPowerKey). Compare against the literal so we don't have
// to round-trip through CFString equality.
const BATTERY_POWER: &str = "Battery Power";

/// Pauses the player based on power source.
///
/// Owns a heap-allocated context handed to IOKit by raw pointer. The run loop
/// retains the source independently of our `CFRetained` handle, so `Drop`
/// invalidates the source before the boxed `Context` is freed.
pub struct BatteryObserver {
    _ctx: Box<Context>,
    source: CFRetained<CFRunLoopSource>,
}

struct Context {
    player: Retained<AVPlayer>,
    mode: PauseMode,
}

impl BatteryObserver {
    /// Returns `None` only on IOKit setup failure (already logged).
    pub fn install(player: Retained<AVPlayer>, mode: PauseMode) -> Option<Self> {
        let mut ctx = Box::new(Context { player, mode });
        let ctx_ptr: *mut Context = &raw mut *ctx;

        let Some(source) = (unsafe {
            IOPSNotificationCreateRunLoopSource(Some(power_changed), ctx_ptr.cast::<c_void>())
        }) else {
            log::warn!(
                "IOPSNotificationCreateRunLoopSource returned null; battery pause control disabled"
            );
            return None;
        };

        let Some(main) = CFRunLoop::main() else {
            log::warn!("CFRunLoop::main() unavailable; battery pause control disabled");
            return None;
        };
        let Some(mode_ref) = (unsafe { kCFRunLoopDefaultMode }) else {
            log::warn!("kCFRunLoopDefaultMode unavailable; battery pause control disabled");
            return None;
        };
        main.add_source(Some(&source), Some(mode_ref));

        apply_state(&ctx.player, &ctx.mode);

        Some(Self { _ctx: ctx, source })
    }
}

impl Drop for BatteryObserver {
    fn drop(&mut self) {
        // Removes the source from every run loop/mode it was added to so the
        // callback can't fire after `_ctx` is freed.
        self.source.invalidate();
    }
}

unsafe extern "C-unwind" fn power_changed(context: *mut c_void) {
    let ctx = unsafe { &*(context.cast::<Context>()) };
    apply_state(&ctx.player, &ctx.mode);
}

fn apply_state(player: &AVPlayer, mode: &PauseMode) {
    let on_batt = on_battery();

    let should_pause = match mode {
        PauseMode::Never => false,
        PauseMode::OnBattery => on_batt,
    };

    if should_pause {
        log::info!("pausing wallpaper (on_battery={on_batt})");
        unsafe { player.pause() };
    } else {
        log::info!("playing wallpaper (on_battery={on_batt})");
        unsafe { player.play() };
    }
}

fn on_battery() -> bool {
    let Some(snapshot) = IOPSCopyPowerSourcesInfo() else {
        return false;
    };
    let Some(kind) = (unsafe { IOPSGetProvidingPowerSourceType(Some(&snapshot)) }) else {
        return false;
    };
    kind.to_string() == BATTERY_POWER
}
