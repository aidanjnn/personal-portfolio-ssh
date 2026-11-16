# ssh aidanjeon.com

## Outcome

A visitor runs `ssh aidanjeon.com` and finds [aidanjeon.com](https://www.aidanjeon.com/) in their terminal: the same nav, copy, columns, footer and colors. Each visitor gets independent state. The website is the single content source: it serves `/portfolio.json`, which the server refreshes at most once a minute, with a bundled snapshot as the fallback.

## Design

**Direction A, "Page"** was approved on Nov 14, 2026. It supersedes the September Explorer design.

- Frames: [docs/screens/page-mockups.md](docs/screens/page-mockups.md). Tokens and the type mapping: [docs/MOCKUPS.md](docs/MOCKUPS.md).
- Header nav: Aidan Jeon / 996 / Projects / Thoughts. A 66-cell centered column, a year gutter, a footer with © and the webring, and a status bar showing the focused URL.
- Paper (the site's colors), Night, and Plain (the terminal's own colors) themes, switched with `d`.
- Link sheet with OSC 8 and OSC 52 copy. Help sheet with the keymap and the credit to [Sean Yang](https://seanyang.ca).
- Compact layout under 64 columns. Birds in the right gutter at 96+ columns.

## Architecture

| You type | DNS | Lands on | Result |
| --- | --- | --- | --- |
| `ssh aidanjeon.com` | A → VM | host :22 → container :2222 | Portfolio session |
| `ssh -p 2222 ssh.aidanjeon.com` | A → VM | host :2222 → container :2222 | Fallback where outbound 22 is blocked |
| `https://aidanjeon.com` | A → VM | Caddy :443 | 308 to www |
| `https://www.aidanjeon.com` | CNAME → Vercel | Next.js | Site and `/portfolio.json` |
| admin | Tailscale SSH | VM | No public admin port |

The host is an Azure VM on Azure for Students: `Standard_B2ats_v2`, Ubuntu 24.04, 64 GiB P6 disk, static IPv4, in `canadacentral` (`westus2` if refused). The VM and disk are on the 12-month free tier; anything else comes out of the $100 credit. DNS is at Namecheap. The runbook is [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

## Status

- [x] 0. Saved the Explorer work (`4850a5e`).
- [x] 1a. Content contract v2 (`content/portfolio.schema.json`), Rust model and validation.
- [x] 1b. Website serves `/portfolio.json` from its own content (website branch `feat/portfolio-json`, not merged or deployed yet).
- [x] 1c. Server refreshes content from the website (≤ 1/min, 2 s timeout, last good copy, bundled fallback).
- [x] 2. Page design renderer, focus, sheets, themes, compact layout and birds.
- [x] 3. Hardening: 3 connections per IP, 30-minute hard limit, goodbye line, connect logs without IPs, decoder fuzz test.
- [x] 4. Deploy files: production compose, Caddyfile, Azure provisioning script (`deploy/azure.sh`), unattended bootstrap, fingerprint script, GHCR deploy workflow.
- [ ] 5. Provision the Azure VM with `deploy/azure.sh` and do the DNS cutover at Namecheap (apex A and a new `ssh` A record only). **Owner action**: needs Azure for Students, Tailscale, and Namecheap access.
- [ ] 6. Launch: change the site's "A TUI portfolio is coming!" line to `ssh aidanjeon.com`, publish the host-key fingerprint, record a demo.

## Launch checks

- [ ] `ssh aidanjeon.com` works from another network, and `curl -I https://aidanjeon.com` returns a 308 to www.
- [ ] Terminal.app (macOS 26), iTerm2, Ghostty, Windows Terminal, and a Linux terminal: colors, SGR 58 underline colors, links, copy.
- [ ] 100×30, 80×24, 40×16 and resizing mid-session.
- [ ] Host key backed up offline; fingerprint published on the site.
- [ ] Rollback to the previous image tag rehearsed.
