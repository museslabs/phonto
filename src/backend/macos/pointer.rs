//! The pointer of a procedural wallpaper, read once per frame.
//!
//! phonto's windows are below Finder's desktop window. They never get mouse
//! events. Instead each frame reads the pointer position, the buttons,
//! and the system's count of left clicks. None of these needs a permission.
//! The count catches a click that is shorter than a frame.
//!
//! The position follows the pointer everywhere on the screen, also under
//! windows, because a translucent window shows the wallpaper. `iPointer.z` is
//! 1 only where the window under the pointer is the desktop. Only a click
//! there presses the button of `iMouse`.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSWindow};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSNumber, NSPoint, NSRect, NSString,
};

use crate::procedural::{BTN_LEFT, Pointer};

/// `kCGEventSourceStateCombinedSessionState`.
const COMBINED_SESSION_STATE: i32 = 0;
/// `kCGEventLeftMouseDown`.
const LEFT_MOUSE_DOWN: u32 = 1;
/// `kCGDesktopIconWindowLevelKey`.
const DESKTOP_ICON_LEVEL_KEY: i32 = 18;
/// `kCGWindowListOptionIncludingWindow`.
const INCLUDING_WINDOW: u32 = 1 << 3;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    safe fn CGEventSourceCounterForEventType(state: i32, event_type: u32) -> u32;
    safe fn CGWindowLevelForKey(key: i32) -> i32;
    /// Returns a CFArray of CFDictionary, which is an NSArray of
    /// NSDictionary. The caller owns it.
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relative_to_window: u32,
    ) -> *mut NSArray<NSDictionary<NSString, AnyObject>>;
}

/// The pointer state of one screen.
pub struct DesktopPointer {
    pointer: Pointer,
    /// The count of left clicks at the last frame.
    clicks: u32,
    /// The window that was last under the pointer, and if it is the desktop.
    last_hit: Option<(isize, bool)>,
}

impl DesktopPointer {
    pub fn new() -> Self {
        Self {
            pointer: Pointer::default(),
            clicks: left_clicks(),
            last_hit: None,
        }
    }

    /// Reads the pointer for a screen with `frame` in global coordinates.
    /// Returns true if the user clicked the desktop on this screen since the
    /// last call.
    pub fn poll(&mut self, mtm: MainThreadMarker, frame: NSRect) -> bool {
        let location = NSEvent::mouseLocation();
        let clicks = left_clicks();
        let clicked = clicks != self.clicks;
        self.clicks = clicks;
        let pressed = NSEvent::pressedMouseButtons() & 1 != 0;

        let mut clicked_desktop = false;
        if contains(frame, location) {
            // Global coordinates start at the bottom left. Pointer wants the
            // top left of the screen.
            let x = location.x - frame.origin.x;
            let y = frame.origin.y + frame.size.height - location.y;
            if self.over_desktop(mtm, location) {
                self.pointer.enter(x, y);
                if clicked {
                    self.pointer.button(BTN_LEFT, true);
                    clicked_desktop = true;
                }
            } else {
                self.pointer.motion(x, y);
                self.pointer.leave();
            }
        } else {
            self.pointer.leave();
        }
        if !pressed {
            self.pointer.button(BTN_LEFT, false);
        }
        clicked_desktop
    }

    /// `(iMouse, iPointer)` for the next frame. `surface` is the size of the
    /// screen in points.
    pub fn uniforms(&mut self, surface: (u32, u32), target: (u32, u32)) -> ([f32; 4], [f32; 4]) {
        let area = [0.0, 0.0, f64::from(surface.0), f64::from(surface.1)];
        self.pointer.uniforms(area, target)
    }

    /// True if the window under `location` is the desktop: Finder's desktop
    /// window, or a window below it such as the system wallpaper.
    fn over_desktop(&mut self, mtm: MainThreadMarker, location: NSPoint) -> bool {
        let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(location, 0, mtm);
        if let Some((last, desktop)) = self.last_hit
            && last == number
        {
            return desktop;
        }
        let desktop = window_level(number)
            .is_none_or(|level| level <= CGWindowLevelForKey(DESKTOP_ICON_LEVEL_KEY));
        self.last_hit = Some((number, desktop));
        desktop
    }
}

fn left_clicks() -> u32 {
    CGEventSourceCounterForEventType(COMBINED_SESSION_STATE, LEFT_MOUSE_DOWN)
}

/// The window level of window `number`, or `None` if there is no such window.
/// The level and the owner are readable without the screen recording
/// permission. Only the titles are not.
fn window_level(number: isize) -> Option<i32> {
    let number = u32::try_from(number).ok().filter(|&n| n != 0)?;
    // SAFETY: The function follows the Create Rule. The array is ours.
    let windows =
        unsafe { Retained::from_raw(CGWindowListCopyWindowInfo(INCLUDING_WINDOW, number)) }?;
    let window = windows.firstObject()?;
    let level = window.objectForKey(&NSString::from_str("kCGWindowLayer"))?;
    Some(level.downcast::<NSNumber>().ok()?.intValue())
}

/// True if `point` is in `rect`, with the top and right edges outside, so
/// that a point on the edge of two screens is in one of them.
fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y > rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}
