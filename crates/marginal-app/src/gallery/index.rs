//! The Typora theme gallery's listing. Its site is built from one Markdown post per theme in
//! `typora/theme.typora.io` (branch `gh-pages`); each post's front matter names the theme, its
//! author, tags, a thumbnail and a download link. We list the posts with one GitHub API call,
//! read their front matter, and cache the result.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const REPO_RAW: &str = "https://raw.githubusercontent.com/typora/theme.typora.io/gh-pages";
const TREE: &str = "https://api.github.com/repos/typora/theme.typora.io/git/trees/gh-pages?recursive=1";
/// How long a cached index is used before fetching again.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Stable id from the post's file name, e.g. `night`.
    pub slug: String,
    pub title: String,
    pub author: String,
    pub tags: Vec<String>,
    /// Full URL of the thumbnail, when the post has one.
    pub thumbnail: Option<String>,
    pub homepage: Option<String>,
    pub download: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cached {
    /// Seconds since the Unix epoch when the index was fetched.
    pub fetched_at: u64,
    pub entries: Vec<Entry>,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Cached {
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    pub fn is_fresh(&self) -> bool {
        now_secs().saturating_sub(self.fetched_at) < MAX_AGE.as_secs()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        marginal_session::write_atomic(path, &serde_json::to_vec(self).map_err(std::io::Error::other)?)
    }
}

/// Percent-encodes what URLs can't hold raw (spaces, `&`, `#`…), keeping `/`.
pub(super) fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

pub(super) fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut request =
        ureq::get(url).header("User-Agent", "Marginal (https://github.com/typora/theme.typora.io reader)");
    // GitHub's API allows 60 anonymous requests an hour; a personal token (optional, free)
    // lifts that for bulk use such as the gallery survey.
    if url.starts_with("https://api.github.com/")
        && let Ok(token) = std::env::var("MARGINAL_GITHUB_TOKEN")
    {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|e| format!("{url}: {e}"))?;
    response.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("{url}: {e}"))
}

