use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const BUNDLED_JSON: &str = include_str!("../content/portfolio.json");
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const CONTENT_VERSION: u8 = 2;
const SHORT_TEXT_LIMIT: usize = 128;
const BODY_TEXT_LIMIT: usize = 2048;
const URL_LIMIT: usize = 2048;

/// Public portfolio content, contract v2. The website serves it at /portfolio.json.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Portfolio {
    pub version: u8,
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
        let content: Self = serde_json::from_str(json)
            .map_err(|error| ContentError(format!("invalid portfolio JSON: {error}")))?;
        content.validate()?;
        Ok(content)
    }

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
                }
            }
            if paragraph.iter().all(|span| span.text.trim().is_empty()) {
                return Err(ContentError(format!("{field} has no visible text")));
            }
        }
        validate_count("work", self.work.len(), 64)?;
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
        validate_count("projects", self.projects.len(), 64)?;
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
        validate_count("thoughts", self.thoughts.len(), 64)?;
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
                validate_count(&format!("{field}.items"), section.items.len(), 64)?;
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

fn validate_text(field: &str, text: &str, maximum: usize) -> Result<(), ContentError> {
    if text.trim().is_empty() || text.chars().count() > maximum {
        return Err(ContentError(format!(
            "{field} must contain 1–{maximum} visible characters"
        )));
    }
    reject_unsafe_chars(field, text)
}

fn reject_unsafe_chars(field: &str, text: &str) -> Result<(), ContentError> {
    if text.chars().any(|ch| {
        ch.is_control()
            || matches!(ch, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
    }) {
        return Err(ContentError(format!("{field} contains terminal control or invisible formatting characters")));
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
            let mut content = Portfolio::bundled().unwrap();
            content.intro[0][0].text = unsafe_text.into();
            assert!(content.validate().is_err(), "accepted {unsafe_text:?}");
            let mut content = Portfolio::bundled().unwrap();
            content.thoughts[0].sections[0].items[0].text = unsafe_text.into();
            assert!(content.validate().is_err(), "accepted {unsafe_text:?}");
        }
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
        let mut content = Portfolio::bundled().unwrap();
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
        let mut content = Portfolio::bundled().unwrap();
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
        let mut content = Portfolio::bundled().unwrap();
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
        let snapshot: serde_json::Value = serde_json::from_str(BUNDLED_JSON).unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        for (key, _) in snapshot.as_object().unwrap() {
            assert!(required.contains(&key.as_str()), "schema missing {key}");
        }
        let encoded = serde_json::to_string(&Portfolio::bundled().unwrap()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            snapshot
        );
    }
}
