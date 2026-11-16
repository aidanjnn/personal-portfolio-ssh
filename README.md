# ssh aidanjeon.com

[aidanjeon.com](https://www.aidanjeon.com/) in your terminal, served over SSH. It shows the same nav, copy, columns, footer and color tokens as the site, drawn in terminal cells with Rust, [Ratatui](https://ratatui.rs/) and [russh](https://docs.rs/russh/).

**Not public yet.** The server listens on loopback by default. [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) is the launch runbook.

Rendered frames: [home](docs/screens/page-home-100x30.txt), [996](docs/screens/page-work-80x24.txt), [projects](docs/screens/page-projects-link-80x24.txt), [compact](docs/screens/page-home-40x16.txt).

## Run it

Install Rust 1.89+ with [rustup](https://rustup.rs/).

```sh
cargo run --locked                 # in this terminal
cargo run --locked -- serve        # SSH server on 127.0.0.1:2222
ssh -p 2222 localhost              # from another terminal
```

The first connection shows the normal host-key prompt. The key is created at `data/ssh_host_ed25519_key` (outside Git); keep it across restarts. A session runs only the portfolio: there's no shell, file transfer, or forwarding. `cargo run -- --help` lists every server flag, including `--content-url`, `--max-per-ip` and `--max-session-secs`.

## Keys

| Keys | Action |
| --- | --- |
| `h` `w` `p` `t` | Home, 996, Projects, Thoughts |
| `←` `→` | Previous / next page |
| `j` `k` `↑` `↓` `Tab` | Move focus; scroll in the reader |
| `↵` | Open a link sheet, expand a Work row, or open a Thought |
| `o` · `c` | Show the focused URL · copy it to your clipboard (OSC 52) |
| `PgUp` `PgDn` `g` `G` | Scroll |
| `d` | Theme: paper (the site's colors), night, plain (your terminal's colors) |
| `?` · `esc` | Keys and credits · back |
| `q` · `Ctrl+C` | Quit |

Below 64 columns the layout goes compact; below 32×10 it asks you to resize. Focused links carry OSC 8 hyperlinks (cmd/ctrl-click), and every URL is also shown as text. The server never opens a browser for a visitor. `NO_COLOR` starts local mode in the plain theme.

## Content

The website is the source of truth. It serves `/portfolio.json` ([contract v2](content/portfolio.schema.json)). With `--content-url`, the server refreshes from it at most once a minute and falls back to the bundled [content/portfolio.json](content/portfolio.json). See [docs/CONTENT.md](docs/CONTENT.md).

## Layout

```text
src/main.rs     CLI and local terminal loop
src/ssh.rs      SSH sessions, limits, lifecycle
src/store.rs    Content refresh from the website
src/content.rs  Contract v2 model and validation
src/input.rs    Incremental SSH input decoding
src/app.rs      Per-visitor pages, focus, scroll, sheets
src/ui.rs       Page design rendering, OSC 8 / OSC 52
src/theme.rs    Paper / night / plain tokens
src/text.rs     Styled span wrapping
deploy/         Production compose, Caddyfile, Azure VM scripts
scripts/smoke.py  Real PTY and OpenSSH checks
```

## Verify

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked && python3 scripts/smoke.py --ssh
```

CI runs all four on every push and pull request. The plan and status live in [PLAN.md](PLAN.md); the design in [docs/MOCKUPS.md](docs/MOCKUPS.md).

## Credits

Inspired by [Sean Yang's portfolio at seanyang.ca](https://seanyang.ca) and his [portfolio over SSH](https://github.com/aicheye/tui.seanyang.ca). This is an original implementation. See [CREDITS.md](CREDITS.md).
