//! The timing of a video channel: which decoded frame shows at each moment of
//! the playback clock of a wallpaper. A backend decodes the video on its own
//! thread and gives the frames, with their time stamps, to a [`VideoQueue`].
//! This module does not depend on the platform.
//!
//! The video plays at the speed of the playback clock, so it stops while
//! time stops. The playback time before the first frame counts too, so the
//! video does not depend on how fast the decoder starts. At its end, the
//! decoder starts the video again and tells the queue. The last frame of the
//! video still shows for its full length, and the next loop starts after it.
//!
//! The queue keeps two frames: the newest frame that is due, and the first
//! frame that is not due yet. While a frame waits for its time, the decoder
//! waits too, so it does not decode far ahead of the playback. A frame that
//! is already due when it comes takes the place of the due frame that did
//! not show yet. So a decoder that is behind does not wait, and it catches
//! up with the playback between two frames of the wallpaper.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a frame shows if its length is not known yet: the first frame
/// of a video without frame lengths.
const DEFAULT_FRAME_LENGTH: f64 = 1.0 / 30.0;

/// A decoded frame, as the decoder of a backend gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded<F> {
    pub frame: F,
    /// The time of the frame in the video, in seconds.
    pub time: f64,
    /// How long the frame shows, in seconds, if the decoder knows it.
    pub length: Option<f64>,
}

/// The time line of a video channel, without threads. [`VideoQueue`] shares
/// it between the decoder and the renderer.
///
/// All times are in one time line that starts at the first frame and goes on
/// through the loops. For example, a frame of the second loop is due at the
/// length of the first loop plus its time stamp.
#[derive(Debug)]
pub struct VideoTimer<F> {
    /// The time stamp and the length of the last frame from the decoder.
    received: Option<(f64, f64)>,
    /// The length of the loops before the loop that the decoder decodes now.
    offset: f64,
    /// True if the decoder started the video again after its last frame.
    restarted: bool,
    /// The playback time. `None` until the first frame comes.
    position: Option<f64>,
    /// The playback time before the first frame came.
    waited: f64,
    /// The newest frame that is due and did not show yet.
    ready: Option<F>,
    /// The first frame that is not due yet, and when it is due.
    next: Option<(F, f64)>,
}

impl<F> Default for VideoTimer<F> {
    fn default() -> Self {
        Self {
            received: None,
            offset: 0.0,
            restarted: false,
            position: None,
            waited: 0.0,
            ready: None,
            next: None,
        }
    }
}

impl<F> VideoTimer<F> {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if the decoder must wait before it gives the next frame: a frame
    /// that is not due yet waits already.
    pub fn is_full(&self) -> bool {
        self.next.is_some()
    }

    /// The decoder started the video again: the next frame is the first
    /// frame of a new loop.
    pub fn start_loop(&mut self) {
        self.restarted = true;
    }

    /// Puts a decoded frame in the time line. Call it only while the timer
    /// is not full.
    pub fn receive(&mut self, decoded: Decoded<F>) {
        debug_assert!(!self.is_full(), "a frame came while another one waits");
        // A time stamp that goes back also starts a new loop, for a decoder
        // that does not tell. A video of one frame gives the same time stamp
        // in each loop, so only the decoder can tell its loops apart.
        let restarted = std::mem::take(&mut self.restarted);
        let mut wrapped = false;
        if let Some((time, length)) = self.received
            && (restarted || decoded.time < time)
        {
            // The next loop starts when the last frame of this loop ends. The
            // time stamps of some videos start after 0, so the first frame of
            // the new loop is due then, whatever its time stamp.
            let loop_length = time + length - decoded.time;
            log::debug!("video channel: the video starts again after {loop_length:.3} s");
            self.offset += loop_length;
            wrapped = true;
        }
        // Without a length from the decoder, a frame shows as long as the
        // frame before it did.
        let length = decoded
            .length
            .or_else(|| {
                self.received.map(
                    |(time, length)| {
                        if wrapped { length } else { decoded.time - time }
                    },
                )
            })
            .filter(|length| *length > 0.0)
            .unwrap_or(DEFAULT_FRAME_LENGTH);
        self.received = Some((decoded.time, length));
        let due = self.offset + decoded.time;
        // The playback starts at the first frame, after the time that passed
        // before it came.
        let waited = self.waited;
        let position = *self.position.get_or_insert(due + waited);
        if due <= position {
            self.ready = Some(decoded.frame);
        } else {
            self.next = Some((decoded.frame, due));
        }
    }

