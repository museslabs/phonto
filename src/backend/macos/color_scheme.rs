//! `iColorScheme` on macOS: whether the desktop prefers dark colors.

use objc2::runtime::NSObject;
use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration,
    ns_string,
};

/// True if System Settings > Appearance is dark now. The appearance of the
/// app follows it, and resolves Auto.
fn is_dark(mtm: MainThreadMarker) -> bool {
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    // A high-contrast appearance has a name of its own. Its best match is
    // still one of these two.
    NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark]))
        .is_some_and(|name| &*name == dark)
}

/// `iColorScheme`: 1 if the appearance is dark, else 0.
pub fn uniform(mtm: MainThreadMarker) -> f32 {
    if is_dark(mtm) { 1.0 } else { 0.0 }
}

/// Calls `observeValueForKeyPath:ofObject:change:context:` on `observer`
/// when the appearance changes. Call [`unobserve`] before `observer` is
/// freed.
pub fn observe(mtm: MainThreadMarker, observer: &NSObject) {
    unsafe {
        NSApplication::sharedApplication(mtm).addObserver_forKeyPath_options_context(
            observer,
            ns_string!("effectiveAppearance"),
            NSKeyValueObservingOptions::empty(),
            std::ptr::null_mut(),
        );
    }
}

pub fn unobserve(mtm: MainThreadMarker, observer: &NSObject) {
    unsafe {
        NSApplication::sharedApplication(mtm)
            .removeObserver_forKeyPath(observer, ns_string!("effectiveAppearance"));
    }
}
