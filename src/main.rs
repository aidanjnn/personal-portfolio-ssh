mod app;
mod content;
mod text;
mod theme;
mod ui;

use crossterm::event::{self, Event};
use std::{
    ffi::OsStr,
    io::{self, IsTerminal, Write},
};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Aidan Jeon's terminal portfolio\n\nUsage:\n  personal-portfolio-tui [--no-links]\n  personal-portfolio-tui --print-content\n\nOptions:\n  --no-links               Disable OSC 8 hyperlinks; readable URLs remain\n\nh w p t: pages | j/k, Tab: move | Enter: open / expand | o / c: show / copy link\nd: theme | ?: keys and credits | Esc: back | q / Ctrl+C: quit"
        );
        return Ok(());
    }
    let content = content::Portfolio::bundled()?;
    if args == ["--print-content"] {
        println!("{}", serde_json::to_string_pretty(&content)?);
        return Ok(());
    }
    let links = match args.as_slice() {
        [] => true,
        [flag] if flag == "--no-links" => false,
        _ => return Err("Unknown argument. Use --help for usage.".into()),
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Run in an interactive terminal, or use --help.".into());
    }
    let mut app = app::App {
        theme: initial_theme(std::env::var_os("NO_COLOR").as_deref()),
        ..Default::default()
    };
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &content, links);
    ratatui::restore();
    result?;
    // The restored terminal translates newlines itself.
    println!("\n{}", ui::goodbye(&content.web));
    Ok(())
}

/// Honours https://no-color.org: any non-empty `NO_COLOR` selects the plain theme.
fn initial_theme(no_color: Option<&OsStr>) -> app::Theme {
    if no_color.is_some_and(|value| !value.is_empty()) {
        app::Theme::Plain
    } else {
        app::Theme::default()
    }
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut app::App,
    content: &content::Portfolio,
    links: bool,
) -> io::Result<()> {
    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, app, content))?;
        if links {
            terminal
                .backend_mut()
                .write_all(ui::link_escapes(app).as_bytes())?;
        }
        terminal
            .backend_mut()
            .write_all(ui::clipboard_escape(app).as_bytes())?;
        terminal.backend_mut().flush()?;
        if let Event::Key(key) = event::read()? {
            app.handle_key(key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_selects_the_plain_theme() {
        assert_eq!(initial_theme(None), app::Theme::Paper);
        assert_eq!(initial_theme(Some(OsStr::new(""))), app::Theme::Paper);
        assert_eq!(initial_theme(Some(OsStr::new("1"))), app::Theme::Plain);
    }
}