    /// Moves the playback `delta` seconds on.
    pub fn advance(&mut self, delta: f64) {
        let Some(position) = &mut self.position else {
            self.waited += delta;
            return;
        };
        *position += delta;
        if self.next.as_ref().is_some_and(|(_, due)| *due <= *position) {
            self.ready = self.next.take().map(|(frame, _)| frame);
        }
    }

    /// True until the decoder gives a frame that is not due yet. Only such a
    /// frame shows that no other frame is due: time stamps can be rounded,
    /// so the length of a frame does not tell when the next one is due.
    pub fn is_behind(&self) -> bool {
        self.next.is_none()
    }

    /// Takes the newest frame that is due, if a new one is.
    pub fn take(&mut self) -> Option<F> {
        self.ready.take()
    }
}

/// A [`VideoTimer`] that the decoder thread and the renderer share.
#[derive(Debug)]
pub struct VideoQueue<F> {
    shared: Mutex<Shared<F>>,
    /// Signals each change: a new frame for the renderer, or space for the
    /// decoder.
    changed: Condvar,
}

#[derive(Debug)]
struct Shared<F> {
    timer: VideoTimer<F>,
    /// True after the decoder stopped or the renderer closed the queue.
    ended: bool,
}

impl<F> Default for VideoQueue<F> {
    fn default() -> Self {
        Self {
            shared: Mutex::new(Shared {
                timer: VideoTimer::new(),
                ended: false,
            }),
            changed: Condvar::new(),
        }
    }
}

