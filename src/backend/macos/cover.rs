//! When windows cover a wallpaper, as `pause_behind` says.
//!
//! The window server calls a window occluded only when no pixel of it shows.
//! The menu bar and the gaps between tiled windows always show some of the
//! wallpaper, and a translucent window counts as showing what is behind it.
//! So each mode reads something else:
//!
//! - `any` reads the frames of the windows of apps. They cover the screen
//!   when they cover 95% of its visible frame. Tiled windows leave 2 to 3%
//!   for their gaps.
//! - `opaque` asks the window server about the sentinel, a clear window over
//!   the visible frame, 64 points in from its edges. Opaque windows over all
//!   of it occlude it. Two windows side by side never do, because the gap
//!   between them runs through it.
//! - `none` never reads windows.
//!
//! In every mode, a hidden wallpaper is covered: its window is occluded, as
//! on the lock screen, in native full screen and under the screen saver, or
//! the screens sleep, or another user's session is in front.
//!
//! A screen pauses after it has been covered for a second, because windows
//! that move between Spaces cover a screen for less. It resumes at once.

use std::cell::OnceCell;
use std::time::{Duration, Instant};

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSScreen, NSWindow, NSWindowOcclusionState, NSWindowStyleMask,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
use objc2_core_graphics::{
    CGRectInset, CGRectIntersection, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowLayer,
};
use objc2_foundation::{MainThreadMarker, NSRect};

use super::displays::{assigned, is_named};
use crate::config::{Behind, PauseBehind};
use crate::plan::PauseBehindPlan;

/// The share of the visible frame that windows cover in `any` mode.
const COVERED_SHARE: f64 = 0.95;

/// How far the sentinel stays inside the visible frame. It clears the gaps
/// of tiled windows and their rounded corners.
const SENTINEL_INSET: f64 = 64.0;

/// How long a screen is covered before its wallpaper pauses.
const DELAY: Duration = Duration::from_secs(1);

/// How often `any` reads the windows while the wallpaper runs.
const POLL: Duration = Duration::from_secs(1);

/// How often a covered screen is read. The resume is what the user sees.
const POLL_COVERED: Duration = Duration::from_millis(250);

/// Whether windows cover one screen, by the screen's own modes.
pub struct Cover {
    pause_behind: PauseBehind,
    /// Only for a screen that uses `opaque` on battery or on power.
    sentinel: Option<Retained<NSWindow>>,
    /// When the screen was first read as covered, until it is read as
    /// uncovered.
    since: Option<Instant>,
    /// True once the screen has been covered for `DELAY`.
    covered: bool,
}

impl Cover {
    pub fn new(mtm: MainThreadMarker, screen: &NSScreen, pause_behind: PauseBehind) -> Self {
        let mut cover = Self {
            pause_behind,
            sentinel: None,
            since: None,
            covered: false,
        };
        cover.place(mtm, screen, pause_behind);
        cover
    }

    /// Sets the modes of the screen, and makes, moves or removes its
    /// sentinel.
    pub fn place(&mut self, mtm: MainThreadMarker, screen: &NSScreen, pause_behind: PauseBehind) {
        self.pause_behind = pause_behind;
        let opaque =
            pause_behind.mode(true) == Behind::Opaque || pause_behind.mode(false) == Behind::Opaque;
        let frame = CGRectInset(screen.visibleFrame(), SENTINEL_INSET, SENTINEL_INSET);
        match &self.sentinel {
            // A renamed screen stops drawing a window until it is ordered
            // front again.
            Some(sentinel) if opaque => {
                sentinel.setFrame_display(frame, false);
                sentinel.orderFront(None);
            }
            Some(_) => self.remove_sentinel(),
            None if opaque => self.sentinel = Some(sentinel(mtm, frame)),
            None => {}
        }
    }

    /// Reads whether `screen` is covered. `hidden` is true if nothing of the
    /// wallpaper can show. `windows` holds the frames of the windows, read
    /// once for all screens. Returns how soon to read again, if the screen
    /// needs it.
    pub fn read(
        &mut self,
        screen: &NSScreen,
        hidden: bool,
        on_battery: bool,
        windows: &OnceCell<Vec<NSRect>>,
        now: Instant,
    ) -> Option<Duration> {
        let mode = self.pause_behind.mode(on_battery);
        let covered = hidden
            || match mode {
                Behind::Any => {
                    let windows = windows.get_or_init(|| window_frames(screen.mtm()));
                    share(windows, screen.visibleFrame()) >= COVERED_SHARE
                }
                Behind::Opaque => self.sentinel.as_deref().is_some_and(|s| !shows(s)),
                Behind::None => false,
            };
        self.since = covered.then(|| self.since.unwrap_or(now));
        let was = self.covered;
        self.covered = self
            .since
            .is_some_and(|since| now.duration_since(since) >= DELAY);
        if self.covered != was {
            let state = if self.covered { "covered" } else { "uncovered" };
            log::info!(
                "display {:?} is {state}",
                screen.localizedName().to_string()
            );
        }
        let polls = mode == Behind::Any && !hidden;
        if covered && (polls || !self.covered) {
            Some(POLL_COVERED)
        } else {
            polls.then_some(POLL)
        }
    }

    pub fn covered(&self) -> bool {
        self.covered
    }

    fn remove_sentinel(&mut self) {
        if let Some(sentinel) = self.sentinel.take() {
            sentinel.orderOut(None);
        }
    }
}

impl Drop for Cover {
    fn drop(&mut self) {
        self.remove_sentinel();
    }
}

