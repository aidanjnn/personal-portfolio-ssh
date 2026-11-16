use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PASTE_END: &[u8] = b"\x1b[201~";

/// SSH packets are transport chunks, not key boundaries.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    discard_paste: bool,
}

impl Decoder {
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<KeyEvent> {
        self.feed_up_to(bytes, usize::MAX)
    }

    /// Decodes all of `bytes`, so paste and escape state stay correct, but keeps only the
    /// first `max` keys.
    pub fn feed_up_to(&mut self, bytes: &[u8], max: usize) -> Vec<KeyEvent> {
        let mut keys = Vec::new();
        for &byte in bytes {
            self.pending.push(byte);
            self.decode(&mut keys);
            keys.truncate(max);
        }
        keys
    }

    pub fn has_escape(&self) -> bool {
        !self.discard_paste && self.pending == [0x1b]
    }

    pub fn expire_escape(&mut self) -> Option<KeyEvent> {
        if self.has_escape() {
            self.pending.clear();
            Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        } else {
            // CSI/SS3 prefixes resolve the Escape ambiguity. Keep them across slow packets.
            None
        }
    }

    fn decode(&mut self, keys: &mut Vec<KeyEvent>) {
        if self.discard_paste {
            // Only the literal end marker leaves a paste, whatever came before it, so a
            // partial sequence inside the paste cannot hide the marker. Keep just enough of
            // the tail to recognise a marker split across packets.
            if self.pending.ends_with(PASTE_END) {
                self.discard_paste = false;
                self.pending.clear();
            } else if self.pending.len() >= PASTE_END.len() {
                self.pending.remove(0);
            }
            return;
        }
        let first = self.pending[0];
        if first == 0x1b {
            if self.pending.len() == 1 {
                return;
            }
            if matches!(self.pending[1], b'[' | b'O') {
                if self.pending.len() < 3 {
                    return;
                }
                if self.pending.len() > 32 {
                    self.pending.clear();
                    return;
                }
                let last = *self.pending.last().unwrap();
                if !(0x40..=0x7e).contains(&last) {
                    return;
                }
                if self.pending == b"\x1b[200~" {
                    self.discard_paste = true;
                } else {
                    let code = match self.pending.as_slice() {
                        b"\x1b[A" | b"\x1bOA" => Some(KeyCode::Up),
                        b"\x1b[B" | b"\x1bOB" => Some(KeyCode::Down),
                        b"\x1b[C" | b"\x1bOC" => Some(KeyCode::Right),
                        b"\x1b[D" | b"\x1bOD" => Some(KeyCode::Left),
                        b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" | b"\x1b[7~" => Some(KeyCode::Home),
                        b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" | b"\x1b[8~" => Some(KeyCode::End),
                        b"\x1b[5~" => Some(KeyCode::PageUp),
                        b"\x1b[6~" => Some(KeyCode::PageDown),
                        b"\x1b[3~" => Some(KeyCode::Delete),
                        b"\x1b[Z" => Some(KeyCode::BackTab),
                        _ => None,
                    };
                    if let Some(code) = code {
                        let modifiers = if code == KeyCode::BackTab {
                            KeyModifiers::SHIFT
                        } else {
                            KeyModifiers::NONE
                        };
                        keys.push(KeyEvent::new(code, modifiers));
                    }
                }
                self.pending.clear();
            } else {
                // A lone Escape followed by anything but a CSI/SS3 introducer is a press of
                // Escape, then the next key: Esc Esc is two presses and `ESC q` is Esc, q.
                // Alt chords are not portfolio commands, so splitting them loses nothing.
                keys.push(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                self.pending.remove(0);
                self.decode(keys);
            }
            return;
        }
        if first.is_ascii() {
            self.pending.clear();
            let (code, modifiers) = match first {
                b'\r' | b'\n' => (KeyCode::Enter, KeyModifiers::NONE),
                b'\t' => (KeyCode::Tab, KeyModifiers::NONE),
                0x08 | 0x7f => (KeyCode::Backspace, KeyModifiers::NONE),
                1..=26 => (
                    KeyCode::Char(char::from(first + b'a' - 1)),
                    KeyModifiers::CONTROL,
                ),
                0x20..=0x7e => (KeyCode::Char(char::from(first)), KeyModifiers::NONE),
                _ => return,
            };
            keys.push(KeyEvent::new(code, modifiers));
        } else {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    keys.extend(chars(text));
                    self.pending.clear();
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    let Some(invalid) = error.error_len() else {
                        // An incomplete character: wait for the rest of it.
                        return;
                    };
                    // Keep what decoded, drop only the invalid bytes, and decode the rest:
                    // `C3 q` loses the stray C3, not the q.
                    let text = std::str::from_utf8(&self.pending[..valid]).unwrap_or_default();
                    keys.extend(chars(text));
                    self.pending.drain(..valid + invalid);
                    if !self.pending.is_empty() {
                        self.decode(keys);
                    }
                }
            }
        }
    }
}

fn chars(text: &str) -> impl Iterator<Item = KeyEvent> + '_ {
    text.chars()
        .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(keys: Vec<KeyEvent>) -> Vec<KeyCode> {
        keys.into_iter().map(|key| key.code).collect()
    }

