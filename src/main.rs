mod app;
mod content;
mod input;
mod ssh;
mod store;
mod text;
mod theme;
mod ui;

use crossterm::event::{self, Event};
use std::{
    ffi::OsStr,
    io::{self, IsTerminal, Read, Write},
    time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Aidan Jeon's terminal portfolio\n\nUsage:\n  personal-portfolio-tui [--no-links]\n  personal-portfolio-tui --print-content\n  personal-portfolio-tui --check-content FILE|-\n  personal-portfolio-tui serve [OPTIONS]\n\nServer options:\n  --bind ADDRESS           Listen address (default 127.0.0.1:2222)\n  --host-key PATH          Persistent Ed25519 key (default data/ssh_host_ed25519_key)\n  --max-sessions COUNT     Concurrent connections, 1–256 (default 32)\n  --max-per-ip COUNT       Concurrent connections per client address, 1–32 (default 3)\n  --idle-timeout SECS      Input inactivity timeout, 1–3600 (default 300)\n  --max-session-secs SECS  Hard session length, 60–86400 (default 1800)\n  --content-url URL        HTTPS portfolio.json to refresh from once a minute\n  --no-links               Disable OSC 8 hyperlinks; readable URLs remain\n\nAnonymous SSH only runs the portfolio. PTY required; no commands or forwarding.\nh w p t: pages | j/k, Tab: move | Enter: open / expand | o / c: show / copy link\nd: theme | ?: keys and credits | Esc: back | q / Ctrl+C: quit"
        );
        return Ok(());
    }
    if let [flag, path] = args.as_slice()
        && flag == "--check-content"
    {
        return match read_snapshot(path).and_then(|raw| check_content(&raw)) {
            Ok(summary) => {
                println!("{summary}");
                Ok(())
            }
            Err(error) => {
                eprintln!("invalid portfolio content: {error}");
                std::process::exit(1);
            }
        };
    }
    let content = content::Portfolio::bundled()?;
    if args == ["--print-content"] {
        println!("{}", serde_json::to_string_pretty(&content)?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "serve") {
        let config = parse_server_args(&args[1..])?;
        return tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(ssh::serve(config, content));
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

/// Reads a snapshot from a file, or stdin for `-`, stopping one byte past the size cap.
fn read_snapshot(path: &str) -> Result<Vec<u8>, Error> {
    let limit = content::MAX_SNAPSHOT_BYTES as u64 + 1;
    let mut raw = Vec::new();
    if path == "-" {
        io::stdin().take(limit).read_to_end(&mut raw)?;
    } else {
        std::fs::File::open(path)?
            .take(limit)
            .read_to_end(&mut raw)?;
    }
    Ok(raw)
}

/// Validates a snapshot exactly as the server does before serving fetched content.
/// The website's CI runs this against the `/portfolio.json` it is about to deploy.
fn check_content(raw: &[u8]) -> Result<String, Error> {
    if raw.len() > content::MAX_SNAPSHOT_BYTES {
        return Err(format!("larger than {} bytes", content::MAX_SNAPSHOT_BYTES).into());
    }
    let json = std::str::from_utf8(raw).map_err(|_| "not UTF-8")?;
    let portfolio = content::Portfolio::from_json(json)?;
    Ok(format!(
        "portfolio.json OK: v{}, {} bytes, {} work, {} projects, {} thoughts",
        portfolio.version,
        raw.len(),
        portfolio.work.len(),
        portfolio.projects.len(),
        portfolio.thoughts.len()
    ))
}

fn parse_server_args(args: &[String]) -> Result<ssh::Config, Error> {
    let mut config = ssh::Config::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--no-links" {
            config.links = false;
            continue;
        }
        let value = args
            .next()
            .ok_or("Missing server option value; use --help.")?;
        match arg.as_str() {
            "--bind" => config.bind = value.parse()?,
            "--host-key" => config.host_key = value.into(),
            "--max-sessions" => config.max_sessions = value.parse()?,
            "--max-per-ip" => config.max_per_ip = value.parse()?,
            "--idle-timeout" => config.idle_timeout = Duration::from_secs(value.parse()?),
            "--max-session-secs" => config.max_session = Duration::from_secs(value.parse()?),
            "--content-url" => {
                content::validate_https_url("--content-url", value)?;
                config.content_url = Some(value.clone());
            }
            _ => return Err("Unknown server option; use --help.".into()),
        }
    }
    if !(1..=256).contains(&config.max_sessions) {
        return Err("--max-sessions must be 1–256".into());
    }
    if !(1..=32).contains(&config.max_per_ip) {
        return Err("--max-per-ip must be 1–32".into());
    }
    if !(1..=3600).contains(&config.idle_timeout.as_secs()) {
        return Err("--idle-timeout must be 1–3600 seconds".into());
    }
    if !(60..=86400).contains(&config.max_session.as_secs()) {
        return Err("--max-session-secs must be 60–86400 seconds".into());
    }
    Ok(config)
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
    fn server_config_defaults_to_localhost_and_rejects_unbounded_limits() {
        let config = parse_server_args(&[]).unwrap();
        assert_eq!(config.bind.to_string(), "127.0.0.1:2222");
        assert_eq!(config.max_per_ip, 3);
        assert_eq!(config.max_session, Duration::from_secs(1800));
        assert_eq!(config.content_url, None);
        for args in [
            ["--max-sessions", "0"],
            ["--max-sessions", "257"],
            ["--idle-timeout", "0"],
            ["--idle-timeout", "3601"],
            ["--max-per-ip", "0"],
            ["--max-per-ip", "33"],
            ["--max-per-ip", "-1"],
            ["--max-session-secs", "59"],
            ["--max-session-secs", "86401"],
        ] {
            assert!(parse_server_args(&args.map(String::from)).is_err());
        }
    }

    #[test]
    fn server_config_accepts_limit_bounds() {
        for (flag, value) in [
            ("--max-per-ip", "1"),
            ("--max-per-ip", "32"),
            ("--max-session-secs", "60"),
            ("--max-session-secs", "86400"),
        ] {
            assert!(parse_server_args(&[flag.into(), value.into()]).is_ok());
        }
    }

    #[test]
    fn server_config_accepts_only_https_content_urls() {
        let args = ["--content-url", "https://www.aidanjeon.com/portfolio.json"];
        let config = parse_server_args(&args.map(String::from)).unwrap();
        assert_eq!(
            config.content_url.as_deref(),
            Some("https://www.aidanjeon.com/portfolio.json")
        );
        for url in [
            "http://www.aidanjeon.com/portfolio.json",
            "mailto:hello@aidanjeon.com",
            "https://localhost/portfolio.json",
            "https://www.aidanjeon.com/a b",
            "https://user@www.aidanjeon.com/",
            "file:///etc/passwd",
            "",
        ] {
            let error = parse_server_args(&["--content-url".into(), url.into()])
                .err()
                .unwrap_or_else(|| panic!("accepted {url:?}"))
                .to_string();
            assert!(error.contains("--content-url"), "{error}");
            assert!(!error.contains("mailto"), "{error}");
        }
    }

    #[test]
    fn check_content_accepts_the_bundled_snapshot_and_rejects_bad_input() {
        let bundled = include_bytes!("../content/portfolio.json");
        assert!(
            check_content(bundled)
                .unwrap()
                .starts_with("portfolio.json OK: v2")
        );
        let mut json: serde_json::Value = serde_json::from_slice(bundled).unwrap();
        json["work"][0]["logo"] = "x".into();
        assert!(check_content(json.to_string().as_bytes()).is_err());
        json["work"][0].as_object_mut().unwrap().remove("logo");
        json["work"][0]["url"] = "/work".into();
        assert!(check_content(json.to_string().as_bytes()).is_err());
        assert!(check_content(&[0xff, 0xfe]).is_err());
        assert!(check_content(&vec![b' '; content::MAX_SNAPSHOT_BYTES + 1]).is_err());
    }

    #[test]
    fn no_color_selects_the_plain_theme() {
        assert_eq!(initial_theme(None), app::Theme::Paper);
        assert_eq!(initial_theme(Some(OsStr::new(""))), app::Theme::Paper);
        assert_eq!(initial_theme(Some(OsStr::new("1"))), app::Theme::Plain);
    }
}
