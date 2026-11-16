# Page design mockups (approved direction A)

Plain-text renderings of the approved mockups (claude.ai artifact "ssh aidanjeon.com", Oct 16 2026). Colors and the token table live in docs/MOCKUPS.md. The final Explorer frame is the superseded direction B, kept for comparison. Copy in these frames predates the Nov 2026 site copy; content/portfolio.json wins.

### 100×30 — Home. 100×30, paper. Visitors land on the same prose as the site, with focus on the Copperlane link and its URL in the status bar. Birds sit in the right gutter only at 96+ columns.

```text

                 Aidan Jeon    996    Projects    Thoughts
                                                                                          \_/
                 Updated Sep 24th, 2026
                                                                                               v
                 I'm a Software Engineering student at the University of Waterloo,     \/
                 focused on agent infrastructure and dabbling into physical AI.
                                                                                            -v-
                 Most recently built agents for regulated finance, changing the
                 mortgage industry with Copperlane (YC W26) in SF with great            v
                 people!                                                                        \/

                 Previously, I was the youngest ever engineering intern (15) at       \_/
                 Vancouver's largest hospital. In this stage of my life, I seek to
                 achieve greatness in every single thing I touch!                            v

                 I split my time across SF, Toronto, and Vancouver. Outside of           \/
                 work, I like long walks, football, and finding sunset views.
                                                                                                v
                 You can find me on X, LinkedIn, GitHub, or reach me via email.
                                                                                      \_/
                                                                                              v

                                 ↳ you found the terminal version.                        -v-

                                                                                       \/
                                                                                                v
                 © Aidan Jeon 2026                                            ← ◇ →

                 ↗ copperlane.ai   ↵ open   c copy    h w p t  ·  ? keys  ·  q quit
```

### 80×24 — 996 / Work. 80×24. It keeps the site's year | company | role grid. ↵ expands the row inline the way the chevron does on the web, and o shows the company link.

```text

       Aidan Jeon    996    Projects    Thoughts

       Work

     › 2026  Copperlane (YC W26) ▴         Software Engineering Intern
             Copperlane is the intelligence layer for regulated
             finance. I'm helping develop the agent + eval harness + do
             FDE work.

       2026  UW Blueprint ▾                Software Engineer

       2025  WATonomous ▾                  Robotics

       2024  Vancouver General Hospital ▾  Biomedical Engineering Intern

       2024  Strive ▾                      Organizer




       © Aidan Jeon 2026                                            ← ◇ →

       ↗ copperlane.ai   ↵ collapse   o link            ? keys  ·  q quit
```

### 80×24 — Projects with the link sheet open. ↵ on Otto opens an OSC 8 link (cmd/ctrl-click) with the URL in plain text, and c copies it to the visitor's clipboard over OSC 52. The server never launches a browser.

```text

       Aidan Jeon    996    Projects    Thoughts

       Projects

       2026  teachAR (Hack the North)
             AR tutor for learning physical skills through recorded ghost
             hands & an AI coach

     › 2026  Otto
             Voice agent automating GitHub, Gmail, & G-Cal via natural
             speech. Won 1st place at NexHacks at CMU (1500+ hackers)
             (Best use of LiveKit — $1000 + internship).

       2026  Toothfairy
             Cursor for dentists, streamlining dental workflows

       2026  Sketchy
             Draw wireframes & deploy to Vercel in <60 seconds

       ┌─ Otto ─────────────────────────────────────────────────────────┐
       │ https://github.com/abrj7/otto           cmd/ctrl-click to open │
       └────────────────────────────────────────────────────────────────┘
       ↵/esc close   c copy link                        ? keys  ·  q quit
```

### 80×24 — Thoughts → misc. opens in an in-terminal reader that follows thought-article.tsx: year, title, subtitle, then sections. It scrolls with j/k and has a scrollbar in rule color.

```text

       Aidan Jeon    996    Projects    Thoughts

       2026
       misc.
       my entire life-stack

       productivity stack                                                 ┃
         dia (considering aside)                                          ┃
         wisprflow (my coworkers put me on)                               ┃
         notion (ever since grade 8)                                      ┃
         poke (negotiated it down to 0.50 c/m)                            │
         paper                                                            │
         cap (open-source screen studio)                                  │
         raycast                                                          │
         rectangle                                                        │
                                                                          │
       devtools                                                           │
         cloud-based agents: devin                                        │
         zed, cursor (for design)                                         │
         codex macOS app (lovely harness + considering the move to        │
         GUI)                                                             │

       j k scroll   esc back to Thoughts      ↓ 22%  ·  ? keys  ·  q quit
```

### 80×24 — ? keys & credits. One sheet over a dimmed page holds the keymap, the credit to Sean, and the © + webring row.

