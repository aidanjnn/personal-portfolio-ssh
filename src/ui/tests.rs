use super::*;
use crate::content::PortfolioData;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

pub(crate) fn content() -> Portfolio {
    Portfolio::bundled().unwrap()
}

/// The bundled content after `edit`, validated again.
pub(crate) fn edited(edit: impl FnOnce(&mut PortfolioData)) -> Portfolio {
    let mut data = content().into_data();
    edit(&mut data);
    Portfolio::new(data).unwrap()
}

/// Renders `app` once at `cols`×`rows`.
pub(crate) fn render(app: &mut App, content: &Portfolio, cols: u16, rows: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    terminal.draw(|frame| draw(frame, app, content)).unwrap();
    terminal.backend().buffer().clone()
}

/// Presses `keys` one at a time with a draw after each, like the transport does.
pub(crate) fn scenario(keys: &[KeyCode], cols: u16, rows: u16) -> (App, Buffer) {
    let content = content();
    let mut app = App::default();
    let mut buffer = render(&mut app, &content, cols, rows);
    for &code in keys {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        buffer = render(&mut app, &content, cols, rows);
    }
    (app, buffer)
}

/// The buffer as text, one string per row, trailing spaces trimmed.
pub(crate) fn dump(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut skip = 0;
            for x in 0..area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                skip = text::width(symbol).saturating_sub(1);
                row.push_str(symbol);
            }
            row.trim_end().to_string()
        })
        .collect()
}

/// The seven mockup scenarios: name, saved file, keys, size.
const SCENARIOS: [(&str, &str, &[KeyCode], u16, u16); 7] = [
    (
        "home",
        "page-home-100x30.txt",
        &[KeyCode::Char('j')],
        100,
        30,
    ),
    (
        "work",
        "page-work-80x24.txt",
        &[KeyCode::Char('w'), KeyCode::Enter],
        80,
        24,
    ),
    (
        "projects",
        "page-projects-link-80x24.txt",
        &[KeyCode::Char('p'), KeyCode::Char('j'), KeyCode::Enter],
        80,
        24,
    ),
    (
        "reader",
        "page-reader-80x24.txt",
        &[KeyCode::Char('t'), KeyCode::Enter],
        80,
        24,
    ),
    ("help", "page-help-80x24.txt", &[KeyCode::Char('?')], 80, 24),
    (
        "night",
        "page-home-night-80x24.txt",
        &[KeyCode::Char('d')],
        80,
        24,
    ),
    ("compact", "page-home-40x16.txt", &[], 40, 16),
];

fn frame(name: &str) -> Vec<String> {
    let (_, _, keys, cols, rows) = SCENARIOS.iter().find(|s| s.0 == name).unwrap();
    dump(&scenario(keys, *cols, *rows).1)
}

/// Frame `index` of docs/screens/page-mockups.md, one string per row.
fn mockup(index: usize) -> Vec<String> {
    let md = include_str!("../../docs/screens/page-mockups.md");
    let block = md.split("```text\n").nth(index + 1).unwrap();
    let block = &block[..block.find("```").unwrap()];
    block
        .lines()
        .map(|row| row.trim_end().to_string())
        .collect()
}

fn assert_rows(name: &str, actual: &[String], expected: &[String], rows: &[usize]) {
    for &row in rows {
        assert_eq!(actual[row], expected[row], "{name} row {row}");
    }
}

fn cell(buffer: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
    &buffer[(x, y)]
}

/// Column of the first occurrence of `needle` in row `y`.
fn find(buffer: &Buffer, y: u16, needle: &str) -> u16 {
    let row = dump(buffer)[usize::from(y)].clone();
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    text::width(&row[..byte])
}

pub(crate) fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

// Golden frames against the mockups. Rows whose copy changed in content/portfolio.json
// (the Updated date, the 5th intro paragraph, the Copperlane description) are left out.

#[test]
fn golden_home_100x30_with_copperlane_focused() {
    let rows: Vec<usize> = (0..30).filter(|row| ![3, 19, 20].contains(row)).collect();
    assert_rows("home", &frame("home"), &mockup(0), &rows);
}

#[test]
fn golden_work_80x24_with_copperlane_expanded() {
    let rows: Vec<usize> = (0..24).filter(|row| ![7, 8].contains(row)).collect();
    assert_rows("work", &frame("work"), &mockup(1), &rows);
}

