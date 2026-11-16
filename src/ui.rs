//! The Page design (direction A): one centered column with a header, a scrolling page,
//! a footer and a status bar, styled with the aidanjeon.com tokens.
mod escapes;

// `base64` stays part of the `ui` API; only tests call it outside `clipboard_escape`.
#[allow(unused_imports)]
pub use escapes::base64;
pub use escapes::{clipboard_escape, link_escapes};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use crate::{
    app::{App, Hyperlink, ItemRows, Page, Sheet},
    content::{Portfolio, SpanStyle},
    text::{self, Line, Piece, Run},
    theme::{Palette, Theme},
};

const MIN_COLS: u16 = 32;
const MIN_ROWS: u16 = 10;
const SEAN_URL: &str = "https://seanyang.ca";
const SOURCE_URL: &str = "https://github.com/aidanjnn/personal-portfolio-ssh";
const COPYRIGHT_YEAR: u16 = 2026;
const TERMINAL_NOTE: &str = "↳ you found the terminal version.";
const CLICK_HINT: &str = "cmd/ctrl-click to open";

/// Where everything sits for one terminal size.
#[derive(Clone, Copy, Debug)]
struct Layout {
    cols: u16,
    rows: u16,
    /// Left margin of the content column.
    m: u16,
    /// Content column width.
    w: u16,
    compact: bool,
    header_y: u16,
    view_top: u16,
    view_h: u16,
    footer_y: Option<u16>,
    status_y: u16,
}

impl Layout {
    fn new(area: Rect) -> Self {
        let (cols, rows) = (area.width, area.height);
        if cols >= 64 && rows >= 16 {
            let w = 66.min(cols - 4);
            Self {
                cols,
                rows,
                m: (cols - w) / 2,
                w,
                compact: false,
                header_y: 1,
                view_top: 3,
                view_h: rows - 7,
                footer_y: Some(rows - 3),
                status_y: rows - 1,
            }
        } else {
            Self {
                cols,
                rows,
                m: 2,
                w: cols - 4,
                compact: true,
                header_y: 0,
                view_top: 2,
                view_h: rows - 4,
                footer_y: None,
                status_y: rows - 1,
            }
        }
    }

    /// Right edge (exclusive) of the content column.
    fn right(&self) -> u16 {
        self.m + self.w
    }
}

/// A focusable element of a page.
#[derive(Clone, Copy, Debug)]
struct Item<'a> {
    top: usize,
    bottom: usize,
    /// Row that carries the "›" marker (list pages).
    marker: Option<usize>,
    url: Option<&'a str>,
    label: &'a str,
}

/// A page's content laid out to the column width, before scrolling.
#[derive(Default)]
struct Doc<'a> {
    lines: Vec<Line>,
    items: Vec<Item<'a>>,
}

impl<'a> Doc<'a> {
    fn blank(&mut self) {
        self.lines.push(Vec::new());
    }

    fn push(&mut self, runs: Line) {
        self.lines.push(runs);
    }

    /// Wraps `pieces` to `width` at column `indent`; returns the first and last row.
    fn wrap(&mut self, pieces: &[Piece], width: u16, indent: u16) -> (usize, usize) {
        let first = self.lines.len();
        for mut line in text::wrap(pieces, width) {
            for run in &mut line {
                run.x += indent;
            }
            self.lines.push(line);
        }
        (first, self.lines.len().saturating_sub(1).max(first))
    }

    /// Rows where runs tagged `tag` landed.
    fn tag_rows(&self, from: usize, tag: usize) -> (usize, usize) {
        let rows: Vec<usize> = (from..self.lines.len())
            .filter(|&row| self.lines[row].iter().any(|run| run.tag == Some(tag)))
            .collect();
        match (rows.first(), rows.last()) {
            (Some(&top), Some(&bottom)) => (top, bottom),
            _ => (from, from),
        }
    }
}

fn run(x: u16, text: impl Into<String>, style: Style) -> Run {
    Run {
        x,
        text: text.into(),
        style,
        tag: None,
    }
}

/// `https://www.copperlane.ai/` → `copperlane.ai`; `mailto:a@b.c` → `a@b.c`.
pub fn display_url(url: &str) -> &str {
    if let Some(address) = url.strip_prefix("mailto:") {
        return address;
    }
    let url = url.strip_prefix("https://").unwrap_or(url);
    let url = url.strip_prefix("www.").unwrap_or(url);
    url.strip_suffix('/').unwrap_or(url)
}

fn without_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("mailto:"))
        .unwrap_or(url)
}

fn footer_line(content: &Portfolio, w: u16, pal: &Palette) -> Line {
    let mut line = vec![run(
        0,
        format!("© {} {COPYRIGHT_YEAR}", content.name),
        pal.faint(),
    )];
    if w >= 5 {
        line.push(run(w - 5, "←", pal.faint()));
        line.push(run(w - 3, "◇", pal.ink()));
        line.push(run(w - 1, "→", pal.faint()));
    }
    line
}

