//! The keyboard of a procedural wallpaper, in the layout of the keyboard
//! texture of Shadertoy.
//!
//! The texture is 256 texels wide, one for each JavaScript key code, and 3
//! rows high. Row 0 is 1 while a key is down. Row 1 is 1 on the frame after a
//! press. Row 2 flips on each press. Shaders read it with
//! `texelFetch(iChannelN, ivec2(code, row), 0).x`.
//!
//! Wayland gives each key as an evdev code, the code of the Linux input
//! layer. Each seat has its own keyboard and its own focus. Seats and outputs
//! go by their registry names.

use std::collections::HashMap;

/// One texel for each JavaScript key code.
pub const KEYS: usize = 256;

pub type Rows = [[u8; KEYS]; 3];

/// The keys of every wallpaper with a keyboard channel, and the wallpaper
/// that the keyboard of each seat focuses.
#[derive(Default)]
pub struct Keyboards {
    /// The keys of each source with a keyboard channel, by the index of the
    /// source.
    by_source: HashMap<usize, Keyboard>,
    /// The output and the source that the keyboard of each seat focuses. Only
    /// a source with a keyboard channel counts.
    focus: HashMap<u32, (u32, usize)>,
}

impl Keyboards {
    pub fn new(sources: impl IntoIterator<Item = usize>) -> Self {
        Self {
            by_source: sources
                .into_iter()
                .map(|s| (s, Keyboard::default()))
                .collect(),
            focus: HashMap::new(),
        }
    }

    /// The keyboard of `seat` entered the surface of `output`, which shows
    /// `source`, while the keys `held` were down. True if the source has a
    /// keyboard channel.
    pub fn enter(
        &mut self,
        seat: u32,
        output: u32,
        source: Option<usize>,
        held: impl IntoIterator<Item = u32>,
    ) -> bool {
        // The compositor sends a `Leave` first. This covers one that does not.
        self.leave(seat);
        let Some(source) = source else {
            return false;
        };
        let Some(keyboard) = self.by_source.get_mut(&source) else {
            return false;
        };
        keyboard.enter(seat, held);
        self.focus.insert(seat, (output, source));
        true
    }

    pub fn key(&mut self, seat: u32, code: u32, down: bool) {
        if let Some((_, source)) = self.focus.get(&seat)
            && let Some(keyboard) = self.by_source.get_mut(source)
        {
            keyboard.key(seat, code, down);
        }
    }

    /// The keyboard of `seat` left its surface, or went away. Releases the
    /// keys of the seat, and returns the output that it left.
    pub fn leave(&mut self, seat: u32) -> Option<u32> {
        let (output, source) = self.focus.remove(&seat)?;
        if let Some(keyboard) = self.by_source.get_mut(&source) {
            keyboard.leave(seat);
        }
        Some(output)
    }

    /// True if the keyboard of a seat focuses `output`.
    pub fn focuses(&self, output: u32) -> bool {
        self.focus.values().any(|&(o, _)| o == output)
    }

    /// Releases the keys of the seats that focus `output`, an output that is
    /// gone.
    pub fn remove_output(&mut self, output: u32) {
        let seats: Vec<u32> = self
            .focus
            .iter()
            .filter(|(_, (o, _))| *o == output)
            .map(|(&seat, _)| seat)
            .collect();
        for seat in seats {
            self.leave(seat);
        }
    }

    /// The rows of `source` for its next frame, if they changed.
    pub fn take_frame(&mut self, source: usize) -> Option<Rows> {
        self.by_source.get_mut(&source)?.take_frame()
    }
}

/// The keys of one source.
struct Keyboard {
    rows: Rows,
    /// True if the rows changed since the last frame took them.
    changed: bool,
    /// The keys that are down, as a seat and an evdev code. Keys can share a
    /// texel, for example the left and the right Shift, or the same key on
    /// two seats. The texel stays at 1 while one of them is down.
    held: Vec<(u32, u32)>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            rows: [[0; KEYS]; 3],
            // The program starts with the same empty texture.
            changed: false,
            held: Vec::new(),
        }
    }
}

