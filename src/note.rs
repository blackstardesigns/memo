use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

/// Frontmatter metadata stored at the top of every note file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub title: String,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    /// True once the user has set the title themselves; refine won't auto-overwrite it.
    #[serde(default)]
    pub title_custom: bool,
    /// Id of the [`Folder`] this note lives in, or `None` for the top level.
    /// Omitted from the frontmatter when the note isn't filed in a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// A user-created folder for organising notes. Folders are stored separately
/// from notes (see [`crate::storage::Store::list_folders`]); a note joins a
/// folder by referencing its id in [`Meta::folder`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub title: String,
    pub created: DateTime<Utc>,
}

impl Folder {
    pub fn new(title: String) -> Self {
        Folder {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            created: Utc::now(),
        }
    }
}

/// A note: its metadata, the original body, and an optional refined version
/// (loaded from the `<id>.refined.md` sidecar when present).
#[derive(Debug, Clone)]
pub struct Note {
    pub meta: Meta,
    pub content: String,
    pub refined: Option<String>,
}

impl Note {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let now = Utc::now();
        let title = format!(
            "Untitled Note {}",
            now.with_timezone(&Local).format("%Y-%m-%d %H:%M")
        );
        Note {
            meta: Meta {
                id: uuid::Uuid::new_v4().to_string(),
                title,
                created: now,
                modified: now,
                title_custom: false,
                folder: None,
            },
            content: String::new(),
            refined: None,
        }
    }

    /// Serialize the note (metadata + original body) to its on-disk `.md` representation.
    pub fn to_file_string(&self) -> Result<String> {
        let yaml = serde_yaml::to_string(&self.meta).context("serializing frontmatter")?;
        Ok(format!("---\n{yaml}---\n{}", self.content))
    }
}

/// Split a note file into its frontmatter [`Meta`] and the Markdown body.
pub fn parse(text: &str) -> Result<(Meta, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .context("note file is missing YAML frontmatter")?;
    if let Some(end) = rest.find("\n---\n") {
        let meta: Meta = serde_yaml::from_str(&rest[..end]).context("parsing frontmatter")?;
        return Ok((meta, rest[end + 5..].to_string()));
    }
    // Frontmatter terminator at end of file (no trailing body).
    if let Some(end) = rest.find("\n---") {
        let meta: Meta = serde_yaml::from_str(&rest[..end]).context("parsing frontmatter")?;
        return Ok((meta, String::new()));
    }
    anyhow::bail!("note file frontmatter is not terminated by `---`")
}

/// Derive a concise title from Markdown content: the first non-empty line with
/// heading/bullet/quote markers and inline emphasis stripped, capped at 60 chars.
pub fn derive_title(md: &str) -> Option<String> {
    for line in md.lines() {
        let stripped = line
            .trim()
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '>'])
            .trim();
        let cleaned: String = stripped
            .chars()
            .filter(|c| !matches!(c, '*' | '_' | '`'))
            .collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return Some(cleaned.chars().take(60).collect());
        }
    }
    None
}

/// Turn a title into a filesystem-friendly slug for export filenames.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "memo".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_meta_and_body() {
        let mut n = Note::new();
        n.meta.title = "Hello World".into();
        n.content = "# Heading\n- a\n- b".into();
        let s = n.to_file_string().unwrap();
        let (meta, body) = parse(&s).unwrap();
        assert_eq!(meta.title, "Hello World");
        assert_eq!(meta.id, n.meta.id);
        assert_eq!(body, "# Heading\n- a\n- b");
    }

    #[test]
    fn roundtrip_empty_body() {
        let n = Note::new();
        let s = n.to_file_string().unwrap();
        let (_, body) = parse(&s).unwrap();
        assert_eq!(body, "");
    }

    #[test]
    fn slugify_examples() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  multiple   spaces "), "multiple-spaces");
        assert_eq!(slugify("!!!"), "memo");
        assert_eq!(slugify(""), "memo");
    }

    #[test]
    fn parse_rejects_missing_frontmatter() {
        assert!(parse("just some text").is_err());
    }

    #[test]
    fn derive_title_from_markdown() {
        assert_eq!(
            derive_title("# My Heading\nbody"),
            Some("My Heading".into())
        );
        assert_eq!(
            derive_title("\n\n- first bullet"),
            Some("first bullet".into())
        );
        assert_eq!(
            derive_title("**Bold** intro line"),
            Some("Bold intro line".into())
        );
        assert_eq!(derive_title("   \n  "), None);
    }

    #[test]
    fn meta_title_custom_defaults_false_when_absent() {
        let n = Note::new();
        let s = n.to_file_string().unwrap();
        // Old notes won't have `title_custom`; ensure it parses with serde default.
        let without = s
            .lines()
            .filter(|l| !l.contains("title_custom"))
            .collect::<Vec<_>>()
            .join("\n");
        let (meta, _) = parse(&without).unwrap();
        assert!(!meta.title_custom);
    }
}