```text

       Aidan Jeon    996    Projects    Thoughts

       Updated Sep 24th, 2026
              ┌─ keys ───────────────────────────────────────────┐
       I'm a S│                                                  │erloo,
       focused│  h w p t  pages          ← →  prev / next        │AI.
              │  j k ↑ ↓  move           ↵  open · expand        │
       Most re│  esc      back           o c  show · copy link   │the
       mortgag│  d        theme: paper / night / plain           │
       people!│  q        quit                                   │
              │                                                  │
       Previou│  ──────────────────────────────────────────────  │ at
       Vancouv│  SSH idea from Sean Yang · seanyang.ca           │eek to
       achieve│  built with ratatui + russh · source             │
              │                                                  │
       I split│  © Aidan Jeon 2026             ← ◇ se-webring →  │of
       work, I│                                                  │.
              │                                                  │
       You can└──────────────────────────────────────────────────┘il.

       © Aidan Jeon 2026                                            ← ◇ →

       esc close                                                   q quit
```

### 80×24 — Night. Press d. It keeps the hierarchy and the accent, with tokens tuned for dark terminals. Plain mode (the third press) uses the terminal's own colors for anything older.

```text

       Aidan Jeon    996    Projects    Thoughts

       Updated Sep 24th, 2026

       I'm a Software Engineering student at the University of Waterloo,
       focused on agent infrastructure and dabbling into physical AI.

       Most recently built agents for regulated finance, changing the
       mortgage industry with Copperlane (YC W26) in SF with great
       people!

       Previously, I was the youngest ever engineering intern (15) at
       Vancouver's largest hospital. In this stage of my life, I seek to
       achieve greatness in every single thing I touch!

       I split my time across SF, Toronto, and Vancouver. Outside of
       work, I like long walks, football, and finding sunset views.

       You can find me on X, LinkedIn, GitHub, or reach me via email.

       © Aidan Jeon 2026                                            ← ◇ →

       ↗ uwaterloo.ca/engineering   ↵ open             ↓ more  ·  d theme
```

### 40×16 — Compact. 40×16. Below 64 columns the column shrinks to 36 cells, the nav gap tightens to 2, and the footer moves into the scroll. Below 32×10 it shows a resize message.

```text
  Aidan Jeon  996  Projects  Thoughts

  I'm a Software Engineering student
  at the University of Waterloo,
  focused on agent infrastructure and
  dabbling into physical AI.

  Most recently built agents for
  regulated finance, changing the
  mortgage industry with Copperlane
  (YC W26) in SF with great people!

  Previously, I was the youngest ever
  engineering intern (15) at

  ↓ more                ? keys  q quit
```

### 80×13 — Before and after. On first connect OpenSSH asks the visitor to trust the host key, and the site publishes the fingerprint to compare against. After q, the TUI leaves the alternate screen and prints one line.

```text
~ ❯ ssh aidanjeon.com
The authenticity of host 'aidanjeon.com (x.x.x.x)' can't be established.
ED25519 key fingerprint is SHA256:<published on www.aidanjeon.com>
This key is not known by any other names.
Are you sure you want to continue connecting (yes/no/[fingerprint])? yes
Warning: Permanently added 'aidanjeon.com' (ED25519) to the list of known hosts.

        … portfolio runs in the alternate screen …

↳ thanks for stopping by  www.aidanjeon.com
Connection to aidanjeon.com closed.
~ ❯ █

```

### 80×24 — B · Explorer, as captured from your worktree and recolored with the site's tokens. It works, but the boxes, the tree and the TREE/READ modes are a file-manager idiom the site doesn't use. It also omits teachAR.

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│aidan / projects / otto                                                       │
│──────────────────────────────────────────────────────────────────────────────│
│ ~/aidan ───────────┐ preview ────────────────────────────────────────────────│
│  about             │ PROJECT / 2026                                          │
│  996               │                                                         │
│  v projects/       │ Otto                                                    │
│>   otto            │ Voice agent                                             │
│    toothfairy      │                                                         │
│    sketchy         │ Voice agent automating GitHub, Gmail, & G-Cal via       │
│  v thoughts/       │ natural speech.                                         │
│    misc.           │                                                         │
│  ------------      │ | Won 1st place at NexHacks at CMU (1500+ hackers)      │
│  links             │ (Best use of LiveKit — $1000 + internship).             │
│  credits           │                                                         │
│                    │ -- source                                               │
│                    │                                                         │
│                    │ > Otto                                                  │
│                    │ https://github.com/abrj7/otto                           │
│                    │                                                         │
│                    │                                                         │
│[] link  enter URL  PgUp/Dn scroll  ? credits  s theme                        │
│ TREE j/k move  tab focus  q quit                                       03/08 │
└──────────────────────────────────────────────────────────────────────────────┘
```