#[test]
fn golden_projects_80x24_with_the_otto_link_sheet() {
    let rows: Vec<usize> = (0..24).collect();
    assert_rows("projects", &frame("projects"), &mockup(2), &rows);
}

#[test]
fn golden_reader_80x24() {
    let actual = frame("reader");
    let expected = mockup(3);
    assert_rows("reader", &actual, &expected, &[0, 1, 2]);
    // The page column matches; the scrollbar sits at m+W+1 = 74 over the viewport rows.
    for row in 3..20 {
        let column: String = actual[row].chars().take(73).collect();
        let mock: String = expected[row].chars().take(73).collect();
        assert_eq!(column.trim_end(), mock.trim_end(), "reader row {row}");
        let bar = actual[row].chars().nth(74);
        assert!(matches!(bar, Some('┃' | '│')), "reader row {row}: {bar:?}");
    }
    assert_eq!(actual[21], mockup(1)[21], "footer");
    assert!(actual[23].starts_with("       j k scroll   esc back to Thoughts"));
    assert!(actual[23].ends_with("%  ·  ? keys  ·  q quit"));
}

#[test]
fn golden_help_80x24() {
    let rows: Vec<usize> = (0..24).filter(|row| ![3, 19].contains(row)).collect();
    assert_rows("help", &frame("help"), &mockup(4), &rows);
}

#[test]
fn golden_night_80x24_with_software_engineering_focused() {
    let rows: Vec<usize> = (0..23).filter(|row| ![3, 19].contains(row)).collect();
    let actual = frame("night");
    assert_rows("night", &actual, &mockup(5), &rows);
    assert_eq!(
        actual[23],
        "       ↗ uwaterloo.ca/engineering   ↵ open   ↓ more  ·  ? keys  ·  q quit"
    );
}

#[test]
fn golden_compact_40x16() {
    let actual = frame("compact");
    let expected = mockup(6);
    assert_rows("compact", &actual, &expected, &[0, 1, 14]);
    // The Updated label (kept per the spec) shifts the prose down two rows.
    let updated = content().updated.clone().unwrap();
    assert_eq!(actual[2], format!("  {updated}"));
    for row in 2..=11 {
        assert_eq!(actual[row + 2], expected[row], "compact row {row}");
    }
    assert_eq!(actual[15], "  ↗ uwaterloo.ca/engi…  ? keys  q quit");
}

/// Plain-text renderings for reviewers, kept in docs/screens/. Regenerate with
/// `UPDATE_SCREENS=1 cargo test --locked saved_screens`.
#[test]
fn saved_screens_match_the_renderer() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screens");
    for (name, file, ..) in SCENARIOS {
        let text = frame(name).join("\n") + "\n";
        let path = dir.join(file);
        if std::env::var_os("UPDATE_SCREENS").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(saved, text, "{file} is stale; rerun with UPDATE_SCREENS=1");
    }
}

// Styles.

#[test]
fn paper_styles_follow_the_site_tokens() {
    let pal = Palette::of(Theme::Paper);
    let (_, buffer) = scenario(&[], 100, 30);
    // Canvas on every cell, name in ink bold, inactive nav faint.
    assert_eq!(cell(&buffer, 0, 0).bg, pal.canvas);
    assert_eq!(cell(&buffer, 17, 1).fg, pal.ink);
    assert!(cell(&buffer, 17, 1).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&buffer, find(&buffer, 1, "996"), 1).fg, pal.faint);
    assert_eq!(cell(&buffer, 17, 3).fg, pal.date);
    // Copperlane: accent + bold, accent underline, unfocused.
    let x = find(&buffer, 9, "Copperlane");
    let copper = cell(&buffer, x, 9);
    assert_eq!(copper.fg, pal.accent);
    assert!(
        copper
            .modifier
            .contains(Modifier::BOLD | Modifier::UNDERLINED)
    );
    assert_eq!(copper.underline_color, pal.accent);
    assert_eq!(copper.bg, pal.canvas);
    // Focused link: Software Engineering has the tint and an accent underline.
    let x = find(&buffer, 5, "Software");
    let focused = cell(&buffer, x, 5);
    assert_eq!(focused.bg, pal.focus_bg.unwrap());
    assert_eq!(focused.underline_color, pal.accent);
    // Emphasis: University of Waterloo is bold italic ink.
    let x = find(&buffer, 5, "University");
    assert!(
        cell(&buffer, x, 5)
            .modifier
            .contains(Modifier::BOLD | Modifier::ITALIC)
    );
    // Highlight: bold with a faint underline.
    let x = find(&buffer, 13, "seek");
    let seek = cell(&buffer, x, 13);
    assert!(
        seek.modifier
            .contains(Modifier::BOLD | Modifier::UNDERLINED)
    );
    assert_eq!(seek.underline_color, pal.faint);
    // Birds in decoration, footer diamond in ink.
    assert_eq!(cell(&buffer, 90, 2).fg, pal.decoration);
    assert_eq!(cell(&buffer, find(&buffer, 27, "◇"), 27).fg, pal.ink);
}

