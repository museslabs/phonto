//! The inputs of each frame, for every backend: the playback clock, the
//! frame-rate cap, the date, the location, and the pointer. `super::audio`
//! makes the audio levels.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::audio::AudioFrame;
use super::pack::MAX_INPUTS;
use crate::config::Location;

/// The largest step of `iTime` in one frame. A longer gap is a pause, for
/// example a battery pause or a hidden wallpaper. Time stops for a pause and
/// does not jump, so simulations stay stable. Below 8 fps, the limit is two
/// frame periods.
const MAX_TIME_DELTA: f64 = 0.25;

/// `iTime` goes back to 0 after this many seconds of playback. An f32 that
/// holds a week of seconds has a resolution of only 1/16 s, and animation
/// then stutters.
const TIME_WRAP: f64 = 24.0 * 60.0 * 60.0;

/// The most displays that `iDisplays` describes.
pub const MAX_DISPLAYS: usize = 8;

/// The Linux evdev code of the left mouse button, as `wl_pointer` reports it.
/// Other backends use this code for their left button.
pub const BTN_LEFT: u32 = 0x110;

/// The inputs of a pass, except its resolution, frame index, and channels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameInputs {
    pub time: f32,
    pub time_delta: f32,
    pub frame_rate: f32,
    pub date: [f32; 4],
    pub mouse: [f32; 4],
    pub pointer: [f32; 4],
    pub audio: AudioFrame,
    /// 0 if the system prefers light colors, 1 if it prefers dark colors.
    pub color_scheme: f32,
    /// The charge from 0 to 1, 1 while charging, 1 on AC power, and 1 if the
    /// system has a battery.
    pub battery: [f32; 4],
    /// See `location`.
    pub location: [f32; 4],
    /// The values of `Pack::inputs`, in the same order.
    pub inputs: [[f32; 4]; MAX_INPUTS],
    /// The place of each display in the image: x, y, width, and height, in
    /// the pixels of the image pass, from the bottom left. A wallpaper that
    /// does not span has one display, the whole image.
    pub displays: [[f32; 4]; MAX_DISPLAYS],
    pub display_count: i32,
}

/// Playback time. It advances only while frames are drawn.
#[derive(Debug)]
pub struct Clock {
    time: f64,
    last: Option<Instant>,
    frame_rate: f32,
    /// The longest gap that counts as playback. A longer gap is a pause.
    max_delta: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub time: f32,
    pub delta: f32,
    pub frame_rate: f32,
}

impl Clock {
    /// A clock for a wallpaper capped at `fps` frames per second, or `None`
    /// for no cap.
    pub fn new(fps: Option<f32>) -> Self {
        // A frame can arrive up to two frame periods after the last one and
        // still count in full. Without this, time at 1 fps runs at a quarter
        // of its speed.
        let max_delta = fps.map_or(MAX_TIME_DELTA, |f| MAX_TIME_DELTA.max(2.0 / f64::from(f)));
        Self {
            time: 0.0,
            last: None,
            frame_rate: 0.0,
            max_delta,
        }
    }

    pub fn tick(&mut self, now: Instant) -> Tick {
        let delta = self.last.map_or(0.0, |last| {
            now.saturating_duration_since(last)
                .as_secs_f64()
                .min(self.max_delta)
        });
        self.last = Some(now);
        self.time = (self.time + delta) % TIME_WRAP;
        if delta > 0.0 {
            let instant = (1.0 / delta) as f32;
            self.frame_rate = if self.frame_rate == 0.0 {
                instant
            } else {
                self.frame_rate * 0.9 + instant * 0.1
            };
        }
        Tick {
            time: self.time as f32,
            delta: delta as f32,
            frame_rate: self.frame_rate,
        }
    }
}

/// Keeps frames at least `1 / fps` apart.
#[derive(Debug)]
pub struct FrameLimiter {
    period: Option<Duration>,
    next: Option<Instant>,
}

impl FrameLimiter {
    pub fn new(fps: Option<f32>) -> Self {
        Self {
            period: fps.map(|f| Duration::from_secs_f64(1.0 / f64::from(f))),
            next: None,
        }
    }

    /// The earliest time for the next frame. `None` means now.
    pub fn next_frame(&self) -> Option<Instant> {
        self.next
    }

    pub fn rendered(&mut self, now: Instant) {
        let Some(period) = self.period else {
            return;
        };
        let next = self.next.unwrap_or(now) + period;
        // If drawing fell behind (a slow GPU, a pause), the cadence starts
        // again from now. It does not draw many frames to catch up.
        self.next = Some(if next <= now { now + period } else { next });
    }
}

