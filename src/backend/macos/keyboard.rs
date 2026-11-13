//! The keyboard of a procedural wallpaper, in the layout of Shadertoy's
//! keyboard texture.
//!
//! The texture is 256 texels wide, one for each JavaScript key code, and 3
//! rows high. Row 0 is 1 while a key is down. Row 1 is 1 on the frame after a
//! press. Row 2 flips on each press. Shaders read it with
//! `texelFetch(iChannelN, ivec2(code, row), 0).x`.

use objc2_app_kit::NSEventModifierFlags;

/// One texel for each JavaScript key code.
pub const KEYS: usize = 256;

/// `NX_DEVICELCMDKEYMASK` and `NX_DEVICERCMDKEYMASK` from IOLLEvent.h. Both
/// Command keys set `NSEventModifierFlags::Command`, but each has its own
/// texel. The other pairs of modifiers share a texel and a flag each.
const LEFT_COMMAND: NSEventModifierFlags = NSEventModifierFlags(0x08);
const RIGHT_COMMAND: NSEventModifierFlags = NSEventModifierFlags(0x10);

pub struct Keyboard {
    rows: [[u8; KEYS]; 3],
    /// True if the texture differs from the one that the last frame got.
    changed: bool,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            rows: [[0; KEYS]; 3],
            changed: true,
        }
    }
}

impl Keyboard {
    /// A key with macOS key code `code` went down or up. A key that was down
    /// when the wallpaper took the keyboard arrives only as repeats. A
    /// `repeat` holds the key down without a press.
    pub fn key(&mut self, code: u16, down: bool, repeat: bool) {
        let Some(js) = js_key_code(code) else {
            return;
        };
        let i = usize::from(js);
        if down && !repeat && self.rows[0][i] == 0 {
            self.rows[1][i] = 255;
            self.rows[2][i] ^= 255;
        }
        self.rows[0][i] = if down { 255 } else { 0 };
        self.changed = true;
    }

    /// A modifier key changed. macOS reports the key and the new flags, not
    /// whether the key went down.
    pub fn modifier(&mut self, code: u16, flags: NSEventModifierFlags) {
        let flag = match code {
            0x38 | 0x3C => NSEventModifierFlags::Shift,
            0x3B | 0x3E => NSEventModifierFlags::Control,
            0x3A | 0x3D => NSEventModifierFlags::Option,
            // An event without the device bits, as a synthesized one can be.
            0x37 | 0x36 if !flags.intersects(LEFT_COMMAND | RIGHT_COMMAND) => {
                NSEventModifierFlags::Command
            }
            0x37 => LEFT_COMMAND,
            0x36 => RIGHT_COMMAND,
            0x39 => NSEventModifierFlags::CapsLock,
            _ => return,
        };
        self.key(code, flags.contains(flag), false);
    }

    /// Releases every key, for when the wallpaper loses the keyboard. Keys
    /// would stay down otherwise, because their key-up goes to another
    /// window.
    pub fn release_all(&mut self) {
        if self.rows[0].iter().any(|&v| v != 0) {
            self.rows[0] = [0; KEYS];
            self.changed = true;
        }
    }

    /// The texture for the next frame, if it changed since the last call.
    /// A press shows on that frame only.
    pub fn take_frame(&mut self) -> Option<[u8; KEYS * 3]> {
        if !self.changed {
            return None;
        }
        let mut texture = [0u8; KEYS * 3];
        for (row, out) in self.rows.iter().zip(texture.as_chunks_mut::<KEYS>().0) {
            *out = *row;
        }
        // The next frame needs a new texture only to clear the presses.
        self.changed = self.rows[1].iter().any(|&v| v != 0);
        self.rows[1] = [0; KEYS];
        Some(texture)
    }
}

