//! Cell-accurate word wrapping of styled spans that remembers where each tagged span landed.
//! Text is measured by grapheme cluster, the unit a terminal cell holds.
use ratatui::{buffer::CellWidth, style::Style};
use unicode_segmentation::UnicodeSegmentation;

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

/// Cells one grapheme cluster occupies, measured exactly as ratatui's buffer diff measures a
/// cell's symbol, so a drawn cluster and the cells the terminal is told to skip agree
/// (emoji + U+FE0F and ZWJ sequences are 2 cells, a letter with combining marks is 1).
pub fn cluster_width(cluster: &str) -> u16 {
    // Validated content has no control characters; never trip ratatui's debug assertion on one.
    if cluster.len() == 1 && cluster.as_bytes()[0].is_ascii_control() {
        return 0;
    }
    cluster.cell_width()
}

/// The grapheme clusters of `text` with their widths.
pub fn clusters(text: &str) -> impl Iterator<Item = (&str, u16)> {
    text.graphemes(true)
        .map(|cluster| (cluster, cluster_width(cluster)))
}

pub fn width(text: &str) -> u16 {
    clusters(text)
        .map(|(_, width)| usize::from(width))
        .sum::<usize>()
        .min(usize::from(u16::MAX)) as u16
}

#[derive(Clone, Copy)]
struct Cell<'a> {
    text: &'a str,
    width: u16,
    style: Style,
    tag: Option<usize>,
}

/// Word-wraps `pieces` to `max` cells. Only spaces break; longer words break by cluster.
pub fn wrap(pieces: &[Piece], max: u16) -> Vec<Line> {
    let max = max.max(1);
    let cells: Vec<Cell> = pieces
        .iter()
        .flat_map(|piece| {
            clusters(piece.text).map(move |(text, width)| Cell {
                text,
                width,
                style: piece.style,
                tag: piece.tag,
            })
        })
        .collect();
    let mut lines: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut used = 0u16;
    let mut index = 0;
    while index < cells.len() {
        if cells[index].text == " " {
            index += 1;
            continue;
        }
        // Spaces before this word, then the word itself.
        let mut gap_start = index;
        while gap_start > 0 && cells[gap_start - 1].text == " " {
            gap_start -= 1;
        }
        let mut end = index;
        while end < cells.len() && cells[end].text != " " {
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
            Some(run) if run.style == cell.style && run.tag == cell.tag => {
                run.text.push_str(cell.text)
            }
            _ => runs.push(Run {
                x,
                text: cell.text.to_string(),
                style: cell.style,
                tag: cell.tag,
            }),
        }
        x += cell.width;
    }
    runs
}

/// Breaks `text` by cluster into rows of at most `max` cells.
pub fn chunk(text: &str, max: u16) -> Vec<String> {
    let max = max.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for (cluster, w) in clusters(text) {
        if used + w > max && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().unwrap().push_str(cluster);
        used += w;
    }
    rows
}

/// The longest prefix of `text` that fits in `max` cells.
pub fn prefix(text: &str, max: u16) -> String {
    let mut out = String::new();
    let mut used = 0;
    for (cluster, w) in clusters(text) {
        if used + w > max {
            break;
        }
        out.push_str(cluster);
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

    #[test]
    fn clusters_are_measured_whole() {
        let heart = "\u{2764}\u{fe0f}"; // ❤️: a 1-cell character made 2 cells by VS16
        let coder = "👩\u{200d}💻";
        assert_eq!(width(heart), 2);
        assert_eq!(width(coder), 2);
        assert_eq!(width("👋🏽"), 2);
        assert_eq!(width("Jose\u{301}"), 4);
        assert_eq!(
            width("\u{2764}\u{fe0e}"),
            1,
            "text presentation stays narrow"
        );
        // Wrapping, chunking and truncation never split a cluster.
        let text = format!("{heart}{heart}{coder}e\u{301}");
        assert_eq!(chunk(&text, 3), [heart, heart, &format!("{coder}e\u{301}")]);
        assert_eq!(prefix(&text, 3), heart);
        assert_eq!(truncate(&text, 4), format!("{heart}…"));
        let lines = wrap(&[Piece::new(&format!("ab {text}"), Style::new())], 4);
        assert_eq!(
            plain(&lines),
            [
                "ab",
                &format!("{heart}{heart}"),
                &format!("{coder}e\u{301}")
            ]
        );
    }
}