fn span_style(style: SpanStyle, url: bool, pal: &Palette) -> Style {
    let base = match style {
        SpanStyle::Plain => pal.ink(),
        SpanStyle::Strong => pal.ink_bold(),
        SpanStyle::Emphasis => pal.ink_bold().add_modifier(Modifier::ITALIC),
        SpanStyle::Accent => pal.accent().add_modifier(Modifier::BOLD),
        SpanStyle::Highlight => pal.underline(pal.ink_bold(), pal.faint),
    };
    if !url {
        return base;
    }
    let color = if style == SpanStyle::Accent {
        pal.accent
    } else {
        pal.decoration
    };
    pal.underline(base, color)
}

fn home<'a>(content: &'a Portfolio, layout: &Layout, pal: &Palette) -> Doc<'a> {
    let w = layout.w;
    let mut doc = Doc::default();
    if let Some(updated) = &content.updated {
        doc.push(vec![run(0, text::truncate(updated, w), pal.date())]);
        doc.blank();
    }
    for (index, paragraph) in content.intro.iter().enumerate() {
        if index > 0 {
            doc.blank();
        }
        let first_tag = doc.items.len();
        let pieces: Vec<Piece> = paragraph
            .iter()
            .map(|span| {
                let tag = span.url.as_ref().map(|_| {
                    doc.items.push(Item {
                        top: 0,
                        bottom: 0,
                        marker: None,
                        url: span.url.as_deref(),
                        label: &span.text,
                    });
                    doc.items.len() - 1
                });
                Piece::tagged(&span.text, span_style(span.style, tag.is_some(), pal), tag)
            })
            .collect();
        let (from, _) = doc.wrap(&pieces, w, 0);
        for tag in first_tag..doc.items.len() {
            let (top, bottom) = doc.tag_rows(from, tag);
            doc.items[tag].top = top;
            doc.items[tag].bottom = bottom;
        }
    }
    // The note sits mid-way through the free space when the page fits, else after a blank row.
    let reserved = if layout.compact { 2 } else { 0 };
    let used = doc.lines.len();
    let free = usize::from(layout.view_h)
        .saturating_sub(reserved)
        .saturating_sub(used);
    let row = if free >= 2 { used + free / 2 } else { used + 1 };
    while doc.lines.len() < row {
        doc.blank();
    }
    let note_x = w.saturating_sub(text::width(TERMINAL_NOTE)) / 2;
    doc.push(vec![run(note_x, TERMINAL_NOTE, pal.faint())]);
    doc
}

fn work<'a>(content: &'a Portfolio, app: &App, layout: &Layout, pal: &Palette) -> Doc<'a> {
    let w = layout.w;
    let mut doc = Doc::default();
    doc.push(vec![run(0, "Work", pal.ink_bold())]);
    doc.blank();
    for (index, entry) in content.work.iter().enumerate() {
        if index > 0 {
            doc.blank();
        }
        let top = doc.lines.len();
        let expanded = app.is_expanded(index);
        let company = text::truncate(&entry.company, w.saturating_sub(8));
        let company_w = text::width(&company);
        let inline_role = w >= 66 && 6 + company_w + 2 + 2 <= 36;
        let mut row = vec![
            run(0, entry.year.to_string(), pal.date()),
            Run {
                x: 6,
                text: company,
                style: pal.ink(),
                tag: Some(index),
            },
            run(
                6 + company_w,
                if expanded { " ▴" } else { " ▾" },
                pal.faint(),
            ),
        ];
        if inline_role {
            row.push(run(36, text::truncate(&entry.role, w - 36), pal.faint()));
        }
        doc.push(row);
        if !inline_role {
            doc.push(vec![run(
                6,
                text::truncate(&entry.role, w.saturating_sub(6)),
                pal.faint(),
            )]);
        }
        if expanded {
            doc.wrap(
                &[Piece::new(&entry.description, pal.faint())],
                w.saturating_sub(8),
                6,
            );
        }
        doc.items.push(Item {
            top,
            bottom: doc.lines.len() - 1,
            marker: Some(top),
            url: Some(&entry.url),
            label: &entry.company,
        });
    }
    doc
}

