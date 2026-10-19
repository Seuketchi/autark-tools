//! A secret typed one byte at a time: what each key does to it, with no screen or terminal in the way, so it can be tested.

use zeroize::Zeroizing;

#[derive(Default)]
pub struct Entry {
    secret: Zeroizing<Vec<u8>>,
    /// Where we are inside an escape sequence (arrow keys and the like send several bytes, which aren't part of the secret).
    escape: Escape,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// Saw ESC.
    Started,
    /// Saw ESC [ or ESC O: swallowing until the final byte.
    Sequence,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Key {
    /// The secret changed (or something that doesn't change it was pressed); redraw the dots.
    Edited,
    /// Enter: the secret is complete.
    Done,
}

impl Entry {
    pub fn feed(&mut self, byte: u8) -> Key {
        match self.escape {
            Escape::Started => {
                self.escape = if matches!(byte, b'[' | b'O') { Escape::Sequence } else { Escape::None };
                return Key::Edited;
            }
            Escape::Sequence => {
                // Parameters are digits and ';', the final byte is a letter or '~'.
                if !(byte.is_ascii_digit() || byte == b';') {
                    self.escape = Escape::None;
                }
                return Key::Edited;
            }
            Escape::None => {}
        }
        match byte {
            b'\r' | b'\n' => return Key::Done,
            0x1b => self.escape = Escape::Started,
            0x7f | 0x08 => self.backspace(),
            0x15 => self.secret.clear(),
            byte if byte >= 0x20 => self.secret.push(byte),
            _ => {}
        }
        Key::Edited
    }

    /// Takes off the last character, which for UTF-8 is its lead byte and the continuation bytes after it.
    fn backspace(&mut self) {
        while let Some(last) = self.secret.pop() {
            if last & 0xC0 != 0x80 {
                break;
            }
        }
    }

    /// How many characters have been typed (one dot each).
    pub fn characters(&self) -> usize {
        self.secret.iter().filter(|&&b| b & 0xC0 != 0x80).count()
    }

    pub fn into_secret(self) -> Zeroizing<Vec<u8>> {
        self.secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_in(entry: &mut Entry, bytes: &[u8]) -> Key {
        let mut last = Key::Edited;
        for &b in bytes {
            last = entry.feed(b);
        }
        last
    }

    #[test]
    fn typing_then_enter_gives_the_secret() {
        let mut entry = Entry::default();
        assert_eq!(type_in(&mut entry, b"1234"), Key::Edited);
        assert_eq!(entry.characters(), 4);
        assert_eq!(entry.feed(b'\r'), Key::Done);
        assert_eq!(&entry.into_secret()[..], b"1234");
    }

    #[test]
    fn backspace_removes_one_character_and_does_nothing_when_empty() {
        let mut entry = Entry::default();
        type_in(&mut entry, b"12\x7f");
        assert_eq!(entry.characters(), 1);
        type_in(&mut entry, b"\x08\x7f\x7f");
        assert_eq!(entry.characters(), 0);
    }

    #[test]
    fn backspace_takes_a_whole_utf8_character() {
        let mut entry = Entry::default();
        type_in(&mut entry, "aé€".as_bytes());
        assert_eq!(entry.characters(), 3);
        entry.feed(0x7f);
        assert_eq!(entry.characters(), 2);
        entry.feed(0x7f);
        assert_eq!(&entry.into_secret()[..], b"a");
    }

    #[test]
    fn ctrl_u_clears_the_line() {
        let mut entry = Entry::default();
        type_in(&mut entry, b"abc\x15");
        assert_eq!(entry.characters(), 0);
    }

    #[test]
    fn arrow_keys_and_other_escape_sequences_are_not_part_of_the_secret() {
        let mut entry = Entry::default();
        type_in(&mut entry, b"1\x1b[A2\x1b[1;5C3\x1bOP4");
        assert_eq!(&entry.into_secret()[..], b"1234");
    }

    #[test]
    fn a_lone_escape_doesnt_swallow_the_next_key() {
        let mut entry = Entry::default();
        type_in(&mut entry, b"\x1bx7");
        assert_eq!(&entry.into_secret()[..], b"7");
    }

    #[test]
    fn newline_also_ends_the_entry_and_control_bytes_are_ignored() {
        let mut entry = Entry::default();
        assert_eq!(type_in(&mut entry, b"\x01\x02ab\n"), Key::Done);
        assert_eq!(&entry.into_secret()[..], b"ab");
    }
}
