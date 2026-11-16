//! Terminal escape sequences written after a frame: OSC 8 hyperlinks over drawn cells and
//! the OSC 52 clipboard write.
use std::fmt::Write as _;

use ratatui::style::{Color, Modifier, Style};

use crate::{app::App, text};

fn color_sgr(out: &mut String, color: Color, foreground: bool) {
    let (base, bright, extended) = if foreground {
        (30, 90, 38)
    } else {
        (40, 100, 48)
    };
    let _ = match color {
        Color::Reset => write!(out, ";{}", base + 9),
        Color::Black => write!(out, ";{base}"),
        Color::Red => write!(out, ";{}", base + 1),
        Color::Green => write!(out, ";{}", base + 2),
        Color::Yellow => write!(out, ";{}", base + 3),
        Color::Blue => write!(out, ";{}", base + 4),
        Color::Magenta => write!(out, ";{}", base + 5),
        Color::Cyan => write!(out, ";{}", base + 6),
        Color::Gray => write!(out, ";{}", base + 7),
        Color::DarkGray => write!(out, ";{bright}"),
        Color::LightRed => write!(out, ";{}", bright + 1),
        Color::LightGreen => write!(out, ";{}", bright + 2),
        Color::LightYellow => write!(out, ";{}", bright + 3),
        Color::LightBlue => write!(out, ";{}", bright + 4),
        Color::LightMagenta => write!(out, ";{}", bright + 5),
        Color::LightCyan => write!(out, ";{}", bright + 6),
        Color::White => write!(out, ";{}", bright + 7),
        Color::Indexed(index) => write!(out, ";{extended};5;{index}"),
        Color::Rgb(r, g, b) => write!(out, ";{extended};2;{r};{g};{b}"),
    };
}

/// SGR that reproduces a drawn cell's style exactly.
fn sgr(style: Style) -> String {
    let mut out = String::from("\x1b[0");
    let modifier = style.add_modifier;
    for (flag, code) in [
        (Modifier::BOLD, 1),
        (Modifier::DIM, 2),
        (Modifier::ITALIC, 3),
        (Modifier::UNDERLINED, 4),
        (Modifier::REVERSED, 7),
    ] {
        if modifier.contains(flag) {
            let _ = write!(out, ";{code}");
        }
    }
    color_sgr(&mut out, style.fg.unwrap_or(Color::Reset), true);
    color_sgr(&mut out, style.bg.unwrap_or(Color::Reset), false);
    out.push('m');
    if modifier.contains(Modifier::UNDERLINED)
        && let Some(Color::Rgb(r, g, b)) = style.underline_color
    {
        let _ = write!(out, "\x1b[58:2::{r}:{g}:{b}m");
    }
    out
}

/// OSC 8 overlays for the visible hyperlinks.
pub fn link_escapes(app: &App) -> String {
    let mut out = String::new();
    for link in &app.hyperlinks {
        let _ = write!(out, "\x1b7\x1b[{};{}H", link.y + 1, link.x + 1);
        let mut current = None;
        let mut opened = false;
        let mut skip = 0;
        for (symbol, style) in &link.cells {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if current != Some(*style) {
                out.push_str(&sgr(*style));
                current = Some(*style);
            }
            if !opened {
                let _ = write!(out, "\x1b]8;;{}\x1b\\", link.url);
                opened = true;
            }
            out.push_str(symbol);
            skip = text::width(symbol).saturating_sub(1);
        }
        out.push_str("\x1b]8;;\x1b\\\x1b[0m\x1b8");
    }
    out
}

/// The pending OSC 52 clipboard write, emitted once whether or not links are enabled.
pub fn clipboard_escape(app: &mut App) -> String {
    app.clipboard
        .take()
        .map(|text| format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
        .unwrap_or_default()
}

pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{content, dump, key, render, scenario};
    use crossterm::event::KeyCode;

    #[test]
    fn link_escapes_wrap_the_focused_url_in_osc_8() {
        let (app, _) = scenario(&[KeyCode::Char('j')], 100, 30);
        let escapes = link_escapes(&app);
        let open = "\x1b]8;;https://www.copperlane.ai/\x1b\\";
        assert!(escapes.contains(open), "{escapes:?}");
        let label = &escapes[escapes.find(open).unwrap() + open.len()..];
        let label = &label[..label.find("\x1b]8;;\x1b\\").unwrap()];
        let visible: String = label
            .split('\x1b')
            .map(|part| part.split_once('m').map_or(part, |(_, text)| text))
            .collect();
        assert_eq!(visible, "Copperlane (YC W26)");
        // Cursor saved, positioned at the link (row 10, col 41), reset and restored.
        assert!(escapes.starts_with(
            "\x1b7\x1b[10;41H\x1b[0;1;4;38;2;249;115;22;48;2;254;230;213m\x1b[58:2::249:115:22m"
        ));
        assert!(escapes.ends_with("\x1b]8;;\x1b\\\x1b[0m\x1b8"));
        assert!(!escapes.contains("\x1b]52;"));
    }

    #[test]
    fn copy_emits_osc_52_once() {
        let (mut app, buffer) = scenario(&[KeyCode::Char('j'), KeyCode::Char('c')], 100, 30);
        assert!(dump(&buffer)[29].contains("copied ↗ copperlane.ai"));
        // The clipboard write is separate from OSC 8, so --no-links still copies.
        assert!(!link_escapes(&app).contains("\x1b]52;"));
        let payload = format!("\x1b]52;c;{}\x07", base64(b"https://www.copperlane.ai/"));
        assert_eq!(clipboard_escape(&mut app), payload);
        assert_eq!(clipboard_escape(&mut app), "");
        key(&mut app, KeyCode::Char('j'));
        let buffer = render(&mut app, &content(), 100, 30);
        assert!(!dump(&buffer)[29].contains("copied"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }
}