/// Projects and the Thoughts index: year, bold title, faint text under it.
fn listing<'a>(
    title: &'static str,
    entries: impl Iterator<Item = (u16, &'a str, Option<&'a str>, &'a str, &'a str)>,
    layout: &Layout,
    pal: &Palette,
) -> Doc<'a> {
    let w = layout.w;
    let mut doc = Doc::default();
    doc.push(vec![run(0, title, pal.ink_bold())]);
    doc.blank();
    for (index, (year, name, event, body, url)) in entries.enumerate() {
        if index > 0 {
            doc.blank();
        }
        let event = event.map(|event| format!(" ({event})"));
        let mut pieces = vec![Piece::tagged(name, pal.ink_bold(), Some(index))];
        if let Some(event) = &event {
            pieces.push(Piece::new(event, pal.faint()));
        }
        let (top, _) = doc.wrap(&pieces, w.saturating_sub(6), 6);
        doc.lines[top].insert(0, run(0, year.to_string(), pal.date()));
        doc.wrap(&[Piece::new(body, pal.faint())], w.saturating_sub(6), 6);
        doc.items.push(Item {
            top,
            bottom: doc.lines.len() - 1,
            marker: Some(top),
            url: Some(url),
            label: name,
        });
    }
    doc
}

fn reader<'a>(content: &'a Portfolio, index: usize, layout: &Layout, pal: &Palette) -> Doc<'a> {
    let w = layout.w;
    let thought = &content.thoughts[index];
    let mut doc = Doc::default();
    doc.push(vec![run(0, thought.year.to_string(), pal.date())]);
    doc.wrap(&[Piece::new(&thought.title, pal.ink_bold())], w, 0);
    doc.wrap(&[Piece::new(&thought.subtitle, pal.ink())], w, 0);
    doc.blank();
    for section in &thought.sections {
        doc.wrap(&[Piece::new(&section.heading, pal.ink_bold())], w, 0);
        for item in &section.items {
            match &item.url {
                Some(url) => {
                    let tag = Some(doc.items.len());
                    let style = pal.underline(pal.ink(), pal.decoration);
                    let pieces = [
                        Piece::tagged(&item.text, style, tag),
                        Piece::tagged(" ", style, tag),
                        Piece::tagged(without_scheme(url), style, tag),
                    ];
                    let (top, bottom) = doc.wrap(&pieces, w.saturating_sub(4), 2);
                    doc.items.push(Item {
                        top,
                        bottom,
                        marker: None,
                        url: Some(url),
                        label: &item.text,
                    });
                }
                None => {
                    doc.wrap(&[Piece::new(&item.text, pal.ink())], w.saturating_sub(4), 2);
                }
            }
        }
        doc.blank();
    }
    doc
}

fn build<'a>(content: &'a Portfolio, app: &App, layout: &Layout, pal: &Palette) -> Doc<'a> {
    let mut doc = match app.page {
        Page::Home => home(content, layout, pal),
        Page::Work => work(content, app, layout, pal),
        Page::Projects => listing(
            "Projects",
            content.projects.iter().map(|p| {
                (
                    p.year,
                    p.title.as_str(),
                    p.event.as_deref(),
                    p.summary.as_str(),
                    p.url.as_str(),
                )
            }),
            layout,
            pal,
        ),
        Page::Thoughts => listing(
            "Thoughts",
            content.thoughts.iter().map(|t| {
                (
                    t.year,
                    t.title.as_str(),
                    None,
                    t.subtitle.as_str(),
                    t.url.as_str(),
                )
            }),
            layout,
            pal,
        ),
        Page::Reader(index) => reader(content, index, layout, pal),
    };
    if layout.compact {
        if doc.lines.last().is_some_and(|line| !line.is_empty()) {
            doc.blank();
        }
        doc.push(footer_line(content, layout.w, pal));
    }
    doc
}

/// Writes `text` from `x`, clipped before column `limit`. Returns the column after the text.
fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, limit: u16) -> u16 {
    let area = buf.area;
    let limit = limit.min(area.right());
    if y < area.top() || y >= area.bottom() {
        return x;
    }
    let mut cx = x;
    // The cell holding the last glyph written, which zero-width marks join.
    let mut last = None;
    for ch in text.chars() {
        let w = text::char_width(ch);
        if w == 0 {
            if let Some(lx) = last {
                let cell = &mut buf[(lx, y)];
                let symbol = format!("{}{ch}", cell.symbol());
                cell.set_symbol(&symbol);
            }
            continue;
        }
        if cx < area.left() || cx + w > limit {
            break;
        }
        last = Some(cx);
        buf[(cx, y)].set_char(ch).set_style(style);
        for k in 1..w {
            let cell = &mut buf[(cx + k, y)];
            cell.reset();
            cell.set_style(style);
        }
        cx += w;
    }
    cx
}

fn clear(buf: &mut Buffer, area: Rect, base: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_style(base);
        }
    }
}

