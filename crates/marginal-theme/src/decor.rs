//! `::before`/`::after` decorations and the fonts a theme ships with (`@font-face`).

use crate::{PseudoElement, Style};

/// A piece of a `content` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentPart {
    Text(String),
    /// `counter(name)` or `counter(name, style)`: Marginal numbers headings itself, so these
    /// resolve to heading numbers.
    Counter {
        name: String,
        style: Option<String>,
    },
}

/// A resolved `::before` or `::after` box.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoration {
    pub pseudo: PseudoElement,
    pub content: Vec<ContentPart>,
    pub style: Style,
    /// `display: block` (or flex, table…): a line of its own above or below the element.
    pub block: bool,
    /// `position: absolute`: placed by its offsets without moving the element's text.
    pub absolute: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub right: Option<f32>,
}

impl Decoration {
    /// The text, with counters resolved by `counter(name, style)`.
    pub fn text(&self, counter: impl Fn(&str, Option<&str>) -> String) -> String {
        self.content
            .iter()
            .map(|p| match p {
                ContentPart::Text(t) => t.clone(),
                ContentPart::Counter { name, style } => counter(name, style.as_deref()),
            })
            .collect()
    }

    /// Whether this is a shape rather than text (`content: ""` with a size or a background).
    pub fn is_shape(&self) -> bool {
        self.content.iter().all(|p| matches!(p, ContentPart::Text(t) if t.trim().is_empty()))
    }
}

/// Text that only repeats Markdown syntax (`#`, `H2`, `>`, a backtick): Marginal already shows
/// syntax in the margin, so such decorations are dropped.
pub fn restates_syntax(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let all = |set: &str| t.chars().all(|c| set.contains(c) || c.is_whitespace());
    let heading_label = t.len() == 2 && t.to_ascii_lowercase().starts_with('h') && t.as_bytes()[1].is_ascii_digit();
    all("#") || all(">") || all("`") || all("*_~") || all("|") || all("-+") || heading_label
}

/// Text that can't render: icon-font private-use glyphs.
pub fn is_icon_glyph(text: &str) -> bool {
    text.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c) || ('\u{F0000}'..='\u{FFFFD}').contains(&c))
}

/// Decodes CSS string escapes: `\201C` → “, `\"` → ".
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            hex.push(chars.next().unwrap());
        }
        if hex.is_empty() {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            if chars.peek() == Some(&' ') {
                chars.next();
            }
            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(ch);
            }
        }
    }
    out
}

/// Parses a `content` value. `None` for `none`/`normal` and for anything Marginal can't resolve
/// (`attr()`, `url()`, icon variables).
pub fn parse_content(value: &str) -> Option<Vec<ContentPart>> {
    let v = value.trim().trim_end_matches("!important").trim();
    if v == "none" || v == "normal" || v.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    let mut rest = v;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let body = &rest[1..];
            let mut end = None;
            let mut escaped = false;
            for (i, c) in body.char_indices() {
                if c == q && !escaped {
                    end = Some(i);
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
            let end = end?;
            parts.push(ContentPart::Text(unescape(&body[..end])));
            rest = &body[end + 1..];
        } else {
            let r = rest.strip_prefix("counter(")?;
            let close = r.find(')')?;
            let mut args = r[..close].split(',').map(str::trim);
            let name = args.next()?.to_owned();
            let style = args.next().map(str::to_owned);
            parts.push(ContentPart::Counter { name, style });
            rest = &r[close + 1..];
        }
    }
    Some(parts)
}

/// A font a theme ships with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFace {
    /// The name the theme uses for it in `font-family`.
    pub family: String,
    /// Local file references, in preference order (relative to the theme file).
    pub sources: Vec<String>,
}

/// Reads an `@font-face` rule's family and local sources from its CSS text.
pub fn parse_font_face(css: &str) -> Option<FontFace> {
    let body = &css[css.find('{')? + 1..css.rfind('}')?];
    let mut family = None;
    let mut sources = Vec::new();
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        match name.trim() {
            "font-family" => family = Some(value.trim().trim_matches(['"', '\'']).to_owned()),
            "src" => {
                let mut rest = value;
                while let Some(i) = rest.find("url(") {
                    rest = &rest[i + 4..];
                    let end = rest.find(')').unwrap_or(rest.len());
                    let url = rest[..end].trim().trim_matches(['"', '\'']).split(['?', '#']).next().unwrap_or_default();
                    if !url.contains("://") && !url.starts_with("data:") && !url.is_empty() {
                        sources.push(url.to_owned());
                    }
                    rest = &rest[end..];
                }
            }
            _ => {}
        }
    }
    let family = family?;
    (!sources.is_empty()).then_some(FontFace { family, sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_values() {
        assert_eq!(parse_content(r#""\201C""#), Some(vec![ContentPart::Text("“".into())]));
        assert_eq!(
            parse_content(r#"counter(h1) "." counter(h2, lower-roman) " ""#),
            Some(vec![
                ContentPart::Counter { name: "h1".into(), style: None },
                ContentPart::Text(".".into()),
                ContentPart::Counter { name: "h2".into(), style: Some("lower-roman".into()) },
                ContentPart::Text(" ".into()),
            ])
        );
        assert_eq!(parse_content("none"), None);
        assert_eq!(parse_content("attr(data-title)"), None);
        assert_eq!(parse_content(r#""""#), Some(vec![ContentPart::Text(String::new())]));
    }

    #[test]
    fn syntax_and_icons() {
        for t in ["#", "## ", "H2", "h6", ">", "`", "**", "|"] {
            assert!(restates_syntax(t), "{t}");
        }
        for t in ["“", "✓", "§", "Note:", ""] {
            assert!(!restates_syntax(t), "{t}");
        }
        assert!(is_icon_glyph("\u{f0da}") && !is_icon_glyph("✓"));
    }

    #[test]
    fn font_faces() {
        let f = parse_font_face(
            r#"@font-face { font-family: "Fira Sans"; src: url("fonts/fira.woff2") format("woff2"), url(fonts/fira.ttf), url(https://cdn/x.woff) }"#,
        )
        .unwrap();
        assert_eq!(f.family, "Fira Sans");
        assert_eq!(f.sources, ["fonts/fira.woff2", "fonts/fira.ttf"]);
        assert!(parse_font_face(r#"@font-face { font-family: X; src: url(https://cdn/x.woff) }"#).is_none());
    }
}
