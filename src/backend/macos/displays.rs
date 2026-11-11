use std::ptr::NonNull;

use anyhow::Context;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFRetained, CFUUID};
use objc2_foundation::{MainThreadMarker, NSNumber, NSRect, NSString};

use crate::displays::DisplayInfo;

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    safe fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> Option<NonNull<CFUUID>>;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    safe fn CGDisplayBounds(display: u32) -> NSRect;
}

/// The screen's CGDirectDisplayID, or 0 (kCGNullDirectDisplay) if it has none.
/// Read from `deviceDescription` because `NSScreen.CGDirectDisplayID` needs
/// macOS 26.
pub fn display_id(screen: &NSScreen) -> u32 {
    let key = NSString::from_str("NSScreenNumber");
    screen
        .deviceDescription()
        .objectForKey(&key)
        .and_then(|number| number.downcast::<NSNumber>().ok())
        .map_or(0, |number| number.unsignedIntValue())
}

/// The UUID macOS keeps for the monitor behind the screen. Unlike the name, it
/// doesn't change when another monitor of the same model connects.
fn uuid(screen: &NSScreen) -> Option<String> {
    // The UUID follows the Create Rule. The reference is ours to release.
    CGDisplayCreateUUIDFromDisplayID(display_id(screen))
        .map(|uuid| unsafe { CFRetained::from_raw(uuid) })
        .and_then(|uuid| CFUUID::new_string(None, Some(&uuid)))
        .map(|uuid| uuid.to_string())
}

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    let mtm =
        MainThreadMarker::new().context("macOS display enumeration must run on the main thread")?;
    let screens = NSScreen::screens(mtm);
    let mut out = Vec::new();
    for screen in screens.iter() {
        let frame = screen.frame();
        let backing = screen.backingScaleFactor();
        let width = (frame.size.width * backing).round() as u32;
        let height = (frame.size.height * backing).round() as u32;
        // CoreGraphics puts the origin at the top-left of the main display, like
        // the arrangement in System Settings. AppKit's `frame` starts at the
        // bottom-left.
        let origin = CGDisplayBounds(display_id(&screen)).origin;
        out.push(DisplayInfo {
            id: screen.localizedName().to_string(),
            // The ID is already the name. The UUID and the position tell
            // monitors of the same model apart.
            description: format!(
                "{} at {},{}",
                uuid(&screen).unwrap_or_else(|| "no UUID".to_owned()),
                origin.x,
                origin.y
            ),
            width,
            height,
        });
    }
    Ok(out)
}