/// A square-cornered box in `style` with " {title} " on the top border at x+2.
fn draw_box(buf: &mut Buffer, area: Rect, title: &str, style: Style, pal: &Palette) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    clear(buf, area, pal.base());
    let (left, right) = (area.left(), area.right() - 1);
    let (top, bottom) = (area.top(), area.bottom() - 1);
    for x in left + 1..right {
        put(buf, x, top, "─", style, area.right());
        put(buf, x, bottom, "─", style, area.right());
    }
    for y in top + 1..bottom {
        put(buf, left, y, "│", style, area.right());
        put(buf, right, y, "│", style, area.right());
    }
    put(buf, left, top, "┌", style, area.right());
    put(buf, right, top, "┐", style, area.right());
    put(buf, left, bottom, "└", style, area.right());
    put(buf, right, bottom, "┘", style, area.right());
    let title = text::truncate(&format!(" {title} "), area.width.saturating_sub(4));
    put(buf, left + 2, top, &title, pal.ink_bold(), right);
}

/// Birds in the right gutter at fixed offsets from the column edge (rows 2…26 of the mockup).
const BIRDS: [(u16, u16, &str); 15] = [
    (2, 7, "\\_/"),
    (4, 12, "v"),
    (5, 4, "\\/"),
    (7, 9, "-v-"),
    (9, 5, "v"),
    (10, 13, "\\/"),
    (12, 3, "\\_/"),
    (14, 10, "v"),
    (16, 6, "\\/"),
    (18, 13, "v"),
    (20, 3, "\\_/"),
    (21, 11, "v"),
    (23, 7, "-v-"),
    (25, 4, "\\/"),
    (26, 13, "v"),
];

fn draw_birds(buf: &mut Buffer, layout: &Layout, pal: &Palette) {
    if pal.theme == Theme::Plain || layout.cols < 96 || layout.compact {
        return;
    }
    let edge = layout.right();
    for y in 2..=layout.rows.saturating_sub(4) {
        // The pattern repeats every 25 rows on taller terminals.
        let key = 2 + (y - 2) % 25;
        let Some(&(_, offset, glyph)) = BIRDS.iter().find(|(row, ..)| *row == key) else {
            continue;
        };
        let len = text::width(glyph);
        let x = (edge + offset).min(layout.cols.saturating_sub(len));
        if x > edge + 1 {
            put(buf, x, y, glyph, pal.decoration(), layout.cols);
        }
    }
}

fn draw_header(buf: &mut Buffer, layout: &Layout, content: &Portfolio, app: &App, pal: &Palette) {
    let gap = if layout.compact { 2 } else { 4 };
    let limit = layout.right();
    let y = layout.header_y;
    let mut x = put(buf, layout.m, y, &content.name, pal.ink_bold(), limit);
    let active = match (app.sheet, app.page) {
        (Some(Sheet::Help), _) | (_, Page::Home) => None,
        (_, Page::Work) => Some(0),
        (_, Page::Projects) => Some(1),
        (_, Page::Thoughts | Page::Reader(_)) => Some(2),
    };
    for (index, label) in ["996", "Projects", "Thoughts"].into_iter().enumerate() {
        // A label is shown whole or not at all; past a missing active label, none follow.
        if x + gap + text::width(label) > limit {
            if active == Some(index) {
                break;
            }
            continue;
        }
        let style = if active == Some(index) {
            pal.underline(pal.ink(), pal.decoration)
        } else {
            pal.faint()
        };
        x = put(buf, x + gap, y, label, style, limit);
    }
}

fn draw_line(
    buf: &mut Buffer,
    x0: u16,
    y: u16,
    line: &Line,
    focus: Option<usize>,
    limit: u16,
    pal: &Palette,
) {
    for run in line {
        let style = if run.tag.is_some() && run.tag == focus {
            pal.focus(run.style)
        } else {
            run.style
        };
        put(buf, x0 + run.x, y, &run.text, style, limit);
    }
}

fn draw_footer(buf: &mut Buffer, layout: &Layout, content: &Portfolio, pal: &Palette) {
    if let Some(y) = layout.footer_y {
        let line = footer_line(content, layout.w, pal);
        draw_line(buf, layout.m, y, &line, None, layout.right(), pal);
    }
}

fn draw_scrollbar(buf: &mut Buffer, layout: &Layout, app: &App, total: usize, pal: &Palette) {
    let x = layout.right() + 1;
    if app.max_scroll == 0 || x >= layout.cols || layout.view_h == 0 {
        return;
    }
    let view = usize::from(layout.view_h);
    let thumb = (view * view / total.max(1)).clamp(1, view);
    let pos = (app.scroll * (view - thumb) + app.max_scroll / 2) / app.max_scroll;
    for row in 0..view {
        let (glyph, style) = if (pos..pos + thumb).contains(&row) {
            ("┃", pal.faint())
        } else {
            ("│", pal.rule())
        };
        put(
            buf,
            x,
            layout.view_top + row as u16,
            glyph,
            style,
            layout.cols,
        );
    }
}

type Segments = Vec<(String, Style)>;

fn segments_width(segments: &Segments) -> u16 {
    segments.iter().map(|(text, _)| text::width(text)).sum()
}