/// `iDate` as on Shadertoy: year, month (0-11), day of the month (1-31), and
/// seconds since local midnight.
pub fn local_date(now: SystemTime) -> [f32; 4] {
    let Some((tm, nanos)) = local_time(now) else {
        return [0.0; 4];
    };
    let day_secs = tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec;
    [
        (tm.tm_year + 1900) as f32,
        tm.tm_mon as f32,
        tm.tm_mday as f32,
        day_secs as f32 + nanos as f32 / 1e9,
    ]
}

/// `iLocation`. x and y are the latitude and the longitude, in degrees. z is
/// the offset of the local time from UTC, in hours. w is 1 if the user set a
/// location. With the date and the time, a shader can compute the sun.
///
/// Without a location, it is the equator at the middle of the local time
/// zone. The sun there rises near 06:00 and sets near 18:00.
pub fn location(now: SystemTime, place: Option<Location>) -> [f32; 4] {
    let offset = local_time(now).map_or(0.0, |(tm, _)| tm.tm_gmtoff as f64 / 3600.0);
    match place {
        Some(p) => [p.latitude as f32, p.longitude as f32, offset as f32, 1.0],
        None => [0.0, (offset * 15.0) as f32, offset as f32, 0.0],
    }
}

/// The local calendar time of `now`, and the nanoseconds of its second.
fn local_time(now: SystemTime) -> Option<(libc::tm, u32)> {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = libc::time_t::try_from(since_epoch.as_secs()).ok()?;
    // SAFETY: `tm` is plain data, so a zeroed value is valid. We read it only
    // after localtime_r succeeds.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: Both pointers are valid during the call. localtime_r does not
    // use shared static state, unlike localtime.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    Some((tm, since_epoch.subsec_nanos()))
}

/// The pointer state of one output, in surface coordinates with the origin
/// at the top left. `uniforms` converts it to the pixels of the render target,
/// with the origin at the bottom left, as for `fragCoord`.
#[derive(Debug, Default, Clone)]
pub struct Pointer {
    pos: [f64; 2],
    inside: bool,
    down: bool,
    /// Where the left button was last pressed.
    click: [f64; 2],
    /// The last position while the left button was held.
    drag: [f64; 2],
    /// True after a press, until a frame reads it.
    clicked: bool,
    ever_clicked: bool,
    /// Where the left button was pressed since the backend last read the
    /// presses to run actions, oldest first. Unlike `click`, which a frame
    /// reads as on Shadertoy, no press is lost: each one can run an action.
    presses: VecDeque<[f64; 2]>,
}

/// The most presses that wait for the backend. More presses between two
/// frames push out the oldest.
const MAX_PRESSES: usize = 16;

impl Pointer {
    pub fn enter(&mut self, x: f64, y: f64) {
        self.inside = true;
        self.motion(x, y);
    }

    pub fn leave(&mut self) {
        self.inside = false;
    }

    pub fn motion(&mut self, x: f64, y: f64) {
        self.pos = [x, y];
        if self.down {
            self.drag = self.pos;
        }
    }

    pub fn button(&mut self, button: u32, pressed: bool) {
        if button != BTN_LEFT {
            return;
        }
        if pressed {
            self.down = true;
            self.clicked = true;
            self.ever_clicked = true;
            self.click = self.pos;
            self.drag = self.pos;
            if self.presses.len() == MAX_PRESSES {
                self.presses.pop_front();
            }
            self.presses.push_back(self.pos);
        } else {
            self.down = false;
        }
    }

    /// `(iMouse, iPointer)` for the next frame. `area` is the part of the
    /// surface coordinates that the render target shows: x, y, width, and
    /// height. A spanned wallpaper shows the whole desktop.
    ///
    /// `iMouse` is the same as on Shadertoy. `xy` is the drag position and
    /// `zw` is the click position. `z` is positive while the button is held.
    /// `w` is positive only on the first frame after a press. `iPointer` is
    /// `(x, y, 1 over the wallpaper, 1 while pressed)`.
    ///
    /// With `take_click` false, the frame does not take the click: `w` stays
    /// negative, and the next frame gets the click. The first frame of a new
    /// render target does this, because phonto reads the actions of a click
    /// from the frame before it.
    pub fn uniforms(
        &mut self,
        area: [f64; 4],
        target: (u32, u32),
        take_click: bool,
    ) -> ([f32; 4], [f32; 4]) {
        let [left, top, width, height] = area;
        let sx = f64::from(target.0) / width.max(1.0);
        let sy = f64::from(target.1) / height.max(1.0);
        let to_target =
            |[x, y]: [f64; 2]| [((x - left) * sx) as f32, ((top + height - y) * sy) as f32];

        let mouse = if self.ever_clicked {
            let [dx, dy] = to_target(self.drag);
            let [cx, cy] = to_target(self.click);
            [
                dx,
                dy,
                if self.down { cx } else { -cx },
                if self.clicked && take_click { cy } else { -cy },
            ]
        } else {
            [0.0; 4]
        };
        if take_click {
            self.clicked = false;
        }

        let [px, py] = to_target(self.pos);
        let pointer = [
            px,
            py,
            if self.inside { 1.0 } else { 0.0 },
            if self.down { 1.0 } else { 0.0 },
        ];
        (mouse, pointer)
    }

