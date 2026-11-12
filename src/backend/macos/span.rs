//! The geometry of `--span`: one image for the whole desktop, and the part
//! of it that each screen shows.
//!
//! macOS global coordinates start at the bottom left and go up, as
//! `fragCoord` and `iDisplays` do. Row 0 of the image is its bottom. Nothing
//! here turns y over, unlike on Wayland.

use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::procedural::MAX_DISPLAYS;

/// The rectangle that covers `screens`, in points.
pub fn desktop_rect(screens: &[NSRect]) -> NSRect {
    let mut bounds: Option<[f64; 4]> = None;
    for s in screens {
        let (x, y) = (s.origin.x, s.origin.y);
        let (w, h) = (s.size.width, s.size.height);
        let [left, bottom, right, top] = bounds.get_or_insert([x, y, x + w, y + h]);
        *left = left.min(x);
        *bottom = bottom.min(y);
        *right = right.max(x + w);
        *top = top.max(y + h);
    }
    let [left, bottom, right, top] = bounds.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    NSRect::new(
        NSPoint::new(left, bottom),
        NSSize::new((right - left).max(1.0), (top - bottom).max(1.0)),
    )
}

/// `iDisplays` and `iDisplayCount`: the place of each of `screens` in an
/// image of `dims` pixels that shows `desktop`.
pub fn display_rects(
    desktop: NSRect,
    screens: &[NSRect],
    dims: (u32, u32),
) -> ([[f32; 4]; MAX_DISPLAYS], i32) {
    let [dx, dy, dw, dh] = parts(desktop);
    let (sx, sy) = (dims.0 as f32 / dw, dims.1 as f32 / dh);
    let mut displays = [[0.0; 4]; MAX_DISPLAYS];
    for (display, &screen) in displays.iter_mut().zip(screens) {
        let [x, y, w, h] = parts(screen);
        *display = [(x - dx) * sx, (y - dy) * sy, w * sx, h * sy];
    }
    (displays, screens.len().min(MAX_DISPLAYS) as i32)
}

/// The part of `desktop` that `screen` shows, as fractions from the bottom
/// left.
pub fn crop(desktop: NSRect, screen: NSRect) -> [f32; 4] {
    let [dx, dy, dw, dh] = parts(desktop);
    let [x, y, w, h] = parts(screen);
    [(x - dx) / dw, (y - dy) / dh, w / dw, h / dh]
}

fn parts(r: NSRect) -> [f32; 4] {
    [r.origin.x, r.origin.y, r.size.width, r.size.height].map(|v| v as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    // The fixtures of the Wayland tests, with the origin at the bottom left:
    // a laptop, and a taller monitor to its right that reaches higher.
    const LAPTOP: [f64; 4] = [0.0, 0.0, 1280.0, 800.0];
    const MONITOR: [f64; 4] = [1280.0, 20.0, 1920.0, 1080.0];

    fn screens() -> [NSRect; 2] {
        [LAPTOP, MONITOR].map(|[x, y, w, h]| rect(x, y, w, h))
    }

    #[test]
    fn desktop_covers_all_screens() {
        let [laptop, monitor] = screens();
        let desktop = desktop_rect(&[laptop, monitor]);
        assert_eq!(desktop, rect(0.0, 0.0, 3200.0, 1100.0));
        assert_eq!(
            crop(desktop, monitor),
            [0.4, 20.0 / 1100.0, 0.6, 1080.0 / 1100.0]
        );
        assert_eq!(crop(desktop, laptop), [0.0, 0.0, 0.4, 800.0 / 1100.0]);
    }

    #[test]
    fn displays_are_in_image_pixels_from_the_bottom_left() {
        let screens = screens();
        let desktop = desktop_rect(&screens);
        // The image is half the size of the desktop.
        let (displays, count) = display_rects(desktop, &screens, (1600, 550));
        assert_eq!(count, 2);
        assert_eq!(displays[0], [0.0, 0.0, 640.0, 400.0]);
        assert_eq!(displays[1], [640.0, 10.0, 960.0, 540.0]);
    }

    #[test]
    fn one_screen_shows_everything() {
        let screen = rect(-1920.0, 0.0, 1920.0, 1080.0);
        let desktop = desktop_rect(&[screen]);
        assert_eq!(desktop, screen);
        assert_eq!(crop(desktop, screen), [0.0, 0.0, 1.0, 1.0]);
    }
}