/// Cuts `segments` to `max` cells, ending in "…" when anything was dropped.
fn truncate_segments(segments: Segments, max: u16) -> Segments {
    if segments_width(&segments) <= max {
        return segments;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for (text, style) in segments {
        let w = text::width(&text);
        if used + w < max {
            used += w;
            out.push((text, style));
        } else {
            let mut cut = text::prefix(&text, max - used - 1);
            cut.push('…');
            out.push((cut, style));
            break;
        }
    }
    out
}

/// The status bar. `left` (the focused URL) may be cut with "…"; `hints` are whole items
/// that drop from the end when they don't fit; `right_short` replaces `right` before any
/// hint is dropped.
struct Status {
    left: Segments,
    hints: Vec<Segments>,
    right: Segments,
    right_short: Option<Segments>,
}

fn status<'a>(
    app: &App,
    doc: &Doc<'a>,
    layout: &Layout,
    focused: Option<&Item<'a>>,
    pal: &Palette,
) -> Status {
    let date = |text: &str| (text.to_string(), pal.date());
    let keys = if layout.compact {
        "? keys  q quit"
    } else {
        "? keys  ·  q quit"
    };
    let copied = |url: &str| {
        vec![
            date("copied "),
            ("↗ ".to_string(), pal.faint()),
            (display_url(url).to_string(), pal.ink()),
        ]
    };
    let focused_url = focused.and_then(|item| item.url);
    match app.sheet {
        Some(Sheet::Help) => {
            return Status {
                left: Vec::new(),
                hints: vec![vec![date("esc close")]],
                right: vec![date("q quit")],
                right_short: None,
            };
        }
        Some(Sheet::Link) => {
            let (left, hints) = match focused_url {
                Some(url) if app.copied => (copied(url), Vec::new()),
                _ => (
                    Vec::new(),
                    if layout.compact {
                        vec![
                            vec![date("esc close")],
                            vec![("c".to_string(), pal.ink_bold()), date(" copy")],
                        ]
                    } else {
                        vec![
                            vec![date("↵/esc close")],
                            vec![("c".to_string(), pal.ink_bold()), date(" copy link")],
                        ]
                    },
                ),
            };
            return Status {
                left,
                hints,
                right: vec![date(keys)],
                right_short: None,
            };
        }
        None => {}
    }
    let (mut left, hints): (Segments, &[&str]) = match focused_url {
        Some(url) if app.copied => (copied(url), &[]),
        Some(url) => {
            let hints: &[&str] = match app.page {
                Page::Work => {
                    let open = app.focus.is_some_and(|index| app.is_expanded(index));
                    if open {
                        &["↵ collapse", "o link"]
                    } else {
                        &["↵ expand", "o link"]
                    }
                }
                Page::Thoughts => &["↵ read"],
                _ => &["↵ open", "c copy"],
            };
            let left = vec![
                ("↗ ".to_string(), pal.faint()),
                (display_url(url).to_string(), pal.ink()),
            ];
            (left, hints)
        }
        None if matches!(app.page, Page::Reader(_)) => {
            (Vec::new(), &["j k scroll", "esc back to Thoughts"])
        }
        None => (Vec::new(), &[]),
    };
    let hints = hints.iter().map(|hint| vec![date(hint)]).collect();
    let more = app.scroll < app.max_scroll;
    let mut right = Vec::new();
    if more && !layout.compact {
        let prefix = match app.page {
            Page::Reader(_) => {
                let shown = (app.scroll + usize::from(layout.view_h)) * 100;
                format!("↓ {}%  ·  ", (shown / doc.lines.len().max(1)).min(100))
            }
            _ => "↓ more  ·  ".to_string(),
        };
        right.push(date(&prefix));
    }
    if more && layout.compact && left.is_empty() {
        left.push(("↓ more".to_string(), pal.faint()));
    }
    let wide_home = app.page == Page::Home && !layout.compact && layout.w >= 66;
    let mut right_short = None;
    if wide_home {
        let mut short = right.clone();
        short.push(date(keys));
        right_short = Some(short);
        right.push(date("h w p t  ·  ? keys  ·  q quit"));
    } else {
        right.push(date(keys));
    }
    Status {
        left,
        hints,
        right,
        right_short,
    }
}