    /// The places of the presses that no one took yet, oldest first, as
    /// fractions of `area` from the bottom left. `area` is the same as for
    /// `uniforms`. A press outside `area` is left out.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn take_presses(&mut self, area: [f64; 4]) -> Vec<[f64; 2]> {
        self.presses
            .drain(..)
            .filter_map(|pos| fraction(pos, area))
            .collect()
    }

    /// The place of the pointer while it is over the wallpaper, as a
    /// fraction of `area` from the bottom left.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn hover(&self, area: [f64; 4]) -> Option<[f64; 2]> {
        if !self.inside {
            return None;
        }
        fraction(self.pos, area)
    }
}

/// `pos` in surface coordinates, as a fraction of `area` from the bottom
/// left. `None` if `pos` is outside `area`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn fraction([x, y]: [f64; 2], [left, top, width, height]: [f64; 4]) -> Option<[f64; 2]> {
    let fx = (x - left) / width.max(1.0);
    let fy = (top + height - y) / height.max(1.0);
    // The surface rows go from 0 at the top to `height` at the bottom. So a
    // row inside the area gives a fraction above 0 and at most 1.
    let inside = (0.0..1.0).contains(&fx) && fy > 0.0 && fy <= 1.0;
    inside.then_some([fx, fy])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_clamps_gaps_and_wraps() {
        let start = Instant::now();
        let mut clock = Clock::new(Some(30.0));
        assert_eq!(clock.tick(start).time, 0.0);
        let t = clock.tick(start + Duration::from_millis(100));
        assert!((t.time - 0.1).abs() < 1e-6);
        assert!((t.frame_rate - 10.0).abs() < 1e-3);
        // A ten-second pause advances time by at most MAX_TIME_DELTA.
        let t = clock.tick(start + Duration::from_secs(10));
        assert!((t.delta - MAX_TIME_DELTA as f32).abs() < 1e-6);

        clock.time = TIME_WRAP - 0.05;
        let t = clock.tick(start + Duration::from_millis(10_100));
        assert!(t.time < 0.1);
    }

    #[test]
    fn clock_keeps_real_time_at_low_frame_rates() {
        let start = Instant::now();
        let mut clock = Clock::new(Some(1.0));
        clock.tick(start);
        let t = clock.tick(start + Duration::from_secs(1));
        assert!((t.time - 1.0).abs() < 1e-6);
        assert!((t.frame_rate - 1.0).abs() < 1e-6);
        // A pause still freezes time, at two frame periods.
        let t = clock.tick(start + Duration::from_secs(60));
        assert!((t.delta - 2.0).abs() < 1e-6);

        let mut uncapped = Clock::new(None);
        uncapped.tick(start);
        let t = uncapped.tick(start + Duration::from_secs(5));
        assert!((t.delta - MAX_TIME_DELTA as f32).abs() < 1e-6);
    }

    #[test]
    fn limiter_keeps_cadence_and_resyncs_when_behind() {
        let start = Instant::now();
        let mut limiter = FrameLimiter::new(Some(10.0));
        assert_eq!(limiter.next_frame(), None);
        limiter.rendered(start);
        assert_eq!(
            limiter.next_frame(),
            Some(start + Duration::from_millis(100))
        );
        limiter.rendered(start + Duration::from_millis(105));
        assert_eq!(
            limiter.next_frame(),
            Some(start + Duration::from_millis(200))
        );
        limiter.rendered(start + Duration::from_secs(1));
        assert_eq!(
            limiter.next_frame(),
            Some(start + Duration::from_millis(1100))
        );

        let mut uncapped = FrameLimiter::new(None);
        uncapped.rendered(start);
        assert_eq!(uncapped.next_frame(), None);
    }

    /// The area of a whole surface of `w` x `h`.
    fn whole((w, h): (u32, u32)) -> [f64; 4] {
        [0.0, 0.0, f64::from(w), f64::from(h)]
    }

    #[test]
    fn mouse_follows_shadertoy_semantics() {
        let surface = (100, 50);
        let mut p = Pointer::default();
        p.enter(10.0, 5.0);
        let (mouse, pointer) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse, [0.0; 4], "no click yet");
        assert_eq!(pointer, [10.0, 45.0, 1.0, 0.0]);

        p.button(BTN_LEFT, true);
        p.motion(20.0, 10.0);
        let (mouse, _) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse, [20.0, 40.0, 10.0, 45.0], "first frame: z, w > 0");
        let (mouse, pointer) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse, [20.0, 40.0, 10.0, -45.0], "held: z > 0, w < 0");
        assert_eq!(pointer[3], 1.0);

        p.button(BTN_LEFT, false);
        p.motion(30.0, 10.0);
        let (mouse, pointer) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse, [20.0, 40.0, -10.0, -45.0], "released: drag stays");
        assert_eq!(pointer, [30.0, 40.0, 1.0, 0.0]);

        p.leave();
        assert_eq!(p.uniforms(whole(surface), surface, true).1[2], 0.0);
    }

    #[test]
    fn a_click_can_wait_for_the_next_frame() {
        let surface = (100, 50);
        let mut p = Pointer::default();
        p.enter(10.0, 5.0);
        p.button(BTN_LEFT, true);
        p.button(BTN_LEFT, false);
        let (mouse, _) = p.uniforms(whole(surface), surface, false);
        assert_eq!(mouse[3], -45.0, "the frame does not take the click");
        let (mouse, _) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse, [10.0, 45.0, -10.0, 45.0], "the next frame does");
        let (mouse, _) = p.uniforms(whole(surface), surface, true);
        assert_eq!(mouse[3], -45.0);
    }

    #[test]
    fn pointer_scales_to_render_target() {
        let mut p = Pointer::default();
        p.enter(50.0, 0.0);
        let (_, pointer) = p.uniforms(whole((100, 50)), (50, 25), true);
        assert_eq!(&pointer[..2], &[25.0, 25.0]);
    }

    #[test]
    fn pointer_maps_into_a_desktop_area() {
        // The desktop starts at x = -100 and is 300 x 100. A target of half
        // that size shows it.
        let mut p = Pointer::default();
        p.enter(50.0, 20.0);
        let (_, pointer) = p.uniforms([-100.0, 0.0, 300.0, 100.0], (150, 50), true);
        assert_eq!(&pointer[..2], &[75.0, 40.0]);
    }

    #[test]
    fn presses_wait_for_the_backend() {
        let area = [0.0, 0.0, 200.0, 100.0];
        let mut p = Pointer::default();
        p.enter(50.0, 25.0);
        assert_eq!(p.hover(area), Some([0.25, 0.75]));
        assert!(p.take_presses(area).is_empty());
        p.button(BTN_LEFT, true);
        p.button(BTN_LEFT, false);
        // A frame reads the click for iMouse. The press stays for the
        // actions, and it goes after one read.
        let _ = p.uniforms(area, (200, 100), true);
        assert_eq!(p.take_presses(area), [[0.25, 0.75]]);
        assert!(p.take_presses(area).is_empty());
        p.leave();
        assert_eq!(p.hover(area), None);
        // A desktop area that starts at x = 100 does not include x = 50.
        p.button(BTN_LEFT, true);
        assert!(p.take_presses([100.0, 0.0, 200.0, 100.0]).is_empty());
    }

    #[test]
    fn every_press_between_frames_waits() {
        // Two clicks in different places before the next frame, as with a
        // low frame rate: both wait, in order.
        let area = [0.0, 0.0, 200.0, 100.0];
        let mut p = Pointer::default();
        p.enter(20.0, 50.0);
        p.button(BTN_LEFT, true);
        p.button(BTN_LEFT, false);
        p.motion(180.0, 50.0);
        p.button(BTN_LEFT, true);
        p.button(BTN_LEFT, false);
        assert_eq!(p.take_presses(area), [[0.1, 0.5], [0.9, 0.5]]);
        // Past the limit, the oldest presses go.
        for _ in 0..MAX_PRESSES + 3 {
            p.button(BTN_LEFT, true);
            p.button(BTN_LEFT, false);
        }
        assert_eq!(p.take_presses(area).len(), MAX_PRESSES);
    }

    #[test]
    fn other_buttons_are_ignored() {
        let mut p = Pointer::default();
        p.button(0x111, true);
        assert_eq!(p.uniforms(whole((10, 10)), (10, 10), true).0, [0.0; 4]);
    }

    #[test]
    fn local_date_is_plausible() {
        let [year, month, day, secs] = local_date(SystemTime::now());
        assert!(year >= 2024.0);
        assert!((0.0..12.0).contains(&month));
        assert!((1.0..=31.0).contains(&day));
        assert!((0.0..86_401.0).contains(&secs));
    }

    #[test]
    fn location_has_the_time_zone() {
        let now = SystemTime::now();
        let athens = Location {
            latitude: 37.98,
            longitude: 23.73,
        };
        let [lat, lon, offset, set] = location(now, Some(athens));
        assert_eq!([lat, lon, set], [37.98, 23.73, 1.0]);
        assert!((-12.0..=14.0).contains(&offset));
        // Without a location, the place is on the equator, at the middle of
        // the time zone.
        assert_eq!(location(now, None), [0.0, offset * 15.0, offset, 0.0]);
    }
}
