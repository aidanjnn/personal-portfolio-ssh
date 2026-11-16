# Portfolio content

The website is the source of truth. Its build serves `https://www.aidanjeon.com/portfolio.json`, generated from its own `content/*.ts`. The SSH server bundles a snapshot of that file (`content/portfolio.json`) and, when started with `--content-url`, refreshes it from the website: at most once a minute, with a 2 s timeout, keeping the last good copy. A visitor never waits on the network. Each session keeps the snapshot it started with.

To change copy, edit the website. Its CI validates the `/portfolio.json` it is about to deploy with this server's own validator (`--check-content`, run from the published image), so a content edit the server would reject fails the website's build instead of silently freezing the SSH portfolio on its last good copy.

Check a file by hand, or refresh the bundled fallback when convenient:

```sh
cargo run --locked -- --check-content path/to/portfolio.json   # or - for stdin
curl -fsS https://www.aidanjeon.com/portfolio.json > content/portfolio.json && cargo test --locked
```

## Contract v2

[`content/portfolio.schema.json`](../content/portfolio.schema.json) (JSON Schema 2020-12) documents the contract; it lives only in this repo, next to the validator. A test keeps its limits and required fields in step with `src/content.rs`. `Portfolio::from_json` in `src/content.rs` enforces the same shape and limits, rejects unknown fields and versions other than 2, caps snapshots at 1 MiB, and rejects control, bidi-override and zero-width characters, so fetched text can't inject terminal escapes.

| Field | Shape |
| --- | --- |
| `version` | `2` |
| `updated` | Display label such as "Updated Nov 10th, 2026", or `null` |
| `name`, `web` | Site owner and canonical web URL |
| `intro` | 1–16 paragraphs. Each is 1–64 spans `{text, style, url}`, where `style` is `plain`, `strong`, `emphasis`, `accent` or `highlight`, and a `url` makes the span a link |
| `work` | `{year, company, role, description, url}`, current role first |
| `projects` | `{slug, title, event, summary, year, url}`; `url` is absolute (external repo or the site's project page) |
| `thoughts` | `{slug, title, subtitle, year, url, sections: [{heading, items: [{text, url}]}]}` |
| `webring` | `{previous, home, next}` |

Short text is at most 128 characters and body text at most 2,048. Years run from 1900 to 2100. URLs are `https://` with a plain DNS hostname, or a simple `mailto:`; credentials, ports, custom schemes and raw whitespace are rejected.

The Sean Yang credit is part of the app (the help sheet), not the content, so it can't be dropped by a content edit.
