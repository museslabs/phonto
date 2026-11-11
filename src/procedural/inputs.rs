//! The inputs of each frame, for every backend: the playback clock, the
//! frame-rate cap, the date, and the pointer. `super::audio` makes the audio
//! levels.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::audio::AudioFrame;

/// The largest step of `iTime` in one frame. A longer gap is a pause, for
/// example a battery pause or a hidden wallpaper. Time stops for a pause and
/// does not jump, so simulations stay stable. Below 8 fps, the limit is two
/// frame periods.
const MAX_TIME_DELTA: f64 = 0.25;

/// `iTime` goes back to 0 after this many seconds of playback. An f32 that
/// holds a week of seconds has a resolution of only 1/16 s, and animation
/// then stutters.
const TIME_WRAP: f64 = 24.0 * 60.0 * 60.0;

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
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let Ok(secs) = libc::time_t::try_from(since_epoch.as_secs()) else {
        return [0.0; 4];
    };
    // SAFETY: `tm` is plain data, so a zeroed value is valid. We read it only
    // after localtime_r succeeds.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: Both pointers are valid during the call. localtime_r does not
    // use shared static state, unlike localtime.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return [0.0; 4];
    }
    let day_secs = tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec;
    [
        (tm.tm_year + 1900) as f32,
        tm.tm_mon as f32,
        tm.tm_mday as f32,
        day_secs as f32 + since_epoch.subsec_nanos() as f32 / 1e9,
    ]
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
}

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
        } else {
            self.down = false;
        }
    }

    /// `(iMouse, iPointer)` for the next frame, scaled from the size of the
    /// surface to the size of the render target.
    ///
    /// `iMouse` is the same as on Shadertoy. `xy` is the drag position and
    /// `zw` is the click position. `z` is positive while the button is held.
    /// `w` is positive only on the first frame after a press. `iPointer` is
    /// `(x, y, 1 over the wallpaper, 1 while pressed)`.
    pub fn uniforms(&mut self, surface: (u32, u32), target: (u32, u32)) -> ([f32; 4], [f32; 4]) {
        let sx = f64::from(target.0) / f64::from(surface.0.max(1));
        let sy = f64::from(target.1) / f64::from(surface.1.max(1));
        let to_target =
            |[x, y]: [f64; 2]| [(x * sx) as f32, ((f64::from(surface.1) - y) * sy) as f32];

        let mouse = if self.ever_clicked {
            let [dx, dy] = to_target(self.drag);
            let [cx, cy] = to_target(self.click);
            [
                dx,
                dy,
                if self.down { cx } else { -cx },
                if self.clicked { cy } else { -cy },
            ]
        } else {
            [0.0; 4]
        };
        self.clicked = false;

        let [px, py] = to_target(self.pos);
        let pointer = [
            px,
            py,
            if self.inside { 1.0 } else { 0.0 },
            if self.down { 1.0 } else { 0.0 },
        ];
        (mouse, pointer)
    }
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

    #[test]
    fn mouse_follows_shadertoy_semantics() {
        let surface = (100, 50);
        let mut p = Pointer::default();
        p.enter(10.0, 5.0);
        let (mouse, pointer) = p.uniforms(surface, surface);
        assert_eq!(mouse, [0.0; 4], "no click yet");
        assert_eq!(pointer, [10.0, 45.0, 1.0, 0.0]);

        p.button(BTN_LEFT, true);
        p.motion(20.0, 10.0);
        let (mouse, _) = p.uniforms(surface, surface);
        assert_eq!(mouse, [20.0, 40.0, 10.0, 45.0], "first frame: z, w > 0");
        let (mouse, pointer) = p.uniforms(surface, surface);
        assert_eq!(mouse, [20.0, 40.0, 10.0, -45.0], "held: z > 0, w < 0");
        assert_eq!(pointer[3], 1.0);

        p.button(BTN_LEFT, false);
        p.motion(30.0, 10.0);
        let (mouse, pointer) = p.uniforms(surface, surface);
        assert_eq!(mouse, [20.0, 40.0, -10.0, -45.0], "released: drag stays");
        assert_eq!(pointer, [30.0, 40.0, 1.0, 0.0]);

        p.leave();
        assert_eq!(p.uniforms(surface, surface).1[2], 0.0);
    }

    #[test]
    fn pointer_scales_to_render_target() {
        let mut p = Pointer::default();
        p.enter(50.0, 0.0);
        let (_, pointer) = p.uniforms((100, 50), (50, 25));
        assert_eq!(&pointer[..2], &[25.0, 25.0]);
    }

    #[test]
    fn other_buttons_are_ignored() {
        let mut p = Pointer::default();
        p.button(0x111, true);
        assert_eq!(p.uniforms((10, 10), (10, 10)).0, [0.0; 4]);
    }

    #[test]
    fn local_date_is_plausible() {
        let [year, month, day, secs] = local_date(SystemTime::now());
        assert!(year >= 2024.0);
        assert!((0.0..12.0).contains(&month));
        assert!((1.0..=31.0).contains(&day));
        assert!((0.0..86_401.0).contains(&secs));
    }
}
