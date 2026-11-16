# Page design

The approved design is direction A, "Page". It renders [aidanjeon.com](https://www.aidanjeon.com/) in terminal cells: the same nav, copy, columns and footer. The character-exact frames are in [screens/page-mockups.md](screens/page-mockups.md). Screenshots of the real binary in kitty are in [screens/png](screens/png).

## Tokens

The tokens come from the website's `app/globals.css`. Paper paints every cell; Plain defers to the visitor's terminal colors.

| Token | Paper | Night | Plain | Used for |
| --- | --- | --- | --- | --- |
| canvas | `#ffffff` | `#11161b` | default | Background |
| ink | `#32404f` | `#dfe4e8` | default | Prose, name, active nav, titles |
| faint | `#7a8794` | `#8d99a5` | dim | Inactive nav, roles, summaries |
| date | `#a8b2bb` | `#6b7783` | dark gray | Years, "Updated", status bar (non-essential text only) |
| rule | `#e4e7ea` | `#262e36` | dark gray | Sheet borders, scrollbar track |
| accent | `#f97316` | `#fb8a3c` | yellow | The Copperlane link, focus underline (never body text) |
| decoration | `#c5ccd3` | `#56636f` | dark gray | Link underlines, birds |
| focus | `#fee6d5` | `#3a2616` | reverse video | Focused link or row |

## From the site to cells

Schibsted Grotesk can't travel over SSH; the visitor's terminal font draws every cell. Hierarchy moves into weight, italic, underline and color.

| Site | Terminal |
| --- | --- |
| Name, 20px bold | Bold ink |
| Page title, 26px bold | Bold ink, then a blank row |
| Body, 15px / 1.75 | Ink, a blank row between paragraphs |
| "University of Waterloo", semibold italic | Bold italic |
| "seek to achieve greatness" | Bold, underlined in faint |
| Links with decoration underline | Underline in decoration color, OSC 8 when focused |
| Year column | Date color, 4 cells and 2 cells of gap |
| 600px column | 66 cells centered; `cols − 4` below 64 columns |
| Work chevron | `▾` / `▴` after the company |
| `birds.jpg` at 25% opacity | ASCII birds in the right gutter at 96+ columns |
| Footer © and webring | Footer row in the same order |
| Copperlane logo | No images; the accent text carries it |

## Keys

| Keys | Action |
| --- | --- |
| `h` `w` `p` `t` | Home, 996, Projects, Thoughts |
| `←` `→` | Previous / next page |
| `j` `k` `↑` `↓` `Tab` | Move focus; scroll in the reader |
| `↵` | Open a link sheet, expand a Work row, or open a Thought |
| `o` · `c` | Show the focused URL · copy it (OSC 52) |
| `PgUp` `PgDn` `g` `G` | Scroll |
| `esc` · `?` · `d` | Back · keys and credits · theme |
| `q` · `Ctrl+C` | Quit and print a goodbye line with the web URL |