    #[test]
    fn an_invalid_byte_drops_only_itself() {
        let mut decoder = Decoder::default();
        assert_eq!(codes(decoder.feed(b"\xc3q")), [KeyCode::Char('q')]);
        assert_eq!(
            codes(decoder.feed(b"\xe2\x82q\xffj\xc3\xa9")),
            [KeyCode::Char('q'), KeyCode::Char('j'), KeyCode::Char('é')]
        );
        // A truncated character before an escape sequence still lets the sequence through.
        assert_eq!(codes(decoder.feed(b"\xf0\x9f\x1b[B")), [KeyCode::Down]);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn a_lone_escape_before_another_key_is_escape_then_that_key() {
        let mut decoder = Decoder::default();
        assert_eq!(
            codes(decoder.feed(b"\x1b\x1b")),
            [KeyCode::Esc],
            "the second Escape waits for its timeout"
        );
        assert_eq!(decoder.expire_escape().unwrap().code, KeyCode::Esc);
        assert_eq!(
            codes(decoder.feed(b"\x1bq")),
            [KeyCode::Esc, KeyCode::Char('q')]
        );
        assert_eq!(
            codes(decoder.feed(b"\x1b\x1b\x1b[A\x1b\r")),
            [
                KeyCode::Esc,
                KeyCode::Esc,
                KeyCode::Up,
                KeyCode::Esc,
                KeyCode::Enter
            ]
        );
        // Split across packets, the pair decodes the same way.
        assert!(decoder.feed(b"\x1b").is_empty());
        assert_eq!(
            codes(decoder.feed(b"j")),
            [KeyCode::Esc, KeyCode::Char('j')]
        );
    }

    #[test]
    fn a_paste_ends_at_its_marker_even_after_a_partial_sequence() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(b"\x1b[200~qq\x1b[12\x1b[201~").is_empty());
        assert_eq!(codes(decoder.feed(b"k")), [KeyCode::Char('k')]);
        // A stray Escape inside a paste is not a key, and neither is its timeout.
        assert!(decoder.feed(b"\x1b[200~q\x1b").is_empty());
        assert!(!decoder.has_escape());
        assert!(decoder.expire_escape().is_none());
        // The end marker may arrive split at any byte.
        for byte in &b"x\x1b[201~"[..] {
            assert!(decoder.feed(&[*byte]).is_empty());
        }
        assert_eq!(codes(decoder.feed(b"j")), [KeyCode::Char('j')]);
    }

    #[test]
    fn oversized_input_keeps_only_the_first_keys_but_all_of_its_state() {
        let mut decoder = Decoder::default();
        let keys = decoder.feed_up_to(&[b'j'; 32768], 32);
        assert_eq!(keys.len(), 32);
        // A paste far longer than the cap still ends, so later typing is not swallowed.
        let mut paste = b"\x1b[200~".to_vec();
        paste.extend_from_slice(&[b'q'; 20000]);
        paste.extend_from_slice(b"\x1b[201~k\x1b");
        let keys = decoder.feed_up_to(&paste, 32);
        assert_eq!(
            keys,
            [KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)]
        );
        assert!(decoder.has_escape());
        assert!(decoder.feed_up_to(b"[B", 0).is_empty());
        assert_eq!(decoder.feed(b"j").len(), 1);
    }

    #[test]
    fn decodes_at_every_packet_boundary() {
        let bytes = "j\x1b[B\x1b[6~é🦆\x03".as_bytes();
        let expected = Decoder::default().feed(bytes);
        for split in 0..=bytes.len() {
            let mut decoder = Decoder::default();
            let mut actual = decoder.feed(&bytes[..split]);
            actual.extend(decoder.feed(&bytes[split..]));
            assert_eq!(actual, expected, "split {split}");
        }
        assert_eq!(expected.len(), 6);
        assert_eq!(expected[1].code, KeyCode::Down);
        assert_eq!(expected[4].code, KeyCode::Char('🦆'));
        assert_eq!(expected[5].modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn escape_waits_but_unknown_sequences_do_not_become_keys() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(b"\x1b").is_empty());
        assert!(decoder.has_escape());
        assert_eq!(decoder.expire_escape().unwrap().code, KeyCode::Esc);
        assert!(decoder.feed(b"\x1b[99q").is_empty());
        assert_eq!(decoder.feed(b"j")[0].code, KeyCode::Char('j'));
    }

    #[test]
    fn bracketed_paste_is_not_navigation() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(b"\x1b[200~qjj\x03\x1b[201~").is_empty());
        assert_eq!(decoder.feed(b"k")[0].code, KeyCode::Char('k'));
    }

    #[tokio::test]
    async fn known_escape_prefixes_survive_the_lone_escape_timeout() {
        for prefix in [b"\x1b[", b"\x1bO"] {
            let mut decoder = Decoder::default();
            assert!(decoder.feed(prefix).is_empty());
            assert!(!decoder.has_escape());
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(decoder.expire_escape().is_none());
            assert_eq!(
                decoder.feed(b"B"),
                [KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)]
            );
        }
    }

    #[test]
    fn seeded_random_chunks_never_panic_or_grow_the_buffer() {
        use rand::{RngExt, SeedableRng, rngs::StdRng};
        const FRAGMENTS: &[&[u8]] = &[
            b"\x1b",
            b"\x1b[",
            b"\x1bO",
            b"\x1b[A",
            b"\x1b[6~",
            b"\x1b[1;5C",
            b"\x1b[200~",
            b"\x1b[201~",
            b"\x1b[99999999999999999999999999999999999q",
            "é".as_bytes(),
            "€".as_bytes(),
            "🦆".as_bytes(),
            &[0xf0, 0x9f],
            &[0xe2, 0x82],
            &[0xc3],
            &[0xff, 0xfe, 0x80],
            b"q\x03\r\t\x7f",
        ];
        let mut rng = StdRng::seed_from_u64(0x5eed_7e57);
        let mut decoder = Decoder::default();
        for _ in 0..20_000 {
            let length = rng.random_range(1..=64);
            let mut chunk = Vec::with_capacity(length + 64);
            while chunk.len() < length {
                if rng.random_bool(0.4) {
                    chunk.extend_from_slice(FRAGMENTS[rng.random_range(0..FRAGMENTS.len())]);
                } else {
                    chunk.push(rng.random());
                }
            }
            chunk.truncate(length);
            // An Escape left over from the previous chunk becomes a key with this one's first byte.
            let carried = usize::from(decoder.has_escape());
            let keys = decoder.feed(&chunk);
            assert!(keys.len() <= chunk.len() + carried);
            assert!(
                decoder.pending.len() <= 32,
                "{} pending",
                decoder.pending.len()
            );
            if rng.random_bool(0.05) {
                decoder.expire_escape();
            }
        }
        // Leave paste mode and any partial sequence; the decoder still recovers.
        decoder.feed(b"\x1b[201~");
        decoder.feed(&[0xff]);
        assert_eq!(
            decoder.feed(b"k"),
            [KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)]
        );
    }

    #[test]
    fn malformed_input_stays_bounded_and_recovers() {
        let mut decoder = Decoder::default();
        decoder.feed(b"\x1b[");
        decoder.feed(&[b'1'; 100]);
        assert!(decoder.pending.len() <= 32);
        decoder.feed(&[0xff, 0xff]);
        assert_eq!(decoder.feed(b"k")[0].code, KeyCode::Char('k'));
    }
}
