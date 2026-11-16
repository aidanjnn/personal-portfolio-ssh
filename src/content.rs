use std::{error::Error, fmt, ops::Deref};

use serde::{Deserialize, Serialize};
use unicode_properties::{
    EmojiStatus, GeneralCategory, GeneralCategoryGroup, UnicodeEmoji, UnicodeGeneralCategory,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

pub const BUNDLED_JSON: &str = include_str!("../content/portfolio.json");
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const CONTENT_VERSION: u8 = 2;
const SHORT_TEXT_LIMIT: usize = 128;
const BODY_TEXT_LIMIT: usize = 2048;
const URL_LIMIT: usize = 2048;
/// The most entries any list may hold: work, projects, thoughts, and a section's items.
/// Expanded Work rows live in a `u64` bitset (`App::expanded`), so this cannot exceed 64.
pub const MAX_LIST_ENTRIES: usize = 64;
const _: () = assert!(MAX_LIST_ENTRIES <= u64::BITS as usize);

/// Content that passed `PortfolioData::validate`. The field is private and the type does not
/// implement `Deserialize`, so `from_json`, `bundled` and `new` are the only ways to get one, and
/// the renderer accepts nothing else. Fields are read through `Deref`.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(transparent)]
pub struct Portfolio(PortfolioData);

/// Public portfolio content as parsed, contract v2. The website serves it at /portfolio.json.
/// Unvalidated: wrap it with `Portfolio::new` before rendering it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PortfolioData {
    pub version: u8,
    /// Required, but may be `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub updated: Option<String>,
    pub name: String,
    pub web: String,
    pub intro: Vec<Vec<Span>>,
    pub work: Vec<Work>,
    pub projects: Vec<Project>,
    pub thoughts: Vec<Thought>,
    pub webring: Webring,
}