fn draw_status(buf: &mut Buffer, layout: &Layout, status: Status) {
    const GAP: u16 = 3;
    let (m, w, y) = (layout.m, layout.w, layout.status_y);
    let Status {
        left,
        hints,
        right,
        right_short,
    } = status;
    // How many hints fit beside `right`, and the left side that leaves room for them.
    let fit = |right: &Segments| {
        let right = truncate_segments(right.clone(), w);
        let budget = w.saturating_sub(segments_width(&right)).saturating_sub(2);
        let left = if left.is_empty() || budget == 0 {
            Vec::new()
        } else {
            truncate_segments(left.clone(), budget)
        };
        let mut used = segments_width(&left);
        let mut shown = 0;
        for hint in &hints {
            let gap = if used == 0 { 0 } else { GAP };
            let hint_w = segments_width(hint);
            if used + gap + hint_w > budget {
                break;
            }
            used += gap + hint_w;
            shown += 1;
        }
        (left, shown, right)
    };
    let mut chosen = fit(&right);
    if let Some(short) = &right_short
        && chosen.1 < hints.len()
    {
        chosen = fit(short);
    }
    let (left, shown, right) = chosen;
    let mut x = m;
    for (text, style) in &left {
        x = put(buf, x, y, text, *style, m + w);
    }
    for hint in &hints[..shown] {
        if x > m {
            x += GAP;
        }
        for (text, style) in hint {
            x = put(buf, x, y, text, *style, m + w);
        }
    }
    let mut x = m + w - segments_width(&right);
    for (text, style) in &right {
        x = put(buf, x, y, text, *style, m + w);
    }
}

/// The line printed after the portfolio closes, locally or over SSH.
pub fn goodbye(web: &str) -> String {
    format!("↳ thanks for stopping by  {web}")
}

/// A region to re-emit with OSC 8 once the frame is complete.
struct LinkRegion {
    x: u16,
    y: u16,
    width: u16,
    url: String,
}

fn draw_link_sheet(
    buf: &mut Buffer,
    layout: &Layout,
    item: &Item,
    url: &str,
    pal: &Palette,
    links: &mut Vec<LinkRegion>,
) -> u16 {
    let inner = layout.w.saturating_sub(4).max(1);
    let mut rows = text::chunk(url, inner);
    if rows.len() > 4 {
        rows.truncate(4);
        rows[3] = text::prefix(&rows[3], inner - 1) + "…";
    }
    let height = rows.len() as u16 + 2;
    let bottom = layout.rows.saturating_sub(2);
    let top = bottom.saturating_sub(height - 1);
    let area = Rect::new(layout.m, top, layout.w, height).intersection(buf.area);
    draw_box(buf, area, item.label, pal.rule(), pal);
    let style = pal.underline(pal.ink(), pal.decoration);
    let x = layout.m + 2;
    let limit = x + inner;
    for (index, row) in rows.iter().enumerate() {
        let y = top + 1 + index as u16;
        let end = put(buf, x, y, row, style, limit);
        if end > x {
            links.push(LinkRegion {
                x,
                y,
                width: end - x,
                url: url.to_string(),
            });
        }
        if index + 1 == rows.len() {
            let hint_w = text::width(CLICK_HINT);
            if text::width(row) + 2 + hint_w <= inner {
                put(buf, limit - hint_w, y, CLICK_HINT, pal.date(), limit);
            }
        }
    }
    top
}

struct HelpSeg {
    x: u16,
    text: String,
    style: Style,
    url: Option<&'static str>,
}

fn help_lines(content: &Portfolio, inner: u16, pal: &Palette) -> Vec<Vec<HelpSeg>> {
    let seg = |x: u16, text: &str, style: Style| HelpSeg {
        x,
        text: text.to_string(),
        style,
        url: None,
    };
    let key = |x: u16, k: &str, label: &str| {
        vec![
            seg(x, k, pal.ink_bold()),
            seg(x + text::width(k) + 2, label, pal.faint()),
        ]
    };
    let column = |k: &str, label: &str| vec![seg(0, k, pal.ink_bold()), seg(9, label, pal.faint())];
    let wide = inner >= 46;
    let mut lines: Vec<Vec<HelpSeg>> = Vec::new();
    if wide {
        lines.push(Vec::new());
        for ((k1, l1), (k2, l2)) in [
            (("h w p t", "pages"), ("← →", "prev / next")),
            (("j k ↑ ↓", "move"), ("↵", "open · expand")),
            (("esc", "back"), ("o c", "show · copy link")),
        ] {
            let mut line = column(k1, l1);
            line.extend(key(24, k2, l2));
            lines.push(line);
        }
        lines.push(column("d", "theme: paper / night / plain"));
        lines.push(column("q", "quit"));
        lines.push(Vec::new());
    } else {
        for (k, label) in [
            ("h w p t", "pages"),
            ("← →", "prev / next"),
            ("j k ↑ ↓", "move"),
            ("↵", "open · expand"),
            ("esc", "back"),
            ("o c", "show · copy link"),
            ("d", "theme"),
            ("q", "quit"),
        ] {
            lines.push(column(k, label));
        }
    }
    lines.push(vec![seg(0, &"─".repeat(usize::from(inner)), pal.rule())]);
    let link = pal.underline(pal.ink(), pal.decoration);
    let sean = |x: u16| HelpSeg {
        x,
        text: "Sean Yang".into(),
        style: link,
        url: Some(SEAN_URL),
    };
    let source = |x: u16| HelpSeg {
        x,
        text: "source".into(),
        style: link,
        url: Some(SOURCE_URL),
    };
    // Narrow boxes break the credits before the links rather than cutting them.
    if inner >= 37 {
        lines.push(vec![
            seg(0, "SSH idea from ", pal.faint()),
            sean(14),
            seg(23, " · seanyang.ca", pal.faint()),
        ]);
    } else {
        lines.push(vec![seg(0, "SSH idea from", pal.faint())]);
        lines.push(vec![sean(0), seg(9, " · seanyang.ca", pal.faint())]);
    }
    if inner >= 35 {
        lines.push(vec![
            seg(0, "built with ratatui + russh · ", pal.faint()),
            source(29),
        ]);
    } else {
        lines.push(vec![seg(0, "built with ratatui + russh", pal.faint())]);
        lines.push(vec![source(0)]);
    }
    if wide {
        lines.push(Vec::new());
    }
    let copyright = format!("© {} {COPYRIGHT_YEAR}", content.name);
    let mut last = vec![seg(0, &copyright, pal.faint())];
    let ring = "← ◇ se-webring →";
    let ring_w = text::width(ring);
    if text::width(&copyright) + 2 + ring_w <= inner {
        let x = inner - ring_w;
        last.push(seg(x, "←", pal.faint()));
        last.push(seg(x + 2, "◇", pal.ink()));
        last.push(seg(x + 3, " se-webring →", pal.faint()));
    }
    lines.push(last);
    lines
}