impl<F> VideoQueue<F> {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Shared<F>> {
        // Each step under the lock leaves the timer whole, so a panic in
        // another thread does not break it.
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// For the decoder: waits while a frame waits for its time, and then
    /// puts `decoded` in the time line. Returns false if the queue ended, so
    /// the decoder must stop.
    pub fn push(&self, decoded: Decoded<F>) -> bool {
        let mut shared = self.lock();
        while shared.timer.is_full() && !shared.ended {
            shared = self
                .changed
                .wait(shared)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if shared.ended {
            return false;
        }
        shared.timer.receive(decoded);
        self.changed.notify_all();
        true
    }

    /// For the renderer: moves the playback `delta` seconds on and takes the
    /// newest frame that is due, if a new one is. If the decoder is behind,
    /// this waits for it at most `wait`.
    pub fn advance(&self, delta: f64, wait: Duration) -> Option<F> {
        let deadline = Instant::now() + wait;
        let mut shared = self.lock();
        shared.timer.advance(delta);
        // The decoder can go on if a waiting frame became due.
        self.changed.notify_all();
        while shared.timer.is_behind() && !shared.ended {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            shared = self
                .changed
                .wait_timeout(shared, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        shared.timer.take()
    }

    /// For the decoder: it started the video again, so the next frame is the
    /// first frame of a new loop. Call it before the first frame of that loop
    /// can come.
    pub fn start_loop(&self) {
        self.lock().timer.start_loop();
    }

    /// For the decoder: it gives no more frames, so the renderer does not
    /// wait for them. The frames in the queue still show when they are due.
    pub fn end(&self) {
        self.lock().ended = true;
        self.changed.notify_all();
    }

    /// For the renderer: stops the queue. The decoder stops at its next
    /// frame, and the frames in the queue go.
    pub fn close(&self) {
        let timer = {
            let mut shared = self.lock();
            shared.ended = true;
            std::mem::take(&mut shared.timer)
        };
        self.changed.notify_all();
        drop(timer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// What a decoder gives: true if the frame starts a new loop, and the
    /// frame.
    type Item = (bool, Decoded<usize>);

    /// A video of `frames` frames of `length` seconds that loops, as a
    /// decoder gives it. The frame is its number in the video.
    fn looping(frames: usize, length: f64, with_length: bool) -> impl FnMut() -> Item {
        let mut n = 0;
        move || {
            let frame = n % frames;
            n += 1;
            let decoded = Decoded {
                frame,
                time: frame as f64 * length,
                length: with_length.then_some(length),
            };
            (n > 1 && frame == 0, decoded)
        }
    }

    /// Gives frames from `decoder` to `timer` until it is full, as a
    /// decoder that is always fast enough does. With `tells`, the decoder
    /// tells the timer when it starts the video again.
    fn fill(timer: &mut VideoTimer<usize>, decoder: &mut impl FnMut() -> Item, tells: bool) {
        // Stop a decoder that never waits, so that the test fails and does
        // not run forever.
        for _ in 0..1000 {
            if timer.is_full() {
                return;
            }
            let (starts_loop, decoded) = decoder();
            if starts_loop && tells {
                timer.start_loop();
            }
            timer.receive(decoded);
        }
        panic!("the decoder never waits");
    }

    /// The frame that shows after each step of `step` seconds, for `steps`
    /// steps.
    fn play(
        timer: &mut VideoTimer<usize>,
        decoder: &mut impl FnMut() -> Item,
        tells: bool,
        step: f64,
        steps: usize,
    ) -> Vec<usize> {
        let mut shown = 0;
        (0..steps)
            .map(|i| {
                timer.advance(if i == 0 { 0.0 } else { step });
                fill(timer, decoder, tells);
                if let Some(frame) = timer.take() {
                    shown = frame;
                }
                shown
            })
            .collect()
    }

    #[test]
    fn the_last_frame_shows_for_its_full_length() {
        // A video of one second with four frames. A loop counts once, also
        // when the decoder tells it and the time stamps go back.
        for with_length in [true, false] {
            for tells in [true, false] {
                let mut timer = VideoTimer::new();
                let mut decoder = looping(4, 0.25, with_length);
                // Steps of 1/16 s, which a float holds exactly: each frame
                // shows for four steps, and each loop for sixteen.
                let shown = play(&mut timer, &mut decoder, tells, 0.0625, 320);
                for (i, frame) in shown.iter().enumerate() {
                    assert_eq!(
                        *frame,
                        (i / 4) % 4,
                        "step {i}, lengths {with_length}, tells {tells}"
                    );
                }
            }
        }
    }

    #[test]
    fn loops_do_not_drift_from_the_clock() {
        // Steps of 0.01 s, which a float does not hold exactly. After 100
        // loops of 0.3 s, at 30.05 s, the middle frame of a loop shows.
        let mut timer = VideoTimer::new();
        let mut decoder = looping(3, 0.1, true);
        let shown = play(&mut timer, &mut decoder, true, 0.01, 3006);
        assert_eq!(shown[3005], 0);
        // 30.15 s is the middle of the second frame.
        let shown = play(&mut timer, &mut decoder, true, 0.01, 11);
        assert_eq!(shown[10], 1);
    }

    #[test]
    fn a_slow_wallpaper_shows_the_frame_of_its_time() {
        // A video of 32 frames per second in a wallpaper of 4 frames per
        // second: each step is 8 frames of the video.
        let mut timer = VideoTimer::new();
        let mut decoder = looping(64, 1.0 / 32.0, true);
        let shown = play(&mut timer, &mut decoder, true, 0.25, 40);
        for (i, frame) in shown.iter().enumerate() {
            assert_eq!(*frame, (i * 8) % 64, "step {i}");
        }
    }

    #[test]
    fn a_video_that_starts_after_zero_loops_without_a_gap() {
        // The time stamps of the video go from 0.5 s to 1.5 s, so a loop is
        // one second long.
        for with_length in [true, false] {
            for tells in [true, false] {
                let mut timer = VideoTimer::new();
                let mut frames = looping(4, 0.25, with_length);
                let mut decoder = || {
                    let (starts_loop, mut decoded) = frames();
                    decoded.time += 0.5;
                    (starts_loop, decoded)
                };
                let shown = play(&mut timer, &mut decoder, tells, 0.0625, 80);
                for (i, frame) in shown.iter().enumerate() {
                    assert_eq!(
                        *frame,
                        (i / 4) % 4,
                        "step {i}, lengths {with_length}, tells {tells}"
                    );
                }
            }
        }
    }

    #[test]
    fn time_before_the_first_frame_counts() {
        // `dump --at 0.6` of a wallpaper at 1 fps moves the playback before
        // the decoder gives its first frame. The frame at 0.6 s shows.
        let mut timer = VideoTimer::new();
        let mut decoder = looping(4, 0.25, true);
        timer.advance(0.6);
        fill(&mut timer, &mut decoder, true);
        assert_eq!(timer.take(), Some(2));
        timer.advance(0.15);
        fill(&mut timer, &mut decoder, true);
        assert_eq!(timer.take(), Some(3));
    }

    #[test]
    fn a_video_of_one_frame_waits_for_its_next_loop() {
        // Each loop gives the same time stamp. The frame is the number of
        // the loop.
        let mut timer = VideoTimer::new();
        let mut loops = 0;
        let mut decoder = || {
            loops += 1;
            let decoded = Decoded {
                frame: loops,
                time: 0.0,
                length: Some(0.5),
            };
            (loops > 1, decoded)
        };
        fill(&mut timer, &mut decoder, true);
        assert_eq!(timer.take(), Some(1));
        // While time stops, the decoder waits.
        for _ in 0..3 {
            timer.advance(0.0);
            fill(&mut timer, &mut decoder, true);
            assert_eq!(timer.take(), None);
        }
        timer.advance(0.5);
        fill(&mut timer, &mut decoder, true);
        assert_eq!(timer.take(), Some(2));
        assert_eq!(loops, 3, "the decoder waits with the next loop");
    }

    #[test]
    fn a_slow_decoder_shows_the_newest_due_frame() {
        let frame = |n: usize| Decoded {
            frame: n,
            time: n as f64 * 0.1,
            length: Some(0.1),
        };
        let mut timer = VideoTimer::new();
        assert!(timer.is_behind(), "no frame came yet");
        timer.receive(frame(0));
        assert_eq!(timer.take(), Some(0));
        // Nothing new is decoded, and the next frame is due.
        timer.advance(0.25);
        assert!(timer.is_behind());
        assert_eq!(timer.take(), None);
        // Two frames come at once: both are due, and the newer one shows.
        timer.receive(frame(1));
        timer.receive(frame(2));
        assert!(!timer.is_full());
        assert!(timer.is_behind(), "frame 3 can be due too");
        // Frame 3 is not due yet: the decoder waits, and no other frame is
        // due.
        timer.receive(frame(3));
        assert!(timer.is_full());
        assert!(!timer.is_behind());
        assert_eq!(timer.take(), Some(2));
        assert_eq!(timer.take(), None);
        timer.advance(0.1);
        assert!(!timer.is_full());
        assert_eq!(timer.take(), Some(3));
    }

    #[test]
    fn the_decoder_thread_catches_up_with_a_slow_wallpaper() {
        // The decoder runs on its own thread, as in a backend. Each step of
        // the wallpaper is 8 frames of the video. The renderer waits for the
        // decoder, so each step shows the frame of its time.
        let queue = Arc::new(VideoQueue::new());
        let decoder = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                let mut decoder = looping(64, 1.0 / 32.0, true);
                loop {
                    let (starts_loop, decoded) = decoder();
                    if starts_loop {
                        queue.start_loop();
                    }
                    if !queue.push(decoded) {
                        break;
                    }
                }
            })
        };
        let wait = Duration::from_secs(5);
        assert_eq!(queue.advance(0.0, wait), Some(0));
        for i in 1..40 {
            assert_eq!(queue.advance(0.25, wait), Some((i * 8) % 64), "step {i}");
        }
        queue.close();
        decoder
            .join()
            .expect("the decoder stops when the queue closes");
    }

    #[test]
    fn rounded_time_stamps_show_the_due_frame() {
        // As in zerotv.mkv: frames of 1/60 s, with time stamps rounded to
        // whole milliseconds. Frame 2 at 0.033 s is due at 0.0335 s, before
        // the end of frame 1 at 0.017 + 1/60 s.
        let queue = Arc::new(VideoQueue::new());
        let decoder = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                for n in 0..60 {
                    let decoded = Decoded {
                        frame: n,
                        time: (n as f64 * 1000.0 / 60.0).round() / 1000.0,
                        length: Some(1.0 / 60.0),
                    };
                    if !queue.push(decoded) {
                        break;
                    }
                }
            })
        };
        let wait = Duration::from_secs(5);
        assert_eq!(queue.advance(0.0, wait), Some(0));
        assert_eq!(queue.advance(0.0335, wait), Some(2));
        queue.close();
        decoder
            .join()
            .expect("the decoder stops when the queue closes");
    }

    #[test]
    fn an_ended_decoder_is_not_waited_for() {
        let queue = VideoQueue::<usize>::new();
        queue.end();
        let start = Instant::now();
        assert_eq!(queue.advance(0.1, Duration::from_secs(5)), None);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!queue.push(Decoded {
            frame: 0,
            time: 0.0,
            length: None,
        }));
    }
}
