use anyhow::Context;
use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSNumber, NSString};

use crate::displays::DisplayInfo;

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

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    let mtm =
        MainThreadMarker::new().context("macOS display enumeration must run on the main thread")?;
    let screens = NSScreen::screens(mtm);
    let mut out = Vec::new();
    for screen in screens.iter() {
        let name = screen.localizedName().to_string();
        let frame = screen.frame();
        let backing = screen.backingScaleFactor();
        let width = (frame.size.width * backing).round() as u32;
        let height = (frame.size.height * backing).round() as u32;
        out.push(DisplayInfo {
            id: name.clone(),
            description: name,
            width,
            height,
        });
    }
    Ok(out)
}
