use anyhow::Context;
use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::displays::DisplayInfo;

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
