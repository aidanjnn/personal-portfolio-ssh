//! Page state and key handling. Holds indices only; `ui::draw` resolves them against content.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::content::MAX_LIST_ENTRIES;
pub use crate::theme::Theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Home,
    Work,
    Projects,
    Thoughts,
    /// The in-terminal reader for `thoughts[index]`.
    Reader(usize),
}

impl Page {
    const ORDER: [Page; 4] = [Page::Home, Page::Work, Page::Projects, Page::Thoughts];

    fn position(self) -> usize {
        match self {
            Page::Home => 0,
            Page::Work => 1,
            Page::Projects => 2,
            Page::Thoughts | Page::Reader(_) => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// The focused item's URL.
    Link,
    Help,
}

/// A focusable element's rows in page coordinates, captured by the last draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemRows {
    pub top: usize,
    pub bottom: usize,
}

/// A hyperlink as drawn: the cells it covers, re-emitted with OSC 8 by `ui::link_escapes`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hyperlink {
    pub x: u16,
    pub y: u16,
    pub cells: Vec<(String, ratatui::style::Style)>,
    pub url: String,
}

#[derive(Debug)]
pub struct App {
    pub theme: Theme,
    pub quit: bool,
    /// Vertical scroll of the page viewport, in rows.
    pub scroll: usize,
    /// Recomputed by `ui::draw`.
    pub max_scroll: usize,
    pub page: Page,
    pub focus: Option<usize>,
    /// Expanded Work rows, one bit per row; read it with `is_expanded`.
    pub expanded: u64,
    pub sheet: Option<Sheet>,
    /// Scroll the focused item into view on the next draw.
    pub reveal: bool,
    /// `c` was pressed; the next draw resolves the URL into `clipboard`.
    pub copy_requested: bool,
    /// Show "copied ↗ …" in the status bar until the next key.
    pub copied: bool,
    /// A pending OSC 52 write, emitted once by `ui::clipboard_escape`.
    pub clipboard: Option<String>,
    pub hyperlinks: Vec<Hyperlink>,
    /// Layout facts from the last draw, used to resolve keys.
    pub viewport: usize,
    pub items: Vec<ItemRows>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            quit: false,
            scroll: 0,
            max_scroll: 0,
            page: Page::Home,
            // Focus starts on the first focusable of the page.
            focus: Some(0),
            expanded: 0,
            sheet: None,
            reveal: true,
            copy_requested: false,
            copied: false,
            clipboard: None,
            hyperlinks: Vec::new(),
            viewport: 0,
            items: Vec::new(),
        }
    }
}

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let code = key.code;
        if code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        // Chords are not bindings: Ctrl-D must not cycle the theme, Ctrl-Q must not quit.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        self.copied = false;
        if code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        if let Some(sheet) = self.sheet {
            match code {
                KeyCode::Esc => self.sheet = None,
                KeyCode::Enter if sheet == Sheet::Link => self.sheet = None,
                KeyCode::Char('c') if sheet == Sheet::Link => self.copy(),
                KeyCode::Char('d') => self.theme = self.theme.next(),
                KeyCode::Char('?') if sheet == Sheet::Help => self.sheet = None,
                KeyCode::Char('?') => self.sheet = Some(Sheet::Help),
                _ => {}
            }
            return;
        }
        let reader = matches!(self.page, Page::Reader(_));
        let back = code == KeyCode::BackTab
            || (code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
        match code {
            KeyCode::Char('h') => self.enter(Page::Home),
            KeyCode::Char('w') => self.enter(Page::Work),
            KeyCode::Char('p') => self.enter(Page::Projects),
            KeyCode::Char('t') => self.enter(Page::Thoughts),
            KeyCode::Left if reader => self.leave_reader(),
            KeyCode::Left => self.enter(Page::ORDER[(self.page.position() + 3) % 4]),
            KeyCode::Right => self.enter(Page::ORDER[(self.page.position() + 1) % 4]),
            KeyCode::Char('j') | KeyCode::Down if reader => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up if reader => self.scroll_by(-1),
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.move_focus(true) && self.page == Page::Home {
                    self.scroll_by(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.move_focus(false) && self.page == Page::Home {
                    self.scroll_by(-1);
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.move_focus(!back);
            }
            KeyCode::Enter => match self.page {
                Page::Work => {
                    if let Some(index) = self.focus.filter(|&index| index < MAX_LIST_ENTRIES) {
                        self.expanded ^= 1 << index;
                        self.reveal = true;
                    }
                }
                Page::Thoughts => {
                    if let Some(index) = self.focus {
                        self.page = Page::Reader(index);
                        self.focus = None;
                        self.scroll = 0;
                    }
                }
                _ => self.open_link(),
            },
            KeyCode::Char('o') => self.open_link(),
            KeyCode::Char('c') => self.copy(),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(self.page_step()),
            KeyCode::PageUp => self.scroll_by(-self.page_step()),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = self.max_scroll,
            KeyCode::Esc if reader => self.leave_reader(),
            KeyCode::Char('?') => self.sheet = Some(Sheet::Help),
            KeyCode::Char('d') => self.theme = self.theme.next(),
            _ => {}
        }
    }

    /// Whether Work row `index` is expanded.
    pub fn is_expanded(&self, index: usize) -> bool {
        index < MAX_LIST_ENTRIES && self.expanded & (1 << index) != 0
    }

    fn enter(&mut self, page: Page) {
        self.page = page;
        self.focus = Some(0);
        self.scroll = 0;
        self.expanded = 0;
        self.reveal = true;
        // Item rows belong to the previous page until the next draw.
        self.items.clear();
    }

    fn leave_reader(&mut self) {
        if let Page::Reader(index) = self.page {
            self.enter(Page::Thoughts);
            self.focus = Some(index);
        }
    }

    fn page_step(&self) -> isize {
        isize::try_from(self.viewport.saturating_sub(1).max(1)).unwrap_or(isize::MAX)
    }

    fn scroll_by(&mut self, rows: isize) {
        self.scroll = self.scroll.saturating_add_signed(rows).min(self.max_scroll);
    }

    /// Moves focus to the next or previous item. Returns false when there is none.
    fn move_focus(&mut self, forward: bool) -> bool {
        let count = self.items.len();
        if count == 0 {
            // No items, or none drawn yet for this page.
            return false;
        }
        let next = match self.focus {
            Some(index) if forward => (index + 1 < count).then_some(index + 1),
            Some(index) => index.checked_sub(1),
            // Nothing focused: start from what is on screen, the first item from the top of the
            // view down or the last from its bottom up. As with a focused item at either end,
            // there is no wrap-around when nothing lies that way.
            None => {
                let top = self.scroll;
                let bottom = self.scroll.saturating_add(self.viewport);
                if forward {
                    self.items.iter().position(|item| item.top >= top)
                } else {
                    self.items.iter().rposition(|item| item.bottom < bottom)
                }
            }
        };
        match next {
            Some(index) => {
                self.focus = Some(index);
                self.reveal = true;
                true
            }
            None => false,
        }
    }

    fn open_link(&mut self) {
        if self.focus.is_some() {
            self.sheet = Some(Sheet::Link);
        }
    }

    fn copy(&mut self) {
        if self.focus.is_some() {
            self.copy_requested = true;
            self.copied = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn starts_on_home_in_paper() {
        let app = App::default();
        assert_eq!(app.page, Page::Home);
        assert_eq!(app.theme, Theme::Paper);
        assert!(!app.quit);
    }

    #[test]
    fn release_events_are_ignored() {
        let mut app = App::default();
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        app.handle_key(key);
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn arrows_wrap_through_the_pages() {
        let mut app = App::default();
        press(&mut app, KeyCode::Left);
        assert_eq!(app.page, Page::Thoughts);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.page, Page::Home);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.page, Page::Work);
    }

    #[test]
    fn ctrl_c_quits_and_d_cycles_themes() {
        let mut app = App::default();
        for expected in [Theme::Night, Theme::Plain, Theme::Paper] {
            press(&mut app, KeyCode::Char('d'));
            assert_eq!(app.theme, expected);
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }

    #[test]
    fn control_and_alt_chords_are_ignored_except_ctrl_c() {
        let mut app = App::default();
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.theme, Theme::Paper);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
        assert_eq!(app.theme, Theme::Paper);
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
        assert_eq!(app.page, Page::Home);
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(!app.quit);
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::SHIFT));
        assert_eq!(app.theme, Theme::Night);
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }
}
