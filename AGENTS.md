# Terminal portfolio

Rust / Ratatui / Crossterm. The application is a local and SSH-served portfolio; public hosting is a separate, owner-run milestone (docs/DEPLOYMENT.md). Read README.md and PLAN.md before changing scope.

- Keep the local event loop, application state, content, and rendering separate.
- The approved design is the Page design (direction A): docs/MOCKUPS.md and docs/screens/page-mockups.md. It mirrors https://www.aidanjeon.com/ — same nav, copy, column, footer and color tokens. Sean Yang is credited for the SSH inspiration in the help sheet.
- The website is the content source of truth (it serves /portfolio.json, contract v2 in content/portfolio.schema.json). content/portfolio.json is the bundled fallback snapshot of it. Do not invent credentials, employers, achievements, contact information, or launch URLs.
- Keep credits linking to https://seanyang.ca in README.md, CREDITS.md and the help sheet.
- Keep terminal rendering event-driven, support small windows, and restore the terminal on exit.
- Preserve the original Next.js portfolio as a separate project.
- Run cargo fmt, cargo clippy --locked --all-targets -- -D warnings, and cargo test --locked for Rust changes.
- Use Conventional Commits. Every commit is authored solely by the configured repo owner: no coauthor trailers, agent attribution, author overrides, or hook bypasses.
- Do not commit target/, credentials, or SSH private host keys.
- Do not deploy, buy infrastructure, change DNS, or expose a public SSH service unless requested.