#[test]
fn focused_rows_tint_only_the_title_and_get_a_marker() {
    let pal = Palette::of(Theme::Paper);
    let (_, buffer) = scenario(&[KeyCode::Char('w')], 80, 24);
    let company = cell(&buffer, 13, 5);
    assert_eq!(company.symbol(), "C");
    assert_eq!(company.bg, pal.focus_bg.unwrap());
    assert_eq!(cell(&buffer, 7, 5).bg, pal.canvas, "year is not tinted");
    assert_eq!(cell(&buffer, 43, 5).bg, pal.canvas, "role is not tinted");
    assert_eq!(cell(&buffer, 43, 5).fg, pal.faint);
    let marker = cell(&buffer, 5, 5);
    assert_eq!(marker.symbol(), "›");
    assert!(marker.modifier.contains(Modifier::BOLD));
    assert_eq!(cell(&buffer, 13, 10).bg, pal.canvas, "unfocused row");
    // The active nav label is ink + underlined.
    let nav = cell(&buffer, find(&buffer, 1, "996"), 1);
    assert_eq!(nav.fg, pal.ink);
    assert!(nav.modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn night_and_plain_themes() {
    let night = Palette::of(Theme::Night);
    let (_, buffer) = scenario(&[KeyCode::Char('d')], 80, 24);
    assert_eq!(cell(&buffer, 0, 0).bg, night.canvas);
    assert_eq!(cell(&buffer, 7, 1).fg, night.ink);
    let x = find(&buffer, 5, "Software");
    assert_eq!(cell(&buffer, x, 5).bg, night.focus_bg.unwrap());

    let (_, buffer) = scenario(
        &[KeyCode::Char('d'), KeyCode::Char('d'), KeyCode::Char('w')],
        80,
        24,
    );
    assert_eq!(cell(&buffer, 0, 0).bg, Color::Reset);
    let company = cell(&buffer, 13, 5);
    assert!(company.modifier.contains(Modifier::REVERSED));
    assert_eq!(company.bg, Color::Reset);
    assert!(
        cell(&buffer, 43, 5).modifier.contains(Modifier::DIM),
        "faint is DIM"
    );
    assert_eq!(cell(&buffer, 7, 5).fg, Color::DarkGray, "date");
    let (_, buffer) = scenario(&[KeyCode::Char('d'), KeyCode::Char('d')], 100, 30);
    assert_eq!(cell(&buffer, 90, 2).symbol(), " ", "no birds in Plain");
}

#[test]
fn help_keeps_the_credits_on_short_terminals() {
    for (cols, rows) in [(32, 10), (40, 12), (80, 16), (100, 30)] {
        let (app, buffer) = scenario(&[KeyCode::Char('?')], cols, rows);
        let text = dump(&buffer).join("\n");
        assert!(text.contains("SSH idea from"), "{cols}x{rows}\n{text}");
        assert!(text.contains("Sean Yang"), "{cols}x{rows}\n{text}");
        assert!(link_escapes(&app).contains("\x1b]8;;https://seanyang.ca\x1b\\"));
    }
}

#[test]
fn help_dims_the_page_and_styles_its_keys() {
    let pal = Palette::of(Theme::Paper);
    let (_, buffer) = scenario(&[KeyCode::Char('?')], 80, 24);
    let page = cell(&buffer, 7, 1);
    assert_eq!(page.symbol(), "A");
    assert_eq!(page.fg, pal.date);
    assert!(page.modifier.is_empty());
    assert_eq!(cell(&buffer, 14, 4).fg, pal.faint, "box border");
    let title = cell(&buffer, 17, 4);
    assert_eq!(title.symbol(), "k");
    assert!(title.modifier.contains(Modifier::BOLD));
    assert!(
        cell(&buffer, 17, 6).modifier.contains(Modifier::BOLD),
        "key"
    );
    assert_eq!(cell(&buffer, 26, 6).fg, pal.faint, "label");
    let sean = cell(&buffer, find(&buffer, 13, "Sean"), 13);
    assert!(sean.modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn active_nav_and_help_divider_use_the_quiet_tokens() {
    let pal = Palette::of(Theme::Paper);
    let (_, buffer) = scenario(&[KeyCode::Char('w')], 80, 24);
    let nav = cell(&buffer, find(&buffer, 1, "996"), 1);
    assert_eq!(nav.fg, pal.ink);
    assert!(nav.modifier.contains(Modifier::UNDERLINED));
    assert_eq!(nav.underline_color, pal.decoration);
    let (_, help) = scenario(&[KeyCode::Char('?')], 80, 24);
    assert_eq!(cell(&help, 17, 12).symbol(), "─");
    assert_eq!(cell(&help, 17, 12).fg, pal.rule, "divider");
}

// Behavior.

#[test]
fn focus_traversal_keeps_the_focused_item_visible() {
    let content = content();
    for (page, cols, rows) in [('h', 64, 16), ('w', 40, 16), ('p', 40, 12), ('t', 32, 10)] {
        let mut app = App::default();
        render(&mut app, &content, cols, rows);
        key(&mut app, KeyCode::Char(page));
        render(&mut app, &content, cols, rows);
        for code in [KeyCode::Char('j'), KeyCode::Tab, KeyCode::Down]
            .repeat(4)
            .into_iter()
            .chain([KeyCode::Char('k'), KeyCode::BackTab, KeyCode::Up].repeat(4))
        {
            key(&mut app, code);
            render(&mut app, &content, cols, rows);
            if let Some(index) = app.focus {
                let item = app.items[index];
                let (top, bottom) = (app.scroll, app.scroll + app.viewport);
                let tall = item.bottom - item.top >= app.viewport;
                assert!(
                    item.top >= top,
                    "{page} {cols}x{rows}: {item:?} above {top}"
                );
                assert!(
                    tall || item.bottom < bottom,
                    "{page}: {item:?} below {bottom}"
                );
            }
            assert!(app.scroll <= app.max_scroll);
        }
    }
}

#[test]
fn home_scrolls_past_the_last_link_with_j() {
    let content = content();
    let mut app = App::default();
    render(&mut app, &content, 40, 12);
    let links = app.items.len();
    for _ in 0..links - 1 {
        key(&mut app, KeyCode::Char('j'));
        render(&mut app, &content, 40, 12);
    }
    assert_eq!(app.focus, Some(links - 1));
    let before = app.scroll;
    key(&mut app, KeyCode::Char('j'));
    render(&mut app, &content, 40, 12);
    assert_eq!(app.focus, Some(links - 1));
    assert_eq!(app.scroll, (before + 1).min(app.max_scroll));
}

#[test]
fn end_scrolls_to_the_bottom_on_every_page() {
    let content = content();
    for page in ['h', 'w', 'p', 't'] {
        let mut app = App::default();
        render(&mut app, &content, 40, 10);
        key(&mut app, KeyCode::Char(page));
        if page == 'w' || page == 't' {
            // Expand a row; open the reader (the Thoughts index fits on screen).
            key(&mut app, KeyCode::Enter);
        }
        render(&mut app, &content, 40, 10);
        key(&mut app, KeyCode::End);
        render(&mut app, &content, 40, 10);
        assert!(app.max_scroll > 0, "{page}");
        assert_eq!(app.scroll, app.max_scroll, "{page}");
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.scroll, 0);
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.scroll, (app.viewport - 1).min(app.max_scroll));
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll, 0);
    }
}

#[test]
fn work_rows_toggle() {
    let content = content();
    let mut app = App::default();
    key(&mut app, KeyCode::Char('w'));
    let collapsed = dump(&render(&mut app, &content, 80, 24));
    assert!(collapsed[5].contains("Copperlane (YC W26) ▾"));
    key(&mut app, KeyCode::Enter);
    let expanded = dump(&render(&mut app, &content, 80, 24));
    assert!(expanded[5].contains("▴"));
    assert!(expanded[6].contains("intelligence layer"));
    assert!(expanded[23].contains("↵ collapse   o link"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(dump(&render(&mut app, &content, 80, 24)), collapsed);
    assert!(collapsed[23].contains("↵ expand   o link"));
}

#[test]
fn sheets_open_and_close() {
    let content = content();
    let mut app = App::default();
    key(&mut app, KeyCode::Char('p'));
    render(&mut app, &content, 80, 24);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sheet, Some(Sheet::Link));
    // Only sheet keys act while it is open.
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::Char('w'));
    assert_eq!(app.page, Page::Projects);
    assert_eq!(app.focus, Some(0));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.sheet, None);
    key(&mut app, KeyCode::Char('o'));
    assert_eq!(app.sheet, Some(Sheet::Link));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sheet, None);
    key(&mut app, KeyCode::Char('?'));
    assert_eq!(app.sheet, Some(Sheet::Help));
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.theme, Theme::Night);
    key(&mut app, KeyCode::Char('?'));
    assert_eq!(app.sheet, None);
    key(&mut app, KeyCode::Char('?'));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.sheet, None);
    key(&mut app, KeyCode::Esc);
    assert_eq!(
        app.page,
        Page::Projects,
        "Esc does nothing outside the Reader"
    );
    key(&mut app, KeyCode::Char('?'));
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn reader_opens_scrolls_and_returns() {
    let content = content();
    let mut app = App::default();
    key(&mut app, KeyCode::Char('t'));
    render(&mut app, &content, 80, 24);
    key(&mut app, KeyCode::Enter);
    render(&mut app, &content, 80, 24);
    assert_eq!(app.page, Page::Reader(0));
    assert_eq!(app.focus, None);
    key(&mut app, KeyCode::Char('j'));
    render(&mut app, &content, 80, 24);
    assert_eq!(app.scroll, 1);
    key(&mut app, KeyCode::Tab);
    let buffer = render(&mut app, &content, 80, 24);
    let spotify = app.items[app.focus.unwrap()];
    assert!(spotify.top >= app.scroll && spotify.bottom < app.scroll + app.viewport);
    assert!(dump(&buffer)[23].contains("↗ open.spotify.com"));
    assert!(dump(&buffer)[1].contains("Thoughts"));
    key(&mut app, KeyCode::Left);
    render(&mut app, &content, 80, 24);
    assert_eq!(app.page, Page::Thoughts);
    assert_eq!(app.focus, Some(0));
    assert_eq!(app.scroll, 0);
}

