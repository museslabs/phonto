use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_av_foundation::AVPlayer;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFRunLoopSource, CFString, CFType,
    ConcreteType, kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList, IOPSGetPowerSourceDescription,
    IOPSGetProvidingPowerSourceType, IOPSNotificationCreateRunLoopSource,
};

use super::screen_observer::ScreenObserver;
use crate::backend::PauseMode;

// CFSTR(kIOPMBatteryPowerKey). Compare against the literal so we don't have
// to round-trip through CFString equality.
const BATTERY_POWER: &str = "Battery Power";

// IOPSGetPowerSourceDescription dict keys (see IOPSKeys.h).
const CURRENT_CAPACITY: &str = "Current Capacity";
const MAX_CAPACITY: &str = "Max Capacity";
const IS_CHARGING: &str = "Is Charging";
const TYPE: &str = "Type";
const INTERNAL_BATTERY: &str = "InternalBattery";

/// Pauses the players and the procedural wallpapers based on power source.
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
    /// Pauses the procedural wallpapers.
    screens: Retained<ScreenObserver>,
    mode: PauseMode,
}

impl BatteryObserver {
    /// Returns `None` only on IOKit setup failure (already logged).
    pub fn install(
        players: Vec<Retained<AVPlayer>>,
        screens: Retained<ScreenObserver>,
        mode: PauseMode,
    ) -> Option<Self> {
        let mut ctx = Box::new(Context {
            players,
            screens,
            mode,
        });
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

        apply_state(&ctx);

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
    apply_state(ctx);
}

fn apply_state(ctx: &Context) {
    let Context {
        players,
        screens,
        mode,
    } = ctx;
    let info = IOPSCopyPowerSourcesInfo();
    let on_batt = info.as_deref().is_some_and(on_battery);
    let pct = info.as_deref().and_then(battery_percent);

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
    screens.set_power_paused(should_pause);
}

/// `iBattery`: the charge from 0 to 1, 1 while charging, 1 on AC power, and
/// 1 if the Mac has a battery. Without a battery, the charge is 1.
pub fn uniform() -> [f32; 4] {
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let info = IOPSCopyPowerSourcesInfo();
    let battery = info.as_deref().and_then(internal_battery);
    [
        battery.map_or(1.0, |(charge, _)| charge),
        flag(battery.is_some_and(|(_, charging)| charging)),
        flag(!info.as_deref().is_some_and(on_battery)),
        flag(battery.is_some()),
    ]
}

/// The charge of the internal battery from 0 to 1, and whether it charges.
/// `None` if the Mac has no battery. A UPS does not count, as on Wayland.
fn internal_battery(info: &CFType) -> Option<(f32, bool)> {
    descriptions(info).iter().find_map(|dict| {
        if read::<CFString>(dict, TYPE)?.to_string() != INTERNAL_BATTERY {
            return None;
        }
        let (cur, max) = capacity(dict)?;
        let charge = (cur as f32 / max as f32).clamp(0.0, 1.0);
        let charging = read::<CFBoolean>(dict, IS_CHARGING).is_some_and(|b| b.as_bool());
        Some((charge, charging))
    })
}

fn on_battery(info: &CFType) -> bool {
    unsafe { IOPSGetProvidingPowerSourceType(Some(info)) }
        .is_some_and(|kind| kind.to_string() == BATTERY_POWER)
}

fn battery_percent(info: &CFType) -> Option<u8> {
    descriptions(info).iter().find_map(|dict| {
        let (cur, max) = capacity(dict)?;
        Some(((cur as i64 * 100) / max as i64).clamp(0, 100) as u8)
    })
}

/// The description of each power source in `info`, the snapshot that
/// `IOPSCopyPowerSourcesInfo` returns.
fn descriptions(info: &CFType) -> Vec<CFRetained<CFDictionary>> {
    let Some(list) = (unsafe { IOPSCopyPowerSourcesList(Some(info)) }) else {
        return Vec::new();
    };
    // SAFETY: The list holds a CFTypeRef for each power source.
    let list = unsafe { list.cast_unchecked::<CFType>() };
    list.iter()
        .filter_map(|ps| unsafe { IOPSGetPowerSourceDescription(Some(info), Some(&ps)) })
        .collect()
}

/// The current and the maximum capacity of a power source, if the maximum
/// is above 0.
fn capacity(dict: &CFDictionary) -> Option<(i32, i32)> {
    let cur = read::<CFNumber>(dict, CURRENT_CAPACITY)?.as_i32()?;
    let max = read::<CFNumber>(dict, MAX_CAPACITY)?.as_i32()?;
    (max > 0).then_some((cur, max))
}

/// The value of `key` in `dict`, if it is a `T`.
fn read<'a, T: ConcreteType>(dict: &'a CFDictionary, key: &'static str) -> Option<&'a T> {
    let key = CFString::from_static_str(key);
    let key_ptr: *const c_void = (&*key as *const CFString).cast();
    let value = unsafe { dict.value(key_ptr) }.cast::<CFType>();
    unsafe { value.as_ref() }?.downcast_ref()
}
