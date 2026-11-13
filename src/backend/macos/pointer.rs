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

use objc2_app_kit::{NSEvent, NSWindow};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
use objc2_core_graphics::{
    CGEventSource, CGEventSourceStateID, CGEventType, CGWindowLevelForKey, CGWindowLevelKey,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowLayer,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};

use crate::procedural::{BTN_LEFT, Pointer};

/// The pointer state of one screen.
pub struct DesktopPointer {
    pointer: Pointer,
    /// The count of left clicks at the last frame.
    clicks: u32,
    /// True from a click on the desktop until its button comes up.
    desktop_click: bool,
    /// The window that was last under the pointer, and if it is the desktop.
    last_hit: Option<(isize, bool)>,
}

impl DesktopPointer {
    pub fn new() -> Self {
        Self {
            pointer: Pointer::default(),
            clicks: left_clicks(),
            desktop_click: false,
            last_hit: None,
        }
    }

    /// Reads the pointer for a screen with `frame` in global coordinates.
    /// Returns true on the first call after a click on the desktop of this
    /// screen ends. Finder takes the key window during the click, at the
    /// latest when the button comes up. The wallpaper takes it after that.
    pub fn poll(&mut self, mtm: MainThreadMarker, frame: NSRect) -> bool {
        let location = NSEvent::mouseLocation();
        let clicks = left_clicks();
        let clicked = clicks != self.clicks;
        self.clicks = clicks;
        // A click elsewhere ends a click on the desktop that waits for its
        // button.
        if clicked {
            self.desktop_click = false;
        }
        let pressed = NSEvent::pressedMouseButtons() & 1 != 0;

        if contains(frame, location) {
            // Global coordinates start at the bottom left. Pointer wants the
            // top left of the screen.
            let x = location.x - frame.origin.x;
            let y = frame.origin.y + frame.size.height - location.y;
            if self.over_desktop(mtm, location) {
                self.pointer.enter(x, y);
                if clicked {
                    self.pointer.button(BTN_LEFT, true);
                    self.desktop_click = true;
                }
            } else {
                self.pointer.motion(x, y);
                self.pointer.leave();
            }
        } else {
            self.pointer.leave();
        }
        if pressed {
            return false;
        }
        self.pointer.button(BTN_LEFT, false);
        std::mem::take(&mut self.desktop_click)
    }

    /// Forgets the clicks since the last call to `poll`, for a wallpaper that
    /// resumes from a pause. A click from the pause would land on the first
    /// frame, minutes late.
    pub fn forget_clicks(&mut self) {
        self.clicks = left_clicks();
        self.desktop_click = false;
    }

    /// `(iMouse, iPointer)` for the next frame. `surface` is the size of the
    /// screen in points.
    pub fn uniforms(&mut self, surface: (u32, u32), target: (u32, u32)) -> ([f32; 4], [f32; 4]) {
        let area = [0.0, 0.0, f64::from(surface.0), f64::from(surface.1)];
        self.pointer.uniforms(area, target, true)
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
        let desktop = window_level(number).is_none_or(|level| {
            level <= CGWindowLevelForKey(CGWindowLevelKey::DesktopIconWindowLevelKey)
        });
        self.last_hit = Some((number, desktop));
        desktop
    }
}

fn left_clicks() -> u32 {
    CGEventSource::counter_for_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType::LeftMouseDown,
    )
}

/// The window level of window `number`, or `None` if there is no such window.
/// The level and the owner are readable without the screen recording
/// permission. Only the titles are not.
fn window_level(number: isize) -> Option<i32> {
    let number = u32::try_from(number).ok().filter(|&n| n != 0)?;
    let windows = CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, number)?;
    // SAFETY: The list holds a dictionary for each window, with string keys.
    let windows = unsafe { windows.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    let level = windows.get(0)?.get(unsafe { kCGWindowLayer })?;
    level.downcast::<CFNumber>().ok()?.as_i32()
}

/// True if `point` is in `rect`, with the left and top edges inside and the
/// right and bottom edges outside. A point on the edge of two screens is then
/// in one of them. `NSEvent::mouseLocation` puts the pointer of a screen
/// above its bottom edge and at most on its top edge.
pub fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y > rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}