#[test]
fn leaving_a_page_resets_focus_and_scroll() {
    let content = content();
    let mut app = App::default();
    key(&mut app, KeyCode::Char('p'));
    render(&mut app, &content, 40, 12);
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::Char('j'));
    render(&mut app, &content, 40, 12);
    assert!(app.scroll > 0);
    key(&mut app, KeyCode::Char('h'));
    key(&mut app, KeyCode::Char('p'));
    render(&mut app, &content, 40, 12);
    assert_eq!((app.focus, app.scroll), (Some(0), 0));
}

// Hyperlinks and clipboard.

#[test]
fn a_copy_resolved_while_too_small_is_not_reported_as_copied() {
    let content = content();
    let mut app = App::default();
    render(&mut app, &content, 100, 30);
    key(&mut app, KeyCode::Char('j'));
    render(&mut app, &content, 100, 30);
    key(&mut app, KeyCode::Char('c'));
    assert!(app.copied);
    render(&mut app, &content, 20, 8);
    assert!(!app.copied);
    assert_eq!(clipboard_escape(&mut app), "");
    let buffer = render(&mut app, &content, 100, 30);
    assert!(!dump(&buffer)[29].contains("copied"));
}

#[test]
fn sheets_carry_their_links() {
    let (app, _) = scenario(
        &[KeyCode::Char('p'), KeyCode::Char('j'), KeyCode::Enter],
        80,
        24,
    );
    let escapes = link_escapes(&app);
    assert!(escapes.contains("\x1b7\x1b[22;10H"), "url row");
    assert!(escapes.contains("\x1b]8;;https://github.com/abrj7/otto\x1b\\"));
    let (app, _) = scenario(&[KeyCode::Char('?')], 80, 24);
    let escapes = link_escapes(&app);
    assert!(escapes.contains("\x1b]8;;https://seanyang.ca\x1b\\"));
    assert!(escapes.contains("\x1b]8;;https://github.com/aidanjnn/personal-portfolio-ssh\x1b\\"));
    assert!(
        !escapes.contains("uwaterloo"),
        "the dimmed page link is not linked"
    );
}