impl Keyboard {
    /// The keyboard of `seat` came while the keys `held` were down. These keys
    /// are held but not pressed, because their press went to another window.
    /// They replace the keys that the seat held before.
    fn enter(&mut self, seat: u32, held: impl IntoIterator<Item = u32>) {
        self.leave(seat);
        for code in held {
            self.set(seat, code, true, false);
        }
    }

    fn key(&mut self, seat: u32, code: u32, down: bool) {
        self.set(seat, code, down, down);
    }

    /// Releases the keys of `seat`, when its keyboard goes. After that, its
    /// key-ups go to another window.
    fn leave(&mut self, seat: u32) {
        let codes: Vec<u32> = self
            .held
            .iter()
            .filter(|&&(s, _)| s == seat)
            .map(|&(_, code)| code)
            .collect();
        for code in codes {
            self.set(seat, code, false, false);
        }
    }

    /// The rows for the next frame, if they changed since the last call. A
    /// press shows on that frame only.
    fn take_frame(&mut self) -> Option<Rows> {
        if !self.changed {
            return None;
        }
        let rows = self.rows;
        // The next frame needs a new texture only to clear the presses.
        self.changed = self.rows[1].iter().any(|&v| v != 0);
        self.rows[1] = [0; KEYS];
        Some(rows)
    }

    /// Sets key `code` of `seat` down or up. With `press`, a key that goes
    /// down also shows a press.
    fn set(&mut self, seat: u32, code: u32, down: bool, press: bool) {
        let Some(js) = js_key_code(code) else {
            return;
        };
        let i = usize::from(js);
        if down {
            if !self.held.contains(&(seat, code)) {
                self.held.push((seat, code));
            }
        } else {
            self.held.retain(|&key| key != (seat, code));
        }
        if press && self.rows[0][i] == 0 {
            self.rows[1][i] = 255;
            self.rows[2][i] ^= 255;
            self.changed = true;
        }
        let held = self.held.iter().any(|&(_, c)| js_key_code(c) == Some(js));
        let value = if held { 255 } else { 0 };
        if self.rows[0][i] != value {
            self.rows[0][i] = value;
            self.changed = true;
        }
    }
}