/// The `pause_behind` of `screen`: its own, or else the table's.
pub fn pause_behind(screen: &NSScreen, plan: &PauseBehindPlan) -> PauseBehind {
    assigned(screen, &plan.displays)
        .copied()
        .unwrap_or(plan.table)
}

/// Warns about the `pause_behind` of displays that are not connected. A
/// display takes its own when it connects.
pub fn warn_unmatched(mtm: MainThreadMarker, plan: &PauseBehindPlan) {
    let screens = NSScreen::screens(mtm);
    for id in plan.displays.keys() {
        if !screens.iter().any(|screen| is_named(&screen, id)) {
            log::warn!("no display is `{id}` now. It takes its `pause_behind` when it connects.");
        }
    }
}

/// True if some of `window` can show.
pub fn shows(window: &NSWindow) -> bool {
    window
        .occlusionState()
        .contains(NSWindowOcclusionState::Visible)
}

/// A clear window over `frame`, one level above the wallpaper. A window of
/// the wallpaper itself at the same level could occlude it.
fn sentinel(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSWindow> {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc::<NSWindow>(),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    super::place_on_desktop(&window, super::WALLPAPER_LEVEL + 1);
    window.orderFront(None);
    window
}

/// The frames of the windows of apps on screen, in AppKit coordinates. Apps
/// put their windows on layer 0. A window with alpha 0 shows nothing.
/// Reading frames, layers and alpha needs no permission. Only the titles do.
fn window_frames(mtm: MainThreadMarker) -> Vec<NSRect> {
    let Some(windows) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    // SAFETY: The list holds a dictionary for each window, with string keys.
    let windows = unsafe { windows.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    // CoreGraphics puts the origin at the top left of the zero screen, and
    // AppKit at its bottom left.
    let top = NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |zero| zero.frame().size.height);
    let (layer, alpha, bounds) = unsafe { (kCGWindowLayer, kCGWindowAlpha, kCGWindowBounds) };
    let number =
        |window: &CFDictionary<CFString, CFType>, key| window.get(key)?.downcast::<CFNumber>().ok();
    windows
        .iter()
        .filter_map(|window| {
            if number(&window, layer)?.as_i32()? != 0 || number(&window, alpha)?.as_f64()? <= 0.0 {
                return None;
            }
            let bounds = window.get(bounds)?.downcast::<CFDictionary>().ok()?;
            let mut frame = NSRect::default();
            // SAFETY: `frame` is a valid rect to write.
            if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut frame) } {
                return None;
            }
            frame.origin.y = top - frame.origin.y - frame.size.height;
            Some(frame)
        })
        .collect()
}

/// The share of `area` under `windows`. Where windows overlap, the area
/// counts once.
fn share(windows: &[NSRect], area: NSRect) -> f64 {
    let parts: Vec<NSRect> = windows
        .iter()
        .map(|window| CGRectIntersection(*window, area))
        .filter(|part| !part.is_empty())
        .collect();
    let mut edges: Vec<f64> = parts
        .iter()
        .flat_map(|part| [part.min().x, part.max().x])
        .collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup();
    // In each strip between two edges, merge the spans of the parts that
    // cross it.
    let mut covered = 0.0;
    for strip in edges.windows(2) {
        let (left, right) = (strip[0], strip[1]);
        let mut spans: Vec<(f64, f64)> = parts
            .iter()
            .filter(|part| part.min().x <= left && part.max().x >= right)
            .map(|part| (part.min().y, part.max().y))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut height = 0.0;
        let mut top = f64::NEG_INFINITY;
        for (bottom, end) in spans {
            let start = bottom.max(top);
            if end > start {
                height += end - start;
                top = end;
            }
        }
        covered += height * (right - left);
    }
    covered / (area.size.width * area.size.height)
}

#[cfg(test)]
mod tests {
    use objc2_foundation::{NSPoint, NSSize};

    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    #[test]
    fn overlapping_windows_count_once() {
        let screen = rect(0.0, 0.0, 100.0, 100.0);
        assert_eq!(share(&[], screen), 0.0);
        assert_eq!(share(&[rect(0.0, 0.0, 50.0, 100.0)], screen), 0.5);
        let both = [rect(0.0, 0.0, 60.0, 100.0), rect(40.0, 0.0, 60.0, 100.0)];
        assert_eq!(share(&both, screen), 1.0);
        let nested = [rect(0.0, 0.0, 100.0, 100.0), rect(10.0, 10.0, 20.0, 20.0)];
        assert_eq!(share(&nested, screen), 1.0);
    }

    #[test]
    fn only_the_area_counts() {
        let screen = rect(100.0, 100.0, 100.0, 100.0);
        // A window parked on the next screen, one point on this one.
        assert_eq!(share(&[rect(199.0, 100.0, 500.0, 500.0)], screen), 0.01);
        assert_eq!(share(&[rect(0.0, 0.0, 100.0, 100.0)], screen), 0.0);
        assert_eq!(share(&[rect(0.0, 0.0, 400.0, 400.0)], screen), 1.0);
    }

    #[test]
    fn tiled_windows_with_gaps_cover_the_screen() {
        // Two windows split side by side, 8 points from the edges and from
        // each other, on a 2560x1410 visible frame.
        let screen = rect(0.0, 0.0, 2560.0, 1410.0);
        let split = [
            rect(8.0, 8.0, 1268.0, 1394.0),
            rect(1284.0, 8.0, 1268.0, 1394.0),
        ];
        let covered = share(&split, screen);
        assert!(covered > COVERED_SHARE && covered < 0.98, "{covered}");
        let half = [rect(0.0, 0.0, 1280.0, 1410.0)];
        assert!(share(&half, screen) < COVERED_SHARE);
    }
}