/// A run of intro text. `url` makes it a focusable link.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
    /// Required, but may be `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpanStyle {
    Plain,
    /// Bold.
    Strong,
    /// Bold italic ("University of Waterloo").
    Emphasis,
    /// Bold in the accent color (the current Work link).
    Accent,
    /// Bold with a faint underline ("seek to achieve greatness").
    Highlight,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Work {
    pub year: u16,
    pub company: String,
    pub role: String,
    pub description: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub slug: String,
    pub title: String,
    /// Required, but may be `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub event: Option<String>,
    pub summary: String,
    pub year: u16,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Thought {
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub year: u16,
    pub url: String,
    pub sections: Vec<ThoughtSection>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThoughtSection {
    pub heading: String,
    pub items: Vec<ThoughtItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThoughtItem {
    pub text: String,
    /// Required, but may be `null`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Webring {
    pub previous: String,
    pub home: String,
    pub next: String,
}

#[derive(Debug)]
pub struct ContentError(String);

impl fmt::Display for ContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ContentError {}

impl Portfolio {
    pub fn bundled() -> Result<Self, ContentError> {
        Self::from_json(BUNDLED_JSON)
    }

    pub fn from_json(json: &str) -> Result<Self, ContentError> {
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err(ContentError(
                "content exceeds the 1 MiB snapshot limit".into(),
            ));
        }
        let data: PortfolioData = serde_json::from_str(json)
            .map_err(|error| ContentError(format!("invalid portfolio JSON: {error}")))?;
        Self::new(data)
    }

    /// Validates `data`; the only constructor besides parsing.
    pub fn new(data: PortfolioData) -> Result<Self, ContentError> {
        data.validate()?;
        Ok(Self(data))
    }

    /// Gives up the validation guarantee so tests can edit content and validate it again.
    #[cfg(test)]
    pub fn into_data(self) -> PortfolioData {
        self.0
    }
}

impl Deref for Portfolio {
    type Target = PortfolioData;

    fn deref(&self) -> &PortfolioData {
        &self.0
    }
}

impl PortfolioData {
    pub fn validate(&self) -> Result<(), ContentError> {
        if self.version != CONTENT_VERSION {
            return Err(ContentError(format!(
                "unsupported content version {} (expected {CONTENT_VERSION})",
                self.version
            )));
        }
        if let Some(updated) = &self.updated {
            validate_text("updated", updated, SHORT_TEXT_LIMIT)?;
        }
        validate_text("name", &self.name, SHORT_TEXT_LIMIT)?;
        validate_url("web", &self.web)?;
        validate_count("intro", self.intro.len(), 16)?;
        for (index, paragraph) in self.intro.iter().enumerate() {
            let field = format!("intro[{index}]");
            validate_count(&field, paragraph.len(), 64)?;
            for (span_index, span) in paragraph.iter().enumerate() {
                let field = format!("{field}[{span_index}]");
                validate_span_text(&format!("{field}.text"), &span.text)?;
                if let Some(url) = &span.url {
                    validate_url(&format!("{field}.url"), url)?;
                    // A link is a focus stop and titles its sheet, so it must draw something.
                    if !has_visible_text(&span.text) {
                        return Err(ContentError(format!(
                            "{field}.text must be visible because the span is a link"
                        )));
                    }
                }
            }
            if !paragraph.iter().any(|span| has_visible_text(&span.text)) {
                return Err(ContentError(format!("{field} has no visible text")));
            }
        }
        validate_count("work", self.work.len(), MAX_LIST_ENTRIES)?;
        for (index, work) in self.work.iter().enumerate() {
            let field = format!("work[{index}]");
            validate_entry(&field, &work.company, work.year, &work.url)?;
            validate_text(&format!("{field}.role"), &work.role, SHORT_TEXT_LIMIT)?;
            validate_text(
                &format!("{field}.description"),
                &work.description,
                BODY_TEXT_LIMIT,
            )?;
        }
        validate_count("projects", self.projects.len(), MAX_LIST_ENTRIES)?;
        for (index, project) in self.projects.iter().enumerate() {
            let field = format!("projects[{index}]");
            validate_slug(&format!("{field}.slug"), &project.slug)?;
            validate_entry(&field, &project.title, project.year, &project.url)?;
            if let Some(event) = &project.event {
                validate_text(&format!("{field}.event"), event, SHORT_TEXT_LIMIT)?;
            }
            validate_text(
                &format!("{field}.summary"),
                &project.summary,
                BODY_TEXT_LIMIT,
            )?;
        }
        validate_count("thoughts", self.thoughts.len(), MAX_LIST_ENTRIES)?;
        for (index, thought) in self.thoughts.iter().enumerate() {
            let field = format!("thoughts[{index}]");
            validate_slug(&format!("{field}.slug"), &thought.slug)?;
            validate_entry(&field, &thought.title, thought.year, &thought.url)?;
            validate_text(
                &format!("{field}.subtitle"),
                &thought.subtitle,
                BODY_TEXT_LIMIT,
            )?;
            if thought.sections.len() > 32 {
                return Err(ContentError(format!(
                    "{field}.sections must contain at most 32 entries"
                )));
            }
            for (section_index, section) in thought.sections.iter().enumerate() {
                let field = format!("{field}.sections[{section_index}]");
                validate_text(
                    &format!("{field}.heading"),
                    &section.heading,
                    SHORT_TEXT_LIMIT,
                )?;
                validate_count(
                    &format!("{field}.items"),
                    section.items.len(),
                    MAX_LIST_ENTRIES,
                )?;
                for (item_index, item) in section.items.iter().enumerate() {
                    let field = format!("{field}.items[{item_index}]");
                    validate_text(&format!("{field}.text"), &item.text, BODY_TEXT_LIMIT)?;
                    if let Some(url) = &item.url {
                        validate_url(&format!("{field}.url"), url)?;
                    }
                }
            }
        }
        validate_url("webring.previous", &self.webring.previous)?;
        validate_url("webring.home", &self.webring.home)?;
        validate_url("webring.next", &self.webring.next)?;
        Ok(())
    }
}

/// Spans may be pure punctuation or whitespace joins (", "), so only length and safety are checked.
fn validate_span_text(field: &str, text: &str) -> Result<(), ContentError> {
    if text.is_empty() || text.chars().count() > BODY_TEXT_LIMIT {
        return Err(ContentError(format!(
            "{field} must contain 1–{BODY_TEXT_LIMIT} characters"
        )));
    }
    reject_unsafe_chars(field, text)
}

fn validate_slug(field: &str, slug: &str) -> Result<(), ContentError> {
    let valid = !slug.is_empty()
        && slug.len() <= 64
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(ContentError(format!(
            "{field} must be a lowercase kebab-case slug"
        )));
    }
    Ok(())
}

fn validate_count(field: &str, count: usize, maximum: usize) -> Result<(), ContentError> {
    if count == 0 || count > maximum {
        return Err(ContentError(format!(
            "{field} must contain 1–{maximum} entries"
        )));
    }
    Ok(())
}

/// Whether `text` draws at least one glyph: whitespace and zero-width marks alone do not.
fn has_visible_text(text: &str) -> bool {
    text.chars()
        .any(|ch| !ch.is_whitespace() && ch.width().is_some_and(|width| width > 0))
}

fn validate_text(field: &str, text: &str, maximum: usize) -> Result<(), ContentError> {
    if !has_visible_text(text) || text.chars().count() > maximum {
        return Err(ContentError(format!(
            "{field} must contain 1–{maximum} visible characters"
        )));
    }
    reject_unsafe_chars(field, text)
}

const ZWJ: char = '\u{200d}';
const VS16: char = '\u{fe0f}';
pub const MAX_COMBINING_MARKS: usize = 8;
/// A grapheme cluster is drawn in one terminal cell (or two), however many code points it
/// holds. The longest standard emoji (👩🏻‍❤️‍💋‍👨🏼, kiss with two skin tones) is 10 code points
/// with 3 joiners, so these caps admit every standard emoji and keep a cell's symbol small.
pub const MAX_CLUSTER_CHARS: usize = 16;
pub const MAX_CLUSTER_JOINERS: usize = 3;
const SKIN_TONES: std::ops::RangeInclusive<char> = '\u{1f3fb}'..='\u{1f3ff}';
/// Hangul fillers are letters (Lo) that draw nothing, so no category rule catches them.
const HANGUL_FILLERS: [char; 4] = ['\u{115f}', '\u{1160}', '\u{3164}', '\u{ffa0}'];

/// U+FDD0–U+FDEF and the last two code points of every plane.
fn is_noncharacter(ch: char) -> bool {
    let code = u32::from(ch);
    (0xfdd0..=0xfdef).contains(&code) || code & 0xfffe == 0xfffe
}

/// Rejected by whole category: controls (Cc: C0, DEL, C1), format characters (Cf: bidi
/// overrides, soft hyphen, tags, invisible operators, …), surrogates (Cs), private use (Co),
/// unassigned code points (Cn, which covers noncharacters), and the line and paragraph
/// separators (Zl, Zp). Categories come from the `unicode-properties` tables.
fn is_unsafe_char(ch: char) -> bool {
    use GeneralCategory::*;
    matches!(
        ch.general_category(),
        Control | Format | Surrogate | PrivateUse | Unassigned | LineSeparator | ParagraphSeparator
    ) || is_noncharacter(ch)
        || HANGUL_FILLERS.contains(&ch)
}

/// ASCII digits, `#` and `*` are Emoji=Yes only as keycap bases, which never take a ZWJ.
fn is_emoji(ch: char) -> bool {
    !ch.is_ascii() && ch.is_emoji_char()
}

/// Emoji_Modifier_Base: the emoji a skin-tone modifier may follow (👋, 👩, ✍).
fn is_modifier_base(ch: char) -> bool {
    matches!(
        ch.emoji_status(),
        EmojiStatus::EmojiModifierBase | EmojiStatus::EmojiPresentationAndModifierBase
    )
}

/// Rejects characters that could drive the terminal or hide text. The one Cf character kept is
/// U+200D ZERO WIDTH JOINER, and only inside an emoji ZWJ sequence (👩‍💻, ❤️‍🔥): right after an
/// emoji (optionally with VS16) and right before another. Those sequences are common in written
/// text and render as one glyph; anywhere else a ZWJ is just an invisible character.
///
/// A skin-tone modifier (U+1F3FB–U+1F3FF) is kept only right after an emoji modifier base,
/// so it can appear once per base (👋🏽) and never repeat (👋🏽🏽 would widen one cell).
///
/// It also caps consecutive combining marks (Mn, Mc, Me) at `MAX_COMBINING_MARKS`: real text
/// stacks a few at most, while "zalgo" stacks draw far outside their cell and make one
/// terminal cell carry an unbounded symbol. Every grapheme cluster is capped at
/// `MAX_CLUSTER_CHARS` code points and `MAX_CLUSTER_JOINERS` ZWJs for the same reason: an emoji
/// ZWJ chain of any length is one cluster, and so one cell.
fn reject_unsafe_chars(field: &str, text: &str) -> Result<(), ContentError> {
    let mut after_emoji = false;
    let mut previous = None;
    let mut marks = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let safe = if ch == ZWJ {
            after_emoji && chars.peek().is_some_and(|&next| is_emoji(next))
        } else if SKIN_TONES.contains(&ch) {
            previous.is_some_and(is_modifier_base)
        } else {
            !is_unsafe_char(ch)
        };
        if !safe {
            return Err(ContentError(format!(
                "{field} contains a terminal control or invisible character (U+{:04X})",
                u32::from(ch)
            )));
        }
        previous = Some(ch);
        after_emoji = is_emoji(ch) || (after_emoji && ch == VS16);
        marks = match ch.general_category_group() {
            GeneralCategoryGroup::Mark => marks + 1,
            _ => 0,
        };
        if marks > MAX_COMBINING_MARKS {
            return Err(ContentError(format!(
                "{field} stacks more than {MAX_COMBINING_MARKS} combining marks on one character"
            )));
        }
    }
    for cluster in text.graphemes(true) {
        let joiners = cluster.chars().filter(|&ch| ch == ZWJ).count();
        if cluster.chars().count() > MAX_CLUSTER_CHARS || joiners > MAX_CLUSTER_JOINERS {
            return Err(ContentError(format!(
                "{field} joins more than {MAX_CLUSTER_CHARS} code points or \
                 {MAX_CLUSTER_JOINERS} ZWJs into one character"
            )));
        }
    }
    Ok(())
}