/// The JavaScript `keyCode` for a macOS virtual key code. The codes follow
/// the position of a key on a US keyboard, as `keyCode` does for letters
/// there.
fn js_key_code(code: u16) -> Option<u8> {
    Some(match code {
        0x00 => b'A',
        0x0B => b'B',
        0x08 => b'C',
        0x02 => b'D',
        0x0E => b'E',
        0x03 => b'F',
        0x05 => b'G',
        0x04 => b'H',
        0x22 => b'I',
        0x26 => b'J',
        0x28 => b'K',
        0x25 => b'L',
        0x2E => b'M',
        0x2D => b'N',
        0x1F => b'O',
        0x23 => b'P',
        0x0C => b'Q',
        0x0F => b'R',
        0x01 => b'S',
        0x11 => b'T',
        0x20 => b'U',
        0x09 => b'V',
        0x0D => b'W',
        0x07 => b'X',
        0x10 => b'Y',
        0x06 => b'Z',
        0x1D => b'0',
        0x12 => b'1',
        0x13 => b'2',
        0x14 => b'3',
        0x15 => b'4',
        0x17 => b'5',
        0x16 => b'6',
        0x1A => b'7',
        0x1C => b'8',
        0x19 => b'9',
        0x33 => 8,   // Delete (backspace)
        0x30 => 9,   // Tab
        0x24 => 13,  // Return
        0x4C => 13,  // Keypad Enter
        0x38 => 16,  // Shift
        0x3C => 16,  // Right Shift
        0x3B => 17,  // Control
        0x3E => 17,  // Right Control
        0x3A => 18,  // Option
        0x3D => 18,  // Right Option
        0x39 => 20,  // Caps Lock
        0x35 => 27,  // Escape
        0x31 => 32,  // Space
        0x74 => 33,  // Page Up
        0x79 => 34,  // Page Down
        0x77 => 35,  // End
        0x73 => 36,  // Home
        0x7B => 37,  // Left
        0x7E => 38,  // Up
        0x7C => 39,  // Right
        0x7D => 40,  // Down
        0x75 => 46,  // Forward Delete
        0x37 => 91,  // Command
        0x36 => 93,  // Right Command
        0x52 => 96,  // Keypad 0
        0x53 => 97,  // Keypad 1
        0x54 => 98,  // Keypad 2
        0x55 => 99,  // Keypad 3
        0x56 => 100, // Keypad 4
        0x57 => 101, // Keypad 5
        0x58 => 102, // Keypad 6
        0x59 => 103, // Keypad 7
        0x5B => 104, // Keypad 8
        0x5C => 105, // Keypad 9
        0x43 => 106, // Keypad *
        0x45 => 107, // Keypad +
        0x4E => 109, // Keypad -
        0x41 => 110, // Keypad .
        0x4B => 111, // Keypad /
        0x7A => 112, // F1
        0x78 => 113, // F2
        0x63 => 114, // F3
        0x76 => 115, // F4
        0x60 => 116, // F5
        0x61 => 117, // F6
        0x62 => 118, // F7
        0x64 => 119, // F8
        0x65 => 120, // F9
        0x6D => 121, // F10
        0x67 => 122, // F11
        0x6F => 123, // F12
        0x29 => 186, // ;
        0x18 => 187, // =
        0x2B => 188, // ,
        0x1B => 189, // -
        0x2F => 190, // .
        0x2C => 191, // /
        0x32 => 192, // `
        0x21 => 219, // [
        0x2A => 220, // \
        0x1E => 221, // ]
        0x27 => 222, // '
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u16 = 0x0D;

    fn texel(texture: &[u8; KEYS * 3], code: u8, row: usize) -> u8 {
        texture[row * KEYS + usize::from(code)]
    }

    #[test]
    fn press_hold_and_release() {
        let mut k = Keyboard::default();
        assert!(k.take_frame().is_some(), "the first frame gets a texture");
        assert!(k.take_frame().is_none());

        k.key(W, true, false);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 1), texel(&t, b'W', 2)),
            (255, 255, 255)
        );
        // Held, and the press is over.
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, b'W', 0), texel(&t, b'W', 1)), (255, 0));
        assert!(k.take_frame().is_none());

        // Key repeat does not press again.
        k.key(W, true, true);
        let t = k.take_frame().unwrap();
        assert_eq!(texel(&t, b'W', 1), 0);

        k.key(W, false, false);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 2)),
            (0, 255),
            "the toggle stays"
        );
        k.key(W, true, false);
        assert_eq!(texel(&k.take_frame().unwrap(), b'W', 2), 0);
    }

    #[test]
    fn modifiers_and_focus_loss() {
        let mut k = Keyboard::default();
        k.take_frame();
        k.modifier(0x38, NSEventModifierFlags::Shift);
        assert_eq!(texel(&k.take_frame().unwrap(), 16, 0), 255);
        k.modifier(0x38, NSEventModifierFlags::empty());
        assert_eq!(texel(&k.take_frame().unwrap(), 16, 0), 0);

        k.key(0x31, true, false);
        k.take_frame();
        k.release_all();
        assert_eq!(texel(&k.take_frame().unwrap(), 32, 0), 0);
    }

    /// Finder has the keyboard for a moment during a click on the desktop.
    /// A key held through the click comes back as repeats, without a press.
    #[test]
    fn a_repeat_after_focus_loss_is_no_press() {
        let mut k = Keyboard::default();
        k.key(W, true, false);
        k.take_frame();
        k.take_frame();
        k.release_all();
        k.take_frame();
        k.key(W, true, true);
        let t = k.take_frame().unwrap();
        assert_eq!(
            (texel(&t, b'W', 0), texel(&t, b'W', 1), texel(&t, b'W', 2)),
            (255, 0, 255)
        );
    }

    #[test]
    fn each_command_key_releases() {
        let command = NSEventModifierFlags::Command;
        let mut k = Keyboard::default();
        k.modifier(0x37, command | LEFT_COMMAND);
        k.modifier(0x36, command | LEFT_COMMAND | RIGHT_COMMAND);
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, 91, 0), texel(&t, 93, 0)), (255, 255));
        k.modifier(0x37, command | RIGHT_COMMAND);
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, 91, 0), texel(&t, 93, 0)), (0, 255));
        k.modifier(0x36, NSEventModifierFlags::empty());
        let t = k.take_frame().unwrap();
        assert_eq!((texel(&t, 91, 0), texel(&t, 93, 0)), (0, 0));
    }

    #[test]
    fn command_without_device_bits_uses_the_flag() {
        let mut k = Keyboard::default();
        k.modifier(0x37, NSEventModifierFlags::Command);
        assert_eq!(texel(&k.take_frame().unwrap(), 91, 0), 255);
        k.modifier(0x37, NSEventModifierFlags::empty());
        assert_eq!(texel(&k.take_frame().unwrap(), 91, 0), 0);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut k = Keyboard::default();
        k.take_frame();
        k.key(0x3F, true, false); // Fn
        assert!(k.take_frame().is_none());
    }
}
