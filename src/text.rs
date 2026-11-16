//! Cell-accurate word wrapping of styled spans that remembers where each tagged span landed.
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

/// One styled input span. `tag` marks a focusable region (its item index).
#[derive(Clone, Copy, Debug)]
pub struct Piece<'a> {
    pub text: &'a str,
    pub style: Style,
    pub tag: Option<usize>,
}

impl<'a> Piece<'a> {
    pub fn new(text: &'a str, style: Style) -> Self {
        Self {
            text,
            style,
            tag: None,
        }
    }

    pub fn tagged(text: &'a str, style: Style, tag: Option<usize>) -> Self {
        Self { text, style, tag }
    }
}

/// A run of same-styled text at column `x` (relative to the start of its line).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub x: u16,
    pub text: String,
    pub style: Style,
    pub tag: Option<usize>,
}

pub type Line = Vec<Run>;

pub fn char_width(ch: char) -> u16 {
    ch.width().unwrap_or(0) as u16
}

pub fn width(text: &str) -> u16 {
    text.chars()
        .map(|ch| char_width(ch) as usize)
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    width: u16,
    style: Style,
    tag: Option<usize>,
}

/// Word-wraps `pieces` to `max` cells. Only spaces break; longer words break by character.
pub fn wrap(pieces: &[Piece], max: u16) -> Vec<Line> {
    let max = max.max(1);
    let cells: Vec<Cell> = pieces
        .iter()
        .flat_map(|piece| {
            piece.text.chars().map(move |ch| Cell {
                ch,
                width: char_width(ch),
                style: piece.style,
                tag: piece.tag,
            })
        })
        .collect();
    let mut lines: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut used = 0u16;
    let mut index = 0;
    while index < cells.len() {
        if cells[index].ch == ' ' {
            index += 1;
            continue;
        }
        // Spaces before this word, then the word itself.
        let mut gap_start = index;
        while gap_start > 0 && cells[gap_start - 1].ch == ' ' {
            gap_start -= 1;
        }
        let mut end = index;
        while end < cells.len() && cells[end].ch != ' ' {
            end += 1;
        }
        let gap = &cells[gap_start..index];
        let word = &cells[index..end];
        let gap_width: u32 = gap.iter().map(|cell| u32::from(cell.width)).sum();
        let word_width: u32 = word.iter().map(|cell| u32::from(cell.width)).sum();
        let line_empty = lines.last().is_some_and(Vec::is_empty);
        if !line_empty && u32::from(used) + gap_width + word_width <= u32::from(max) {
            lines.last_mut().unwrap().extend_from_slice(gap);
            used += gap_width as u16;
        } else if !line_empty {
            lines.push(Vec::new());
            used = 0;
        }
        for &cell in word {
            if used + cell.width > max && used > 0 {
                lines.push(Vec::new());
                used = 0;
            }
            lines.last_mut().unwrap().push(cell);
            used += cell.width;
        }
        index = end;
    }
    lines.into_iter().map(merge).collect()
}

fn merge(cells: Vec<Cell>) -> Line {
    let mut runs: Line = Vec::new();
    let mut x = 0u16;
    for cell in cells {
        match runs.last_mut() {
            Some(run) if run.style == cell.style && run.tag == cell.tag => run.text.push(cell.ch),
            _ => runs.push(Run {
                x,
                text: cell.ch.to_string(),
                style: cell.style,
                tag: cell.tag,
            }),
        }
        x += cell.width;
    }
    runs
}

/// Breaks `text` by character into rows of at most `max` cells.
pub fn chunk(text: &str, max: u16) -> Vec<String> {
    let max = max.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let w = char_width(ch);
        if used + w > max && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(ch);
        used += w;
    }
    rows
}

/// The longest prefix of `text` that fits in `max` cells.
pub fn prefix(text: &str, max: u16) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = char_width(ch);
        if used + w > max {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Shortens `text` to `max` cells, ending in "…" when cut.
pub fn truncate(text: &str, max: u16) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = prefix(text, max - 1);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let mut row = String::new();
                for run in line {
                    while width(&row) < run.x {
                        row.push(' ');
                    }
                    row.push_str(&run.text);
                }
                row
            })
            .collect()
    }

    #[test]
    fn wraps_across_spans_and_tracks_tags() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let pieces = [
            Piece::new("changing the mortgage industry with ", Style::new()),
            Piece::tagged("Copperlane (YC W26)", bold, Some(7)),
            Piece::new(" in SF", Style::new()),
        ];
        let lines = wrap(&pieces, 40);
        assert_eq!(
            plain(&lines),
            [
                "changing the mortgage industry with",
                "Copperlane (YC W26) in SF"
            ]
        );
        assert_eq!(lines[1][0].tag, Some(7));
        assert_eq!(lines[1][0].text, "Copperlane (YC W26)");
        assert_eq!(lines[1][1].x, 19);
    }

    #[test]
    fn punctuation_after_a_link_stays_on_its_word() {
        let pieces = [
            Piece::new("aaaa ", Style::new()),
            Piece::tagged("X", Style::new(), Some(0)),
            Piece::new(", b", Style::new()),
        ];
        assert_eq!(plain(&wrap(&pieces, 6)), ["aaaa", "X, b"]);
    }

    #[test]
    fn long_words_break_by_cell_width() {
        let pieces = [Piece::new("ab 漢字漢字x", Style::new())];
        assert_eq!(plain(&wrap(&pieces, 4)), ["ab", "漢字", "漢字", "x"]);
        assert_eq!(chunk("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