fn validate_entry(field: &str, name: &str, year: u16, url: &str) -> Result<(), ContentError> {
    validate_text(&format!("{field}.name"), name, SHORT_TEXT_LIMIT)?;
    if !(1900..=2100).contains(&year) {
        return Err(ContentError(format!(
            "{field}.year must be between 1900 and 2100"
        )));
    }
    validate_url(&format!("{field}.url"), url)
}

fn valid_domain(domain: &str) -> bool {
    domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.starts_with(|ch: char| ch.is_ascii_alphanumeric())
                && label.ends_with(|ch: char| ch.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_uri_tail(tail: &str) -> bool {
    let mut bytes = tail.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            if !bytes.next().is_some_and(|next| next.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|next| next.is_ascii_hexdigit())
            {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric() && !b"-._~:/?#[]@!$&'()*+,;=".contains(&byte) {
            return false;
        }
    }
    true
}

fn validate_url(field: &str, url: &str) -> Result<(), ContentError> {
    validate_text(field, url, URL_LIMIT)?;
    let valid = if let Some(rest) = url.strip_prefix("https://") {
        let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        valid_domain(&rest[..split]) && valid_uri_tail(&rest[split..])
    } else if let Some(address) = url.strip_prefix("mailto:") {
        address.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && local
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
                && valid_domain(domain)
        })
    } else {
        false
    };
    if !valid {
        return Err(ContentError(format!(
            "{field} must be a public HTTPS URL or simple mailto address"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_data() -> PortfolioData {
        Portfolio::bundled().unwrap().into_data()
    }

    #[test]
    fn parsed_content_is_only_renderable_after_validation() {
        // Parsing alone yields `PortfolioData`; `Portfolio` (what `ui::draw` takes) has no
        // `Deserialize`, so invalid content can only be wrapped through `Portfolio::new`.
        let invalid = BUNDLED_JSON.replacen("https://www.copperlane.ai/", "javascript:x", 1);
        let data: PortfolioData = serde_json::from_str(&invalid).unwrap();
        assert!(Portfolio::new(data).is_err());
        assert!(Portfolio::from_json(&invalid).is_err());
        let data = bundled_data();
        assert_eq!(*Portfolio::new(data.clone()).unwrap(), data);
    }

    #[test]
    fn bundled_content_matches_the_website() {
        let portfolio = Portfolio::bundled().unwrap();
        assert_eq!(portfolio.name, "Aidan Jeon");
        assert_eq!(
            portfolio
                .projects
                .iter()
                .map(|project| project.title.as_str())
                .collect::<Vec<_>>(),
            ["teachAR", "Otto", "Toothfairy", "Sketchy"]
        );
        assert_eq!(portfolio.projects[1].url, "https://github.com/abrj7/otto");
        assert_eq!(portfolio.work.len(), 5);
        assert_eq!(portfolio.work[0].company, "Copperlane (YC W26)");
        assert!(
            portfolio.intro[1]
                .iter()
                .any(|span| span.style == SpanStyle::Accent
                    && span.url.as_deref() == Some("https://www.copperlane.ai/"))
        );
        assert!(!portfolio.thoughts[0].sections.is_empty());
    }

    #[test]
    fn rejects_escape_sequences_and_hidden_direction_changes() {
        for unsafe_text in [
            "hello\x1b[2J",
            "hello\u{009b}",
            "hello\nworld",
            "hello\u{202e}world",
            "hello\u{200b}world",
        ] {
            let mut content = bundled_data();
            content.intro[0][0].text = unsafe_text.into();
            assert!(content.validate().is_err(), "accepted {unsafe_text:?}");
            let mut content = bundled_data();
            content.thoughts[0].sections[0].items[0].text = unsafe_text.into();
            assert!(content.validate().is_err(), "accepted {unsafe_text:?}");
        }
    }

    #[test]
    fn rejects_whole_categories_of_invisible_and_control_characters() {
        // Every character named in review, through the full content validator.
        let named = [
            // C0, DEL, C1
            "\u{0} \u{1b} \u{7f} \u{85} \u{9b}",
            // Cf: soft hyphen, Arabic letter mark, Mongolian vowel separator, zero-width and
            // bidi controls, BOM, tags, interlinear annotations, musical format controls
            "\u{ad} \u{61c} \u{180e} \u{200b} \u{200c} \u{200e} \u{202e} \u{2066} \u{feff}",
            "\u{e0001} \u{e0020} \u{e007f} \u{fff9} \u{fffa} \u{fffb} \u{1d173} \u{1d17a}",
            // Zl, Zp; Hangul fillers
            "\u{2028} \u{2029} \u{115f} \u{1160} \u{3164} \u{ffa0}",
            // Co (BMP, planes 15 and 16), Cn, noncharacters
            "\u{e000} \u{f8ff} \u{f0000} \u{ffffd} \u{100000} \u{10fffd} \u{378} \u{e0000}",
            "\u{fdd0} \u{fdef} \u{fffe} \u{ffff} \u{1fffe} \u{10ffff}",
        ];
        for ch in named.iter().flat_map(|group| group.split(' ')) {
            let mut content = bundled_data();
            content.thoughts[0].sections[0].items[0].text = format!("a{ch}b");
            assert!(content.validate().is_err(), "accepted {ch:?}");
            let mut content = bundled_data();
            content.intro[0][0].text = format!("a{ch}b");
            assert!(content.validate().is_err(), "accepted {ch:?}");
        }
        // Whole ranges, through the character check alone.
        let ranges = [
            '\u{0}'..='\u{1f}',
            '\u{7f}'..='\u{9f}',
            '\u{e0000}'..='\u{e007f}',
            '\u{fff9}'..='\u{fffb}',
            '\u{1d173}'..='\u{1d17a}',
            '\u{e000}'..='\u{f8ff}',
            '\u{f0000}'..='\u{10ffff}',
            '\u{fdd0}'..='\u{fdef}',
        ];
        let noncharacters = (0..=16).flat_map(|plane| {
            [0xfffe, 0xffff].map(|low| char::from_u32((plane << 16) | low).unwrap())
        });
        for ch in ranges.into_iter().flatten().chain(noncharacters) {
            assert!(
                reject_unsafe_chars("t", &format!("a{ch}b")).is_err(),
                "{ch:?}"
            );
        }
        for text in [
            "José",
            "Jose\u{301}",
            "漢字 かな 한국어",
            "café ☕ 👋🏽",
            "\u{2764}\u{fe0f}",
            "👩\u{200d}💻",
            "\u{2764}\u{fe0f}\u{200d}🔥",
            "🏳\u{fe0f}\u{200d}🌈",
            "👩🏽\u{200d}🤝\u{200d}👨🏻",
        ] {
            assert!(validate_text("t", text, 64).is_ok(), "rejected {text:?}");
        }
        // ZWJ outside an emoji sequence is just an invisible character.
        for text in [
            "a\u{200d}b",
            "\u{200d}👩",
            "👩\u{200d}",
            "👩\u{200d}\u{200d}💻",
            "1\u{200d}👩",
            "👩\u{200d}x",
        ] {
            assert!(validate_text("t", text, 64).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn caps_stacked_combining_marks() {
        let stack = |count: usize| format!("Zalgo{} ok", "\u{301}\u{316}".repeat(count / 2));
        assert!(validate_text("t", &stack(MAX_COMBINING_MARKS), 128).is_ok());
        assert!(validate_text("t", &stack(MAX_COMBINING_MARKS + 2), 128).is_err());
        // Any mark category counts (Mn, Mc, Me), and the count restarts after each base.
        let mixed = format!("a{}\u{903}", "\u{301}".repeat(MAX_COMBINING_MARKS));
        assert!(validate_text("t", &mixed, 128).is_err());
        let spread = "a\u{301}\u{302}\u{303}".repeat(40);
        assert!(validate_text("t", &spread, 1024).is_ok());
        let mut content = bundled_data();
        content.projects[0].title = stack(64);
        assert!(content.validate().is_err());
    }

    #[test]
    fn caps_grapheme_clusters() {
        // The longest standard emoji: 10 code points, 3 ZWJs, two skin tones.
        let kiss = "👩🏻\u{200d}\u{2764}\u{fe0f}\u{200d}💋\u{200d}👨🏼";
        let at_cap = format!("{kiss}{}", "\u{fe0f}".repeat(MAX_CLUSTER_CHARS - 10));
        for text in [
            kiss,
            &at_cap,
            "👨\u{200d}👩\u{200d}👧\u{200d}👦",
            "👋🏽 hi",
            "✍🏿",
            "\u{1100}\u{1161}\u{11a8}",
        ] {
            assert!(validate_text("t", text, 128).is_ok(), "rejected {text:?}");
        }
        let chain = |count: usize, emoji: &str| vec![emoji; count].join("\u{200d}");
        for text in [
            // One joiner too many, and the reviewer's chains (128 characters of each).
            chain(MAX_CLUSTER_JOINERS + 2, "👩"),
            format!("A{}", chain(300, "👩")),
            format!("C©{}", "\u{200d}©".repeat(300)),
            // One code point too many: VS16 keeps the joiner legal but grows the cluster.
            format!("{at_cap}\u{fe0f}"),
            format!("👩{0}\u{200d}👩{0}", "\u{fe0f}".repeat(8)),
            // Conjoining jamo never break, so 17 leading consonants are one cluster.
            format!("{}\u{1161}", "\u{1100}".repeat(16)),
            // A skin tone only directly after a modifier base, and once.
            format!("B👋{}", "🏽".repeat(300)),
            "👋🏽🏽".into(),
            "a🏽".into(),
            "🏽".into(),
            "☕🏽".into(),
            "👩\u{200d}🏽".into(),
        ] {
            let short: String = text.chars().take(SHORT_TEXT_LIMIT).collect();
            assert!(
                validate_text("t", &short, 128).is_err(),
                "accepted {short:?}"
            );
            let mut content = bundled_data();
            content.projects[0].title = short;
            assert!(content.validate().is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn linked_spans_need_visible_text() {
        for text in [" ", "\u{3000}", "\u{301}", "  \u{301} "] {
            let mut content = bundled_data();
            let span = content.intro[1]
                .iter_mut()
                .find(|span| span.url.is_some())
                .unwrap();
            span.text = text.into();
            assert!(content.validate().is_err(), "accepted link text {text:?}");
        }
        // Unlinked spans may still be bare joins such as ", " or " ".
        let mut content = bundled_data();
        content.intro[0].push(Span {
            text: " ".into(),
            style: SpanStyle::Plain,
            url: None,
        });
        assert!(content.validate().is_ok());
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../content/portfolio.schema.json")).unwrap();
        let span = &schema["$defs"]["span"];
        assert_eq!(span["if"]["properties"]["url"]["type"], "string");
        assert_eq!(span["then"]["properties"]["text"]["pattern"], "\\S");
    }

    #[test]
    fn rejects_unsafe_urls_and_accepts_public_links() {
        for unsafe_url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "http://example.com",
            "https://example.com\x1b]8;;",
            "https://good.com@evil.com",
            "https://example.com/a b",
            "https://example.com/a%xx",
            "mailto:user@example.com?body=hello",
            "https://example.com\\evil",
            "https://-example.com",
            "https://example..com",
        ] {
            assert!(
                validate_url("url", unsafe_url).is_err(),
                "accepted {unsafe_url:?}"
            );
        }
        for url in [
            "https://example.com",
            "https://example.com/path?a=b#fragment",
            "https://example.com/some%20path",
            "mailto:user+portfolio@example.com",
        ] {
            assert!(validate_url("url", url).is_ok(), "rejected {url:?}");
        }
        let mut content = bundled_data();
        content.intro[0][1].url = Some("javascript:alert(1)".into());
        assert!(content.validate().is_err());
    }

    #[test]
    fn rejects_missing_fields_unknown_fields_wrong_version_and_oversized_snapshots() {
        assert!(Portfolio::from_json("{}").is_err());
        let unknown = BUNDLED_JSON.replacen("{", "{\"unrecognized\": true,", 1);
        assert!(Portfolio::from_json(&unknown).is_err());
        let old = BUNDLED_JSON.replacen("\"version\": 2", "\"version\": 1", 1);
        assert!(Portfolio::from_json(&old).is_err());
        assert!(Portfolio::from_json(&" ".repeat(MAX_SNAPSHOT_BYTES + 1)).is_err());
    }

    #[test]
    fn rejects_invalid_lengths_years_and_slugs() {
        let mut content = bundled_data();
        content.projects[0].year = 1;
        assert!(content.validate().is_err());
        content.projects[0].year = 2026;
        content.projects[0].summary = " ".into();
        assert!(content.validate().is_err());
        content.projects[0].summary = "a".repeat(BODY_TEXT_LIMIT + 1);
        assert!(content.validate().is_err());
        content.projects[0].summary = "A project.".into();
        content.projects[0].slug = "Bad Slug".into();
        assert!(content.validate().is_err());
        content.projects[0].slug = "ok-slug".into();
        content.projects.clear();
        assert!(content.validate().is_err());
        let mut content = bundled_data();
        content.intro[0] = vec![Span {
            text: "  ".into(),
            style: SpanStyle::Plain,
            url: None,
        }];
        assert!(content.validate().is_err());
    }

    #[test]
    fn schema_bounds_and_properties_match_the_rust_contract() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../content/portfolio.schema.json")).unwrap();
        assert_eq!(schema["$defs"]["shortText"]["maxLength"], SHORT_TEXT_LIMIT);
        assert_eq!(schema["$defs"]["bodyText"]["maxLength"], BODY_TEXT_LIMIT);
        assert_eq!(schema["$defs"]["url"]["maxLength"], URL_LIMIT);
        assert_eq!(schema["properties"]["version"]["const"], CONTENT_VERSION);
        let properties = &schema["properties"];
        for list in ["work", "projects", "thoughts"] {
            assert_eq!(properties[list]["maxItems"], MAX_LIST_ENTRIES, "{list}");
        }
        let sections = &properties["thoughts"]["items"]["properties"]["sections"];
        assert_eq!(
            sections["items"]["properties"]["items"]["maxItems"],
            MAX_LIST_ENTRIES
        );
        let snapshot: serde_json::Value = serde_json::from_str(BUNDLED_JSON).unwrap();

        // Every object in the contract: the schema requires exactly the properties it declares
        // and the snapshot has, Rust rejects the snapshot without any one of them, and Rust
        // accepts `null` exactly where the schema does.
        let thought = &properties["thoughts"]["items"];
        let section = &thought["properties"]["sections"]["items"];
        let span = &properties["intro"]["items"]["items"]["$ref"];
        let span = schema.pointer(span.as_str().unwrap().trim_start_matches('#'));
        let objects = [
            ("", &schema),
            ("/intro/0/0", span.unwrap()),
            ("/work/0", &properties["work"]["items"]),
            ("/projects/0", &properties["projects"]["items"]),
            ("/thoughts/0", thought),
            ("/thoughts/0/sections/0", section),
            (
                "/thoughts/0/sections/0/items/0",
                &section["properties"]["items"]["items"],
            ),
            ("/webring", &properties["webring"]),
        ];
        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        for (path, object) in objects {
            assert_eq!(object["additionalProperties"], false, "{path}");
            let declared = keys(&object["properties"]);
            let mut required: Vec<String> =
                serde_json::from_value(object["required"].clone()).unwrap();
            required.sort();
            assert_eq!(required, declared, "{path}: schema required vs properties");
            assert_eq!(
                keys(snapshot.pointer(path).unwrap()),
                declared,
                "{path}: snapshot"
            );
            for key in required {
                let mut json = snapshot.clone();
                let fields = json.pointer_mut(path).unwrap().as_object_mut().unwrap();
                fields.remove(&key);
                assert!(
                    Portfolio::from_json(&json.to_string()).is_err(),
                    "Rust accepts {path} without {key}"
                );
                let nullable = object["properties"][&key]["oneOf"]
                    .as_array()
                    .is_some_and(|options| options.iter().any(|option| option["type"] == "null"));
                let fields = json.pointer_mut(path).unwrap().as_object_mut().unwrap();
                fields.insert(key.clone(), serde_json::Value::Null);
                assert_eq!(
                    Portfolio::from_json(&json.to_string()).is_ok(),
                    nullable,
                    "{path}.{key} = null"
                );
            }
        }

        // Sample strings through the schema's pattern and length bounds and through Rust.
        let schema_accepts = |def: &str, text: &str| {
            let def = &schema["$defs"][def];
            let pattern = regex::Regex::new(def["pattern"].as_str().unwrap()).unwrap();
            let length = text.chars().count() as u64;
            pattern.is_match(text)
                && def["minLength"].as_u64().is_none_or(|min| length >= min)
                && def["maxLength"].as_u64().is_none_or(|max| length <= max)
        };
        let label = "a".repeat(63);
        let long_path = format!("https://example.com/{}", "a".repeat(URL_LIMIT - 20));
        let urls = [
            "https://example.com",
            "https://www.example.com/",
            "https://example.com/path?a=b#fragment",
            "https://example.com/some%20path",
            "https://EXAMPLE.co.uk/a-b_c~d.e:f@g!h$i&j'k(l)m*n+o,p;q=r[s]",
            "https://example.com?query",
            "https://example.com#fragment",
            "https://1.2.3.4/",
            "https://a-b.example/",
            "mailto:user+portfolio@example.com",
            "mailto:first.last-x_y@mail.example.com",
            &format!("https://{label}.com"),
            &long_path,
            &format!("{long_path}a"),
            &format!("https://{label}a.com"),
            "",
            "https://",
            "https://example",
            "https://example.com:8080/",
            "https://user@example.com",
            "https://good.com@evil.com",
            "https://ex_ample.com",
            "https://-example.com",
            "https://example-.com",
            "https://example..com",
            "https://.example.com",
            "https://example.com.",
            "https://example.com/a b",
            "https://example.com/a%xx",
            "https://example.com/a%2",
            "https://example.com/\"<>",
            "https://example.com/{}|^`",
            "https://example.com/\\evil",
            "https://example.com/ü",
            "https://example.com\x1b]8;;",
            "https://example.com\n",
            " https://example.com",
            "HTTPS://example.com",
            "http://example.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "mailto:user@example.com?body=hello",
            "mailto:@example.com",
            "mailto:user@example",
            "mailto:us er@example.com",
            "mailto:user@@example.com",
            "mailto:user@a.com@b.com",
            "mailto:user%40@example.com",
        ];
        for url in urls {
            assert_eq!(
                schema_accepts("url", url),
                validate_url("url", url).is_ok(),
                "schema and Rust disagree on {url:?}"
            );
        }
        // Text the schema can describe (no control or invisible characters).
        let texts = [
            "a",
            " a ",
            "Aidan Jeon",
            "漢字",
            "",
            " ",
            "\u{3000}",
            "\u{a0}",
        ];
        let long = ["a", "漢"].map(|ch| (ch.repeat(SHORT_TEXT_LIMIT), ch.repeat(BODY_TEXT_LIMIT)));
        for text in texts.iter().copied().chain(
            long.iter()
                .flat_map(|(short, body)| [short.as_str(), body.as_str()]),
        ) {
            for (def, limit) in [
                ("shortText", SHORT_TEXT_LIMIT),
                ("bodyText", BODY_TEXT_LIMIT),
            ] {
                for text in [text.to_string(), format!("{text}b")] {
                    assert_eq!(
                        schema_accepts(def, &text),
                        validate_text("t", &text, limit).is_ok(),
                        "schema {def} and Rust disagree on {text:?}"
                    );
                }
            }
        }
        let max_slug = "a".repeat(64);
        let long_slug = "a".repeat(65);
        for slug in [
            "a", "a-b", "ab-12-c", &max_slug, &long_slug, "", "-a", "a-", "a--b", "A", "a_b",
            "a b", "é",
        ] {
            assert_eq!(
                schema_accepts("slug", slug),
                validate_slug("slug", slug).is_ok(),
                "schema and Rust disagree on slug {slug:?}"
            );
        }

        let encoded = serde_json::to_string(&Portfolio::bundled().unwrap()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            snapshot
        );
    }
}