#[test]
fn long_urls_wrap_in_the_link_sheet() {
    let content = edited(|data| {
        data.projects[0].url = format!("https://example.com/{}", "a".repeat(400));
    });
    let mut app = App::default();
    key(&mut app, KeyCode::Char('p'));
    render(&mut app, &content, 80, 24);
    key(&mut app, KeyCode::Enter);
    let rows = dump(&render(&mut app, &content, 80, 24));
    assert!(rows[17].starts_with("       ┌─ teachAR "));
    assert!(rows[21].ends_with("a… │"));
    assert!(rows[22].starts_with("       └"));
    // Four URL rows in the sheet, plus the focused title still visible above it.
    assert_eq!(app.hyperlinks.iter().filter(|link| link.y >= 18).count(), 4);
    assert_eq!(app.hyperlinks.len(), 5);
    assert!(
        app.hyperlinks
            .iter()
            .all(|link| link.url == content.projects[0].url)
    );
}

// Robustness.

fn every_state() -> Vec<Vec<KeyCode>> {
    use KeyCode::*;
    vec![
        vec![],
        vec![Char('w'), Enter],
        vec![Char('p'), Enter],
        vec![Char('t')],
        vec![Char('t'), Enter],
        vec![Char('t'), Enter, Tab, Enter],
        vec![Char('?')],
        vec![Char('d'), Char('d'), Char('w'), Enter, Char('o')],
    ]
}