fn draw_help(
    buf: &mut Buffer,
    layout: &Layout,
    content: &Portfolio,
    pal: &Palette,
    links: &mut Vec<LinkRegion>,
) {
    // Dim the page underneath, keeping its characters.
    for cell in buf.content.iter_mut() {
        cell.fg = pal.date;
        cell.bg = pal.canvas;
        cell.modifier = Modifier::empty();
        cell.underline_color = Color::Reset;
    }
    let width = 52.min(layout.cols.saturating_sub(4));
    let height = 16.min(layout.rows.saturating_sub(1));
    let x = (layout.cols - width) / 2;
    let y = (layout.rows - height) / 2;
    let area = Rect::new(x, y, width, height);
    draw_box(buf, area, "keys", pal.faint(), pal);
    let inner = width.saturating_sub(6);
    let (x0, limit) = (x + 3, x + 3 + inner);
    let mut lines = help_lines(content, inner, pal);
    let available = usize::from(height.saturating_sub(2));
    if lines.len() > available {
        // Short terminals lose keymap rows first: the credits are a repo rule.
        let rule = lines
            .iter()
            .position(|line| line.first().is_some_and(|seg| seg.text.starts_with('─')))
            .unwrap_or(0);
        let excess = (lines.len() - available).min(rule);
        lines.drain(rule - excess..rule);
    }
    for (index, line) in lines.iter().enumerate() {
        let row = y + 1 + index as u16;
        if row + 1 >= y + height {
            break;
        }
        for seg in line {
            let start = x0 + seg.x;
            let end = put(buf, start, row, &seg.text, seg.style, limit);
            if let Some(url) = seg.url.filter(|_| end > start) {
                links.push(LinkRegion {
                    x: start,
                    y: row,
                    width: end - start,
                    url: url.to_string(),
                });
            }
        }
    }
}

fn draw_too_small(buf: &mut Buffer, area: Rect, pal: &Palette) {
    let message = format!("Resize to at least {MIN_COLS}×{MIN_ROWS}");
    let message = text::truncate(&message, area.width);
    let x = area.width.saturating_sub(text::width(&message)) / 2;
    put(buf, x, area.height / 2, &message, pal.faint(), area.width);
}

