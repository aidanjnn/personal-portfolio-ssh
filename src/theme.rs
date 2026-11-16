//! Color tokens from aidanjeon.com (app/globals.css), mapped to terminal styles.
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Paper,
    Night,
    Plain,
}

impl Theme {
    pub fn next(self) -> Self {
        match self {
            Self::Paper => Self::Night,
            Self::Night => Self::Plain,
            Self::Plain => Self::Paper,
        }
    }
}

/// The site's design tokens for one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub theme: Theme,
    pub canvas: Color,
    pub ink: Color,
    pub faint: Color,
    pub date: Color,
    pub rule: Color,
    pub accent: Color,
    pub decoration: Color,
    /// Selection tint; `None` means REVERSED (Plain).
    pub focus_bg: Option<Color>,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    pub fn of(theme: Theme) -> Self {
        match theme {
            Theme::Paper => Self {
                theme,
                canvas: rgb(0xffffff),
                ink: rgb(0x32404f),
                faint: rgb(0x7a8794),
                date: rgb(0xa8b2bb),
                rule: rgb(0xe4e7ea),
                accent: rgb(0xf97316),
                decoration: rgb(0xc5ccd3),
                focus_bg: Some(rgb(0xfee6d5)),
            },
            Theme::Night => Self {
                theme,
                canvas: rgb(0x11161b),
                ink: rgb(0xdfe4e8),
                faint: rgb(0x8d99a5),
                date: rgb(0x6b7783),
                rule: rgb(0x262e36),
                accent: rgb(0xfb8a3c),
                decoration: rgb(0x56636f),
                focus_bg: Some(rgb(0x3a2616)),
            },
            Theme::Plain => Self {
                theme,
                canvas: Color::Reset,
                ink: Color::Reset,
                faint: Color::Reset,
                date: Color::DarkGray,
                rule: Color::DarkGray,
                accent: Color::Yellow,
                decoration: Color::DarkGray,
                focus_bg: None,
            },
        }
    }

    /// Every cell starts as ink on canvas.
    pub fn base(&self) -> Style {
        Style::new().fg(self.ink).bg(self.canvas)
    }

    pub fn ink(&self) -> Style {
        Style::new().fg(self.ink)
    }

    pub fn ink_bold(&self) -> Style {
        self.ink().add_modifier(Modifier::BOLD)
    }

    pub fn faint(&self) -> Style {
        let style = Style::new().fg(self.faint);
        if self.theme == Theme::Plain {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub fn date(&self) -> Style {
        Style::new().fg(self.date)
    }

    pub fn rule(&self) -> Style {
        Style::new().fg(self.rule)
    }

    pub fn accent(&self) -> Style {
        Style::new().fg(self.accent)
    }

    pub fn decoration(&self) -> Style {
        Style::new().fg(self.decoration)
    }

    /// Adds an underline. Plain keeps the terminal's own underline color.
    pub fn underline(&self, style: Style, color: Color) -> Style {
        let style = style.add_modifier(Modifier::UNDERLINED);
        if self.theme == Theme::Plain {
            style
        } else {
            style.underline_color(color)
        }
    }

    /// Focus styling: the selection tint (REVERSED in Plain); links swap to an accent underline.
    pub fn focus(&self, style: Style) -> Style {
        match self.focus_bg {
            Some(bg) => {
                let style = style.bg(bg);
                if style.add_modifier.contains(Modifier::UNDERLINED) {
                    style.underline_color(self.accent)
                } else {
                    style
                }
            }
            None => style.add_modifier(Modifier::REVERSED),
        }
    }
}