#[test]
fn every_page_renders_at_every_size() {
    let sizes = [
        (100, 30),
        (80, 24),
        (64, 16),
        (63, 20),
        (40, 16),
        (32, 10),
        (31, 9),
        (20, 5),
    ];
    for keys in every_state() {
        for (cols, rows) in sizes {
            let (app, buffer) = scenario(&keys, cols, rows);
            assert!(app.scroll <= app.max_scroll);
            if cols < 32 || rows < 10 {
                let text = dump(&buffer).join("\n");
                assert!(
                    text.contains("Resize to at least") || cols < 24,
                    "{cols}x{rows}"
                );
            }
        }
    }
    let rows = dump(&scenario(&[], 31, 9).1);
    assert_eq!(rows[4].trim(), "Resize to at least 32×10");
}

#[test]
fn random_keys_and_resizes_never_panic() {
    use rand::{RngExt, SeedableRng, rngs::StdRng};
    let content = content();
    let mut rng = StdRng::seed_from_u64(0x5eed);
    let codes = [
        KeyCode::Char('h'),
        KeyCode::Char('w'),
        KeyCode::Char('p'),
        KeyCode::Char('t'),
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('o'),
        KeyCode::Char('c'),
        KeyCode::Char('d'),
        KeyCode::Char('?'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Char(' '),
        KeyCode::Char('x'),
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ];
    let mut app = App::default();
    let (mut cols, mut rows) = (80, 24);
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    for step in 0..5000 {
        if rng.random_range(0..20) == 0 {
            cols = rng.random_range(1..=160);
            rows = rng.random_range(1..=60);
            terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        }
        let code = codes[rng.random_range(0..codes.len())];
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        app.quit = false;
        terminal
            .draw(|frame| draw(frame, &mut app, &content))
            .unwrap();
        let _ = link_escapes(&app);
        assert!(app.scroll <= app.max_scroll, "step {step}");
    }
}

#[test]
fn status_never_overlaps_at_40_columns_with_the_longest_url() {
    let content = content();
    let longest = content
        .work
        .iter()
        .map(|w| w.url.len())
        .enumerate()
        .max_by_key(|(_, len)| *len)
        .unwrap()
        .0;
    let mut app = App::default();
    key(&mut app, KeyCode::Char('w'));
    render(&mut app, &content, 40, 16);
    for _ in 0..longest {
        key(&mut app, KeyCode::Char('j'));
        render(&mut app, &content, 40, 16);
    }
    for extra in [None, Some(KeyCode::Char('o')), Some(KeyCode::Char('c'))] {
        if let Some(code) = extra {
            key(&mut app, code);
        }
        let row = dump(&render(&mut app, &content, 40, 16))[15].clone();
        let right = "? keys  q quit";
        assert!(row.ends_with(right), "{row:?}");
        let chars: Vec<char> = row.chars().collect();
        let right_start = chars.len() - right.chars().count();
        assert!(
            chars[right_start - 2..right_start]
                .iter()
                .all(|ch| *ch == ' '),
            "{row:?}"
        );
        match extra {
            Some(KeyCode::Char('o')) => {
                assert!(row.starts_with("  esc close   c copy "), "{row:?}");
            }
            _ => assert!(row.contains('…'), "{row:?}"),
        }
    }
}

#[test]
fn status_hints_drop_whole_and_never_end_in_an_ellipsis() {
    let content = content();
    for (cols, rows) in [(100, 30), (80, 24), (66, 20)] {
        for page in ['h', 'w', 'p', 't'] {
            let mut app = App::default();
            key(&mut app, KeyCode::Char(page));
            for _ in 0..12 {
                let row =
                    dump(&render(&mut app, &content, cols, rows))[usize::from(rows) - 1].clone();
                for cut in ["ope…", "cop…", "exp…", "col…", "lin…", "rea…"] {
                    assert!(!row.contains(cut), "{cols}x{rows} {page}: {row:?}");
                }
                key(&mut app, KeyCode::Char('j'));
            }
        }
    }
    // A long URL on the wide home falls back to the short key list before dropping a hint.
    let mut app = App::default();
    let row = dump(&render(&mut app, &content, 100, 30))[29].clone();
    assert!(
        row.contains("↗ uwaterloo.ca/engineering   ↵ open   c copy"),
        "{row:?}"
    );
    assert!(row.trim_end().ends_with("? keys  ·  q quit"), "{row:?}");
}

#[test]
#[ignore = "debug helper: cargo test print_frames -- --ignored --nocapture"]
fn print_frames() {
    use KeyCode::*;
    for (name, keys, cols, rows) in [
        ("home 100x30", vec![Char('j')], 100, 30),
        ("work 80x24", vec![Char('w'), Enter], 80, 24),
        ("projects 80x24", vec![Char('p'), Char('j'), Enter], 80, 24),
        ("reader 80x24", vec![Char('t'), Enter], 80, 24),
        ("help 80x24", vec![Char('?')], 80, 24),
        ("night 80x24", vec![Char('d')], 80, 24),
        ("compact 40x16", vec![], 40, 16),
        ("work 64x16", vec![Char('w'), Enter], 64, 16),
        ("projects sheet 40x16", vec![Char('p'), Enter], 40, 16),
        ("help 40x16", vec![Char('?')], 40, 16),
        ("help 32x10", vec![Char('?')], 32, 10),
        ("reader 63x20", vec![Char('t'), Enter, Tab], 63, 20),
        ("thoughts 100x30 night", vec![Char('d'), Char('t')], 100, 30),
    ] {
        let (_, buffer) = scenario(&keys, cols, rows);
        println!("--- {name}");
        for (index, row) in dump(&buffer).iter().enumerate() {
            println!("{index:2}|{row}");
        }
    }
}

#[test]
fn combining_marks_join_the_previous_cell() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
    let end = put(&mut buf, 1, 0, "Jose\u{301}!", Style::default(), 10);
    assert_eq!(end, 6);
    assert_eq!(buf[(4, 0)].symbol(), "e\u{301}");
    assert_eq!(buf[(5, 0)].symbol(), "!");
    assert_eq!(dump(&buf), [" Jose\u{301}!"]);
    // A mark after a wide glyph lands on the glyph, not its continuation cell.
    let end = put(&mut buf, 0, 0, "日\u{308}x", Style::default(), 10);
    assert_eq!(end, 3);
    assert_eq!(buf[(0, 0)].symbol(), "日\u{308}");
    // A leading mark has nothing to join and is dropped.
    assert_eq!(put(&mut buf, 0, 0, "\u{301}a", Style::default(), 10), 1);
    assert_eq!(buf[(0, 0)].symbol(), "a");
}

#[test]
fn compact_header_drops_nav_labels_that_do_not_fit_whole() {
    let content = content();
    for cols in [32, 34, 38] {
        for page in [KeyCode::Char('h'), KeyCode::Char('t')] {
            let mut app = App::default();
            key(&mut app, page);
            let header = dump(&render(&mut app, &content, cols, 16))[0].clone();
            assert_eq!(header, "  Aidan Jeon  996  Projects", "{cols} columns");
        }
    }
    let mut app = App::default();
    let header = dump(&render(&mut app, &content, 39, 16))[0].clone();
    assert_eq!(header, "  Aidan Jeon  996  Projects  Thoughts");
}