/// Fetches the whole gallery listing: the post list, then every post's front matter (8 at a
/// time). Posts that fail to load are skipped.
pub fn fetch() -> Result<Vec<Entry>, String> {
    #[derive(Deserialize)]
    struct Tree {
        tree: Vec<Node>,
    }
    #[derive(Deserialize)]
    struct Node {
        path: String,
        #[serde(rename = "type")]
        kind: String,
    }
    let tree: Tree =
        serde_json::from_slice(&get(TREE, 16 << 20)?).map_err(|e| format!("unexpected gallery listing: {e}"))?;
    let posts: Vec<String> = tree
        .tree
        .into_iter()
        .filter(|n| n.kind == "blob" && n.path.starts_with("_posts/") && n.path.ends_with(".md"))
        .filter(|n| !n.path.starts_with("_posts/doc/"))
        .map(|n| n.path)
        .collect();
    if posts.is_empty() {
        return Err("the gallery listing has no themes".into());
    }
    let chunks: Vec<Vec<String>> = posts.chunks(posts.len().div_ceil(8)).map(<[String]>::to_vec).collect();
    let mut entries: Vec<Entry> = std::thread::scope(|scope| {
        let workers: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|path| {
                            let url = format!("{REPO_RAW}/{}", encode_path(path));
                            let text = String::from_utf8(get(&url, 1 << 20).ok()?).ok()?;
                            parse_post(path, &text)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    entries.sort_by_key(|e| e.title.to_lowercase());
    entries.dedup_by(|a, b| a.slug == b.slug);
    Ok(entries)
}

/// Reads a gallery post's front matter. `None` when it isn't a theme (no title or download).
pub fn parse_post(path: &str, text: &str) -> Option<Entry> {
    let body = text.trim_start().strip_prefix("---")?;
    let front = &body[..body.find("\n---")?];
    let mut entry = Entry::default();
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        let unquoted = value.trim_matches(|c| c == '"' || c == '\'').trim().to_owned();
        let some = |v: String| (!v.is_empty()).then_some(v);
        match key.trim() {
            "title" => entry.title = unquoted,
            "author" => entry.author = unquoted,
            "homepage" => entry.homepage = some(unquoted),
            "download" => entry.download = some(unquoted),
            "preview" | "thumbnail" => {
                entry.thumbnail = some(unquoted).map(|p| match p {
                    p if p.starts_with("http") => p,
                    p if p.contains('/') => format!("{REPO_RAW}/{}", p.trim_start_matches('/')),
                    p => format!("{REPO_RAW}/media/thumbnails/{p}"),
                })
            }
            "tags" => {
                entry.tags = value
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(|t| t.trim().trim_matches(|c| c == '"' || c == '\'').to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    let stem = Path::new(path).file_stem()?.to_string_lossy();
    // Posts are named `YYYY-MM-DD-slug.md`.
    let slug = match stem.splitn(4, '-').collect::<Vec<_>>().as_slice() {
        [y, m, d, rest]
            if y.len() == 4 && (1..=2).contains(&m.len()) && (1..=2).contains(&d.len()) && y.parse::<u16>().is_ok() =>
        {
            rest.to_lowercase().replace(' ', "-")
        }
        _ => stem.to_lowercase(),
    };
    entry.slug = slug;
    (!entry.title.is_empty() && (entry.download.is_some() || entry.homepage.is_some())).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POST: &str = r#"---
layout: post
title: "Fuwari Light"
author: "Caphhh"
preview: "fuwari-light.png"
homepage: "https://github.com/Caph-dev/typora-fuwari-theme"
download: "https://github.com/Caph-dev/typora-fuwari-theme/archive/refs/heads/main.zip"
tags: [light, soft, card, cjk, clean]
---

A soft, card-style light theme.
"#;

    #[test]
    fn parses_front_matter() {
        let e = parse_post("_posts/2026-06-24-fuwari-light.md", POST).unwrap();
        assert_eq!(e.slug, "fuwari-light");
        assert_eq!(e.title, "Fuwari Light");
        assert_eq!(e.author, "Caphhh");
        assert_eq!(e.tags, ["light", "soft", "card", "cjk", "clean"]);
        assert_eq!(e.thumbnail.as_deref(), Some(&*format!("{REPO_RAW}/media/thumbnails/fuwari-light.png")));
        assert!(e.download.unwrap().ends_with("main.zip"));
        assert!(parse_post("_posts/doc/x.md", "---\nlayout: post\n---\n").is_none(), "not a theme");
        let e = parse_post("_posts/theme/2024-8-27-Jinxiu Crimson.md", POST).unwrap();
        assert_eq!(e.slug, "jinxiu-crimson");
        assert_eq!(encode_path("_posts/a b&c.md"), "_posts/a%20b%26c.md");
    }

    /// Hits the real gallery: `cargo test -p marginal-app live_gallery -- --ignored --nocapture`.
    #[test]
    #[ignore = "uses the network"]
    fn live_gallery() {
        let entries = fetch().unwrap();
        println!("{} themes", entries.len());
        assert!(entries.len() > 100);
        let with_thumbs = entries.iter().filter(|e| e.thumbnail.is_some()).count();
        let downloadable = entries
            .iter()
            .filter(|e| e.download.as_deref().and_then(super::super::download::resolve).is_some())
            .count();
        println!("{with_thumbs} with thumbnails, {downloadable} with a downloadable link");
        let night = entries.iter().find(|e| e.slug == "night" || e.title == "Night").expect("Night is listed");
        println!("{night:?}");
        let dir = tempfile::tempdir().unwrap();
        let variants =
            super::super::download::download(night.download.as_deref(), night.homepage.as_deref(), dir.path()).unwrap();
        println!("{variants:?}");
        let thumb = get(night.thumbnail.as_ref().unwrap(), 4 << 20).unwrap();
        assert!(super::super::decode_png(&thumb).is_some(), "thumbnail decodes");
    }
}
