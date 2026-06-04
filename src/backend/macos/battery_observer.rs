use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_av_foundation::AVPlayer;
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFNumberType, CFRetained, CFRunLoop, CFRunLoopSource,
    CFString, CFType, kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList, IOPSGetPowerSourceDescription,
    IOPSGetProvidingPowerSourceType, IOPSNotificationCreateRunLoopSource,
};

use crate::backend::PauseMode;

// CFSTR(kIOPMBatteryPowerKey). Compare against the literal so we don't have
// to round-trip through CFString equality.
const BATTERY_POWER: &str = "Battery Power";

// IOPSGetPowerSourceDescription dict keys (see IOPSKeys.h).
const CURRENT_CAPACITY: &str = "Current Capacity";
const MAX_CAPACITY: &str = "Max Capacity";

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
    players: Vec<Retained<AVPlayer>>,
    mode: PauseMode,
}

impl BatteryObserver {
    /// Returns `None` only on IOKit setup failure (already logged).
    pub fn install(players: Vec<Retained<AVPlayer>>, mode: PauseMode) -> Option<Self> {
        let mut ctx = Box::new(Context { players, mode });
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

        apply_state(&ctx.players, &ctx.mode);

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
    apply_state(&ctx.players, &ctx.mode);
}

fn apply_state(players: &[Retained<AVPlayer>], mode: &PauseMode) {
    let on_batt = on_battery();
    let pct = battery_percent();

    let should_pause = match mode {
        PauseMode::Never => false,
        PauseMode::OnBattery => on_batt,
        PauseMode::BelowPercent(threshold) => on_batt && pct.is_some_and(|p| p < *threshold),
    };

    if should_pause {
        log::info!("pausing wallpaper (on_battery={on_batt}, charge={pct:?}%)");
        for player in players {
            unsafe { player.pause() };
        }
    } else {
        log::info!("playing wallpaper (on_battery={on_batt}, charge={pct:?}%)");
        for player in players {
            unsafe { player.play() };
        }
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

fn battery_percent() -> Option<u8> {
    let blob = IOPSCopyPowerSourcesInfo()?;
    let list: CFRetained<CFArray> = unsafe { IOPSCopyPowerSourcesList(Some(&blob)) }?;
    let count = list.count();
    for i in 0..count {
        let ps_ptr = unsafe { list.value_at_index(i) };
        if ps_ptr.is_null() {
            continue;
        }
        let ps: &CFType = unsafe { &*(ps_ptr.cast::<CFType>()) };
        let Some(dict) = (unsafe { IOPSGetPowerSourceDescription(Some(&blob), Some(ps)) }) else {
            continue;
        };
        let (Some(cur), Some(max)) = (
            read_i32(&dict, CURRENT_CAPACITY),
            read_i32(&dict, MAX_CAPACITY),
        ) else {
            continue;
        };
        if max <= 0 {
            continue;
        }
        let pct = ((cur as i64 * 100) / max as i64).clamp(0, 100) as u8;
        return Some(pct);
    }
    None
}

fn read_i32(dict: &CFDictionary, key: &'static str) -> Option<i32> {
    let key = CFString::from_static_str(key);
    let key_ptr: *const c_void = (&*key as *const CFString).cast();
    let value_ptr = unsafe { dict.value(key_ptr) };
    if value_ptr.is_null() {
        return None;
    }
    let number: &CFNumber = unsafe { &*(value_ptr.cast::<CFNumber>()) };
    let mut out: i32 = 0;
    let ok = unsafe { number.value(CFNumberType::SInt32Type, (&raw mut out).cast::<c_void>()) };
    ok.then_some(out)
}