/// Lays out the current page for `area` and brings the app's layout facts, focus, scroll,
/// sheet and pending copy up to date.
/// Returns `None` when the terminal is too small to show the page.
fn settle<'a>(
    app: &mut App,
    content: &'a Portfolio,
    area: Rect,
    pal: &Palette,
) -> Option<(Layout, Doc<'a>)> {
    // Content may have shrunk since the indices were stored.
    if let Page::Reader(index) = app.page {
        if content.thoughts.is_empty() {
            app.page = Page::Thoughts;
            app.focus = None;
        } else if index >= content.thoughts.len() {
            app.page = Page::Reader(content.thoughts.len() - 1);
        }
    }

    if area.width < MIN_COLS || area.height < MIN_ROWS {
        app.max_scroll = 0;
        app.scroll = 0;
        app.viewport = 0;
        app.items.clear();
        app.copy_requested = false;
        // Nothing was resolved into the clipboard, so nothing may claim it was copied.
        app.copied = false;
        return None;
    }
    let layout = Layout::new(area);
    let doc = build(content, app, &layout, pal);
    app.items = doc
        .items
        .iter()
        .map(|item| ItemRows {
            top: item.top,
            bottom: item.bottom,
        })
        .collect();
    app.focus = match app.focus {
        _ if doc.items.is_empty() => None,
        Some(index) => Some(index.min(doc.items.len() - 1)),
        None => None,
    };
    app.viewport = usize::from(layout.view_h);
    let view = usize::from(layout.view_h);
    app.max_scroll = doc.lines.len().saturating_sub(view);
    let revealed = app.reveal;
    if app.reveal {
        app.reveal = false;
        if let Some(item) = app.focus.map(|index| doc.items[index]) {
            if item.top < app.scroll || item.bottom - item.top >= view {
                app.scroll = item.top;
            } else if item.bottom >= app.scroll + view {
                app.scroll = item.bottom + 1 - view;
            }
        }
    }
    app.scroll = app.scroll.min(app.max_scroll);
    let scroll = app.scroll;
    // In the Reader, a link scrolled fully out of view is no longer focused.
    if let (Page::Reader(_), Some(index), false) = (app.page, app.focus, revealed) {
        let item = doc.items[index];
        if item.bottom < scroll || item.top >= scroll + view {
            app.focus = None;
        }
    }
    let focused = app.focus.map(|index| doc.items[index]);
    if focused.and_then(|item| item.url).is_none() && app.sheet == Some(Sheet::Link) {
        app.sheet = None;
    }
    if std::mem::take(&mut app.copy_requested) {
        match focused.and_then(|item| item.url) {
            Some(url) => app.clipboard = Some(url.to_string()),
            None => app.copied = false,
        }
    }
    Some((layout, doc))
}

pub fn draw(frame: &mut Frame, app: &mut App, content: &Portfolio) {
    let area = frame.area();
    let pal = Palette::of(app.theme);
    let buf = frame.buffer_mut();
    clear(buf, area, pal.base());
    app.hyperlinks.clear();
    let Some((layout, doc)) = settle(app, content, area, &pal) else {
        draw_too_small(buf, area, &pal);
        return;
    };
    let view = usize::from(layout.view_h);
    let scroll = app.scroll;
    let focused = app.focus.map(|index| doc.items[index]);

    draw_birds(buf, &layout, &pal);
    draw_header(buf, &layout, content, app, &pal);
    let mut links = Vec::new();
    for row in 0..view {
        let Some(line) = doc.lines.get(scroll + row) else {
            break;
        };
        let y = layout.view_top + row as u16;
        draw_line(buf, layout.m, y, line, app.focus, layout.right(), &pal);
        if focused.is_some_and(|item| item.marker == Some(scroll + row)) && layout.m >= 2 {
            put(buf, layout.m - 2, y, "›", pal.ink_bold(), layout.m);
        }
        if let (Some(index), Some(url)) = (app.focus, focused.and_then(|item| item.url)) {
            let tagged = line.iter().filter(|run| run.tag == Some(index));
            let start = tagged.clone().map(|run| run.x).min();
            let end = tagged.map(|run| run.x + text::width(&run.text)).max();
            if let (Some(start), Some(end)) = (start, end) {
                let x = layout.m + start;
                let end = (layout.m + end).min(layout.right());
                if end > x {
                    links.push(LinkRegion {
                        x,
                        y,
                        width: end - x,
                        url: url.to_string(),
                    });
                }
            }
        }
    }
    if matches!(app.page, Page::Reader(_)) {
        draw_scrollbar(buf, &layout, app, doc.lines.len(), &pal);
    }
    draw_footer(buf, &layout, content, &pal);
    let status_line = status(app, &doc, &layout, focused.as_ref(), &pal);
    match (app.sheet, focused) {
        (Some(Sheet::Link), Some(item)) => {
            let mut sheet_links = Vec::new();
            let url = item.url.unwrap_or_default();
            let top = draw_link_sheet(buf, &layout, &item, url, &pal, &mut sheet_links);
            links.retain(|link| link.y < top);
            links.extend(sheet_links);
        }
        (Some(Sheet::Help), _) => {
            links.clear();
            draw_help(buf, &layout, content, &pal, &mut links);
        }
        _ => {}
    }
    draw_status(buf, &layout, status_line);

    app.hyperlinks = links
        .into_iter()
        .map(|link| Hyperlink {
            x: link.x,
            y: link.y,
            cells: (link.x..link.x + link.width)
                .map(|x| {
                    let cell = &buf[(x, link.y)];
                    (cell.symbol().to_string(), cell.style())
                })
                .collect(),
            url: link.url,
        })
        .collect();
}

#[cfg(test)]
mod tests;