/// The JavaScript `keyCode` for an evdev key code (see
/// `linux/input-event-codes.h`). The code follows the place of the key on a
/// US keyboard, whatever the layout, as on macOS.
fn js_key_code(code: u32) -> Option<u8> {
    Some(match code {
        1 => 27, // Escape
        2 => b'1',
        3 => b'2',
        4 => b'3',
        5 => b'4',
        6 => b'5',
        7 => b'6',
        8 => b'7',
        9 => b'8',
        10 => b'9',
        11 => b'0',
        12 => 189, // -
        13 => 187, // =
        14 => 8,   // Backspace
        15 => 9,   // Tab
        16 => b'Q',
        17 => b'W',
        18 => b'E',
        19 => b'R',
        20 => b'T',
        21 => b'Y',
        22 => b'U',
        23 => b'I',
        24 => b'O',
        25 => b'P',
        26 => 219, // [
        27 => 221, // ]
        28 => 13,  // Enter
        29 => 17,  // Left Control
        30 => b'A',
        31 => b'S',
        32 => b'D',
        33 => b'F',
        34 => b'G',
        35 => b'H',
        36 => b'J',
        37 => b'K',
        38 => b'L',
        39 => 186, // ;
        40 => 222, // '
        41 => 192, // `
        42 => 16,  // Left Shift
        43 => 220, // \
        44 => b'Z',
        45 => b'X',
        46 => b'C',
        47 => b'V',
        48 => b'B',
        49 => b'N',
        50 => b'M',
        51 => 188, // ,
        52 => 190, // .
        53 => 191, // /
        54 => 16,  // Right Shift
        55 => 106, // Keypad *
        56 => 18,  // Left Alt
        57 => 32,  // Space
        58 => 20,  // Caps Lock
        59 => 112, // F1
        60 => 113, // F2
        61 => 114, // F3
        62 => 115, // F4
        63 => 116, // F5
        64 => 117, // F6
        65 => 118, // F7
        66 => 119, // F8
        67 => 120, // F9
        68 => 121, // F10
        69 => 144, // Num Lock
        70 => 145, // Scroll Lock
        71 => 103, // Keypad 7
        72 => 104, // Keypad 8
        73 => 105, // Keypad 9
        74 => 109, // Keypad -
        75 => 100, // Keypad 4
        76 => 101, // Keypad 5
        77 => 102, // Keypad 6
        78 => 107, // Keypad +
        79 => 97,  // Keypad 1
        80 => 98,  // Keypad 2
        81 => 99,  // Keypad 3
        82 => 96,  // Keypad 0
        83 => 110, // Keypad .
        87 => 122, // F11
        88 => 123, // F12
        96 => 13,  // Keypad Enter
        97 => 17,  // Right Control
        98 => 111, // Keypad /
        99 => 44,  // Print Screen
        100 => 18, // Right Alt
        102 => 36, // Home
        103 => 38, // Up
        104 => 33, // Page Up
        105 => 37, // Left
        106 => 39, // Right
        107 => 35, // End
        108 => 40, // Down
        109 => 34, // Page Down
        110 => 45, // Insert
        111 => 46, // Delete
        119 => 19, // Pause
        125 => 91, // Left Super
        // Browsers on Linux give 92 to the right Super key, and 93 to the
        // menu key.
        126 => 92, // Right Super
        127 => 93, // Menu
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 17;
    const SPACE: u32 = 57;
    const LEFT_SHIFT: u32 = 42;
    const RIGHT_SHIFT: u32 = 54;

    const SEAT: u32 = 1;
    const OTHER_SEAT: u32 = 2;

    fn texel(rows: &Rows, code: u8, row: usize) -> u8 {
        rows[row][usize::from(code)]
    }

    #[test]
    fn press_hold_and_release() {
        let mut k = Keyboard::default();
        assert!(k.take_frame().is_none());

        k.key(SEAT, W, true);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 1), texel(&t, b'W', 2)),
            (255, 255, 255)
        );
        // Held, and the press is over.
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, b'W', 0), texel(&t, b'W', 1)), (255, 0));
        assert!(k.take_frame().is_none());

        k.key(SEAT, W, false);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 2)),
            (0, 255),
            "the toggle stays"
        );
        k.key(SEAT, W, true);
        assert_eq!(texel(&k.take_frame().unwrap(), b'W', 2), 0);
    }

    #[test]
    fn both_shift_keys_share_a_texel() {
        let mut k = Keyboard::default();
        k.key(SEAT, LEFT_SHIFT, true);
        // The frame of the press, and the frame that clears it.
        k.take_frame();
        k.take_frame();
        // The second Shift key is not a new press.
        k.key(SEAT, RIGHT_SHIFT, true);
        assert!(k.take_frame().is_none());
        // Shift stays down while one of the two keys is down.
        k.key(SEAT, LEFT_SHIFT, false);
        assert!(k.take_frame().is_none());
        k.key(SEAT, RIGHT_SHIFT, false);
        assert_eq!(texel(&k.take_frame().unwrap(), 16, 0), 0);
    }

    #[test]
    fn keys_down_at_enter_are_held_without_a_press() {
        let mut k = Keyboard::default();
        k.enter(SEAT, [W]);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 1), texel(&t, b'W', 2)),
            (255, 0, 0)
        );
        k.key(SEAT, W, false);
        assert_eq!(texel(&k.take_frame().unwrap(), b'W', 0), 0);
    }

    #[test]
    fn enter_replaces_the_keys_of_the_seat() {
        let mut k = Keyboard::default();
        k.key(SEAT, W, true);
        k.enter(SEAT, [SPACE]);
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, b'W', 0), texel(&t, 32, 0)), (0, 255));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut k = Keyboard::default();
        k.key(SEAT, 0x1d0, true); // KEY_FN
        assert!(k.take_frame().is_none());
    }

    /// Outputs 10 and 11 show sources 0 and 1, which both read the keyboard.
    fn two_wallpapers() -> Keyboards {
        Keyboards::new([0, 1])
    }

    #[test]
    fn leave_releases_the_keys_and_names_the_output() {
        let mut k = two_wallpapers();
        assert!(k.enter(SEAT, 10, Some(0), []));
        k.key(SEAT, SPACE, true);
        k.key(SEAT, W, true);
        k.take_frame(0);
        assert_eq!(k.leave(SEAT), Some(10));
        let t = k.take_frame(0).unwrap();
        assert_eq!((texel(&t, 32, 0), texel(&t, b'W', 0)), (0, 0));
        assert_eq!(k.leave(SEAT), None);
        // Keys after the leave go nowhere.
        k.key(SEAT, W, true);
        assert!(k.take_frame(0).is_none());
    }

    /// A click on output 11 while output 10 has the keyboard: the compositor
    /// sends `Leave` for 10, then `Enter` for 11. Only 10 lost the keyboard.
    #[test]
    fn a_move_to_another_output_names_only_the_old_output() {
        let mut k = two_wallpapers();
        k.enter(SEAT, 10, Some(0), []);
        assert_eq!(k.leave(SEAT), Some(10));
        assert!(k.enter(SEAT, 11, Some(1), []));
        assert!(k.focuses(11));
        assert!(!k.focuses(10));
    }

    #[test]
    fn a_surface_without_a_keyboard_channel_takes_no_keys() {
        let mut k = two_wallpapers();
        assert!(!k.enter(SEAT, 12, Some(5), [W]));
        assert!(!k.enter(SEAT, 12, None, [W]));
        assert!(!k.focuses(12));
        assert_eq!(k.leave(SEAT), None);
    }

    #[test]
    fn each_seat_keeps_its_own_focus() {
        let mut k = two_wallpapers();
        k.enter(SEAT, 10, Some(0), []);
        k.enter(OTHER_SEAT, 11, Some(1), []);
        k.key(SEAT, W, true);
        assert_eq!(texel(&k.take_frame(0).unwrap(), b'W', 0), 255);
        assert!(k.take_frame(1).is_none(), "the keys of one seat stay apart");
        // The leave of one seat does not end the focus of the other.
        assert_eq!(k.leave(OTHER_SEAT), Some(11));
        assert!(k.focuses(10));
        k.key(SEAT, W, false);
        assert_eq!(texel(&k.take_frame(0).unwrap(), b'W', 0), 0);
    }

    #[test]
    fn two_seats_on_one_wallpaper_hold_keys_apart() {
        let mut k = two_wallpapers();
        k.enter(SEAT, 10, Some(0), []);
        k.enter(OTHER_SEAT, 11, Some(0), []);
        k.key(SEAT, W, true);
        k.key(OTHER_SEAT, W, true);
        k.take_frame(0);
        k.leave(SEAT);
        // W stays down while the other seat holds it.
        assert_eq!(texel(&k.take_frame(0).unwrap(), b'W', 0), 255);
        k.key(OTHER_SEAT, W, false);
        assert_eq!(texel(&k.take_frame(0).unwrap(), b'W', 0), 0);
    }

    #[test]
    fn a_removed_output_releases_the_keys_of_its_seats() {
        let mut k = two_wallpapers();
        k.enter(SEAT, 10, Some(0), []);
        k.key(SEAT, W, true);
        k.take_frame(0);
        k.remove_output(10);
        assert!(!k.focuses(10));
        assert_eq!(texel(&k.take_frame(0).unwrap(), b'W', 0), 0);
    }
}
