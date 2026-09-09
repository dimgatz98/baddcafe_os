use spin::Mutex;

const LEFT_SHIFT: usize = 0x2a;
const RIGHT_SHIFT: usize = 0x36;
const CAPS_LOCK: usize = 0x3a;
const EXTENDED: u8 = 0xe0;

/// Scancode set 1, the codes an AT keyboard sends after the controller has
/// translated them. Index is the make code; '\0' means "no printable character".
#[rustfmt::skip]
const UNSHIFTED: [char; 0x40] = [
    '\0', '\0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=', '\x08', '\t',
    'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']', '\n', '\0', 'a', 's',
    'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', '\'', '`', '\0', '\\', 'z', 'x', 'c', 'v',
    'b', 'n', 'm', ',', '.', '/', '\0', '*', '\0', ' ', '\0', '\0', '\0', '\0', '\0', '\0',
];

#[rustfmt::skip]
const SHIFTED: [char; 0x40] = [
    '\0', '\0', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+', '\x08', '\t',
    'Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', 'O', 'P', '{', '}', '\n', '\0', 'A', 'S',
    'D', 'F', 'G', 'H', 'J', 'K', 'L', ':', '"', '~', '\0', '|', 'Z', 'X', 'C', 'V',
    'B', 'N', 'M', '<', '>', '?', '\0', '*', '\0', ' ', '\0', '\0', '\0', '\0', '\0', '\0',
];

pub struct Keyboard {
    shift: bool,
    caps_lock: bool,
    extended: bool,
}

impl Keyboard {
    pub const fn new() -> Keyboard {
        Keyboard {
            shift: false,
            caps_lock: false,
            extended: false,
        }
    }

    /// Bit 7 separates a press from a release: the release code is the make code
    /// with that bit set, which is why the table only needs 0x40 entries.
    pub fn decode(&mut self, scancode: u8) -> Option<char> {
        if scancode == EXTENDED {
            self.extended = true;
            return None;
        }
        if self.extended {
            self.extended = false;
            return None;
        }

        let released = scancode & 0x80 != 0;
        let code = (scancode & 0x7f) as usize;

        match code {
            LEFT_SHIFT | RIGHT_SHIFT => {
                self.shift = !released;
                return None;
            }
            CAPS_LOCK => {
                if !released {
                    self.caps_lock = !self.caps_lock;
                }
                return None;
            }
            _ => {}
        }

        if released || code >= UNSHIFTED.len() {
            return None;
        }

        let character = if self.shift {
            SHIFTED[code]
        } else {
            UNSHIFTED[code]
        };

        match character {
            '\0' => None,
            _ if self.caps_lock && character.is_ascii_alphabetic() => {
                Some(flip_case(character))
            }
            _ => Some(character),
        }
    }
}

fn flip_case(character: char) -> char {
    if character.is_ascii_uppercase() {
        character.to_ascii_lowercase()
    } else {
        character.to_ascii_uppercase()
    }
}

pub static KEYBOARD: Mutex<Keyboard> = Mutex::new(Keyboard::new());
