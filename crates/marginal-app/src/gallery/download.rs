//! Downloading a gallery theme: a `.zip` (usually a GitHub archive) or a single `.css`,
//! extracted with guards into a cache folder, then installed into the user's themes folder.

use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use super::index::get;

/// Most bytes fetched file by file from a repository.
pub const MAX_DOWNLOAD: u64 = 20 << 20;
/// Largest archive accepted when a repository has to be downloaded whole.
const MAX_ARCHIVE: u64 = 80 << 20;
/// Largest total size extracted from one archive.
const MAX_EXTRACTED: u64 = 50 << 20;
/// File types kept from an archive: the stylesheet and the assets it may reference.
const KEEP: &[&str] = &["css", "woff", "woff2", "ttf", "otf", "png", "jpg", "jpeg", "gif", "svg", "webp"];

/// One stylesheet found in a download; a theme may ship several (light and dark variants).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub css: PathBuf,
}

/// Turns a gallery download or homepage link into something fetchable: GitHub repository
/// pages become their archive zip, GitHub file pages become raw files. `None` when the link
/// can't be downloaded directly (a releases page, a website).
pub fn resolve(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_lowercase();
    let github = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("http://github.com/"));
    let Some(rest) = github.filter(|_| !lower.contains("/archive/")) else {
        let path = lower.split(['?', '#']).next().unwrap_or_default();
        let direct = path.ends_with(".zip")
            || path.ends_with(".css")
            || lower.contains("/archive/")
            || lower.contains("codeload.github.com/");
        return direct.then(|| url.to_owned());
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [owner, repo] => Some(format!("https://github.com/{owner}/{repo}/archive/HEAD.zip")),
        [owner, repo, "tree", branch] => Some(format!("https://github.com/{owner}/{repo}/archive/{branch}.zip")),
        // A releases page: the source archive holds the same stylesheets.
        [owner, repo, "releases", ..] => Some(format!("https://github.com/{owner}/{repo}/archive/HEAD.zip")),
        [owner, repo, "blob" | "raw", branch, path @ ..]
            if path
                .last()
                .is_some_and(|p| p.to_lowercase().ends_with(".css") || p.to_lowercase().ends_with(".zip")) =>
        {
            Some(format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{}", path.join("/")))
        }
        _ => None,
    }
}

/// A GitHub repository and the branch or tag a link points at (`None`: the default branch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    pub reference: Option<String>,
}

/// The GitHub repository behind a repository, archive, releases or codeload link.
pub fn github_repo(url: &str) -> Option<Repo> {
    let url = url.trim().split(['?', '#']).next()?;
    let (host_rest, codeload) =
        match url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("http://github.com/")) {
            Some(r) => (r, false),
            None => (url.strip_prefix("https://codeload.github.com/")?, true),
        };
    let parts: Vec<&str> = host_rest.trim_end_matches('/').split('/').collect();
    let repo = |owner: &str, name: &str, reference: Option<&str>| {
        Some(Repo {
            owner: owner.into(),
            name: name.trim_end_matches(".git").into(),
            reference: reference.map(Into::into),
        })
    };
    let strip_zip = |s: &str| s.strip_suffix(".zip").or(s.strip_suffix(".tar.gz")).unwrap_or(s).to_owned();
    match parts.as_slice() {
        [owner, name, "zip" | "tar.gz", rest @ ..] if codeload => repo(owner, name, Some(&rest.join("/"))),
        [owner, name] => repo(owner, name, None),
        [owner, name, "tree", reference @ ..] if !reference.is_empty() => repo(owner, name, Some(&reference.join("/"))),
        [owner, name, "archive", "refs", "heads" | "tags", reference] => repo(owner, name, Some(&strip_zip(reference))),
        [owner, name, "archive", reference] => {
            let r = strip_zip(reference);
            repo(owner, name, (r != "HEAD").then_some(r.as_str()))
        }
        [owner, name, "releases", ..] => repo(owner, name, None),
        _ => None,
    }
}

/// Folders that hold a repository's docs, demos and sources rather than the theme.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".github",
    "docs",
    "doc",
    "demo",
    "demos",
    "screenshot",
    "screenshots",
    "example",
    "examples",
    "test",
    "tests",
    "site",
    "website",
    "_site",
    "preview",
    "previews",
    "img",
    "images",
    "src",
    "scss",
    "sass",
    "less",
];

/// The stylesheet's relative references: `url(...)` assets and `@import`ed sheets.
fn references(css: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = css;
    while let Some(i) = rest.find("url(") {
        rest = &rest[i + 4..];
        let end = rest.find(')').unwrap_or(rest.len());
        refs.push(rest[..end].trim().trim_matches(['"', '\'']).to_owned());
        rest = &rest[end..];
    }
    let mut rest = css;
    while let Some(i) = rest.find("@import") {
        rest = &rest[i + 7..];
        let line = rest.split(';').next().unwrap_or_default().trim();
        if let Some(q) = line.strip_prefix('"').or_else(|| line.strip_prefix('\'')) {
            refs.push(q.split(['"', '\'']).next().unwrap_or_default().to_owned());
        }
    }
    refs.into_iter()
        .map(|r| r.split(['?', '#']).next().unwrap_or_default().to_owned())
        .filter(|r| {
            !r.is_empty()
                && !r.contains("://")
                && !r.starts_with("data:")
                && !r.starts_with("//")
                && !r.starts_with('/')
        })
        .collect()
}

/// `a/b/../c/d.css` → `a/c/d.css`; `None` if it climbs out of the repository.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    Some(out.join("/"))
}

/// Fetches just a theme's stylesheets and the files they reference from a GitHub repository,
/// keeping the repository's layout under `dir`. Much smaller than the archive, which often
/// carries screenshots and demo documents.
fn fetch_github(repo: &Repo, dir: &Path) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    struct Tree {
        tree: Vec<Node>,
    }
    #[derive(serde::Deserialize)]
    struct Node {
        path: String,
        #[serde(rename = "type")]
        kind: String,
        size: Option<u64>,
    }
    let reference = repo.reference.clone().unwrap_or_else(|| "HEAD".into());
    let api = format!("https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1", repo.owner, repo.name, reference);
    let tree: Tree = serde_json::from_slice(&get(&api, 16 << 20)?).map_err(|e| format!("unexpected file list: {e}"))?;
    let files: std::collections::HashMap<String, u64> =
        tree.tree.into_iter().filter(|n| n.kind == "blob").map(|n| (n.path, n.size.unwrap_or(0))).collect();
    let in_skipped = |p: &str| p.split('/').rev().skip(1).any(|d| SKIP_DIRS.contains(&d.to_lowercase().as_str()));
    let css: Vec<&String> = files
        .keys()
        .filter(|p| p.to_lowercase().ends_with(".css") && !p.ends_with(".min.css") && !in_skipped(p))
        .collect();
    let Some(min) = css.iter().map(|p| p.matches('/').count()).min() else {
        return Err("no stylesheet in the repository".into());
    };
    let mut queue: Vec<String> = css.into_iter().filter(|p| p.matches('/').count() == min).take(20).cloned().collect();
    let mut fetched = std::collections::HashSet::new();
    let mut total = 0u64;
    while let Some(path) = queue.pop() {
        if !fetched.insert(path.clone()) {
            continue;
        }
        let size = files.get(&path).copied().unwrap_or(0);
        total += size;
        if total > MAX_DOWNLOAD {
            break;
        }
        let raw = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            repo.owner,
            repo.name,
            reference,
            super::index::encode_path(&path)
        );
        let Ok(bytes) = get(&raw, 8 << 20) else { continue };
        if path.to_lowercase().ends_with(".css")
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            let base = path.rsplit_once('/').map_or("", |(d, _)| d);
            for r in references(text) {
                let joined = if base.is_empty() { r.clone() } else { format!("{base}/{r}") };
                if let Some(p) = normalize(&joined).filter(|p| files.contains_key(p)) {
                    queue.push(p);
                }
            }
        }
        let out = dir.join(&path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Downloads a gallery theme into `dir` (replacing what was there) and returns the stylesheets
/// found. GitHub repositories are fetched file by file; anything else as a zip or CSS file. The
/// homepage is the fallback when the download link isn't usable.
pub fn download(url: Option<&str>, homepage: Option<&str>, dir: &Path) -> Result<Vec<Variant>, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut last_error = String::from("this theme has no download link");
    let candidates = [url, homepage].into_iter().flatten();
    for link in candidates {
        if let Some(repo) = github_repo(link) {
            if fetch_github(&repo, dir).is_ok() {
                let found = variants(dir);
                if !found.is_empty() {
                    return Ok(found);
                }
            }
            // The file list can fail (rate limit, odd layout): try the archive instead.
            let archive = format!(
                "https://github.com/{}/{}/archive/{}.zip",
                repo.owner,
                repo.name,
                repo.reference.as_deref().unwrap_or("HEAD")
            );
            match download_file(&archive, dir) {
                Ok(found) => return Ok(found),
                Err(e) => last_error = e,
            }
        } else if let Some(resolved) = resolve(link) {
            match download_file(&resolved, dir) {
                Ok(found) => return Ok(found),
                Err(e) => last_error = e,
            }
        } else {
            last_error = "this theme's download link isn't a zip or CSS file".into();
        }
    }
    Err(last_error)
}

/// Downloads one zip or CSS file into `dir` and returns the stylesheets found.
fn download_file(url: &str, dir: &Path) -> Result<Vec<Variant>, String> {
    let bytes = get(url, MAX_ARCHIVE)?;
    if bytes.starts_with(b"PK\x03\x04") {
        extract(&bytes, dir)?;
    } else if std::str::from_utf8(&bytes).is_ok_and(|t| t.contains('{')) {
        let name = Path::new(url).file_stem().map_or("theme".into(), |s| s.to_string_lossy().into_owned());
        std::fs::write(dir.join(format!("{name}.css")), &bytes).map_err(|e| e.to_string())?;
    } else {
        return Err("the download isn't a zip or CSS file".into());
    }
    let variants = variants(dir);
    if variants.is_empty() { Err("no stylesheet in the download".into()) } else { Ok(variants) }
}

/// Extracts the archive's stylesheets and assets into `dir`, dropping the single top folder
/// GitHub archives wrap everything in. Unsafe paths and oversized archives are refused.
pub fn extract(bytes: &[u8], dir: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid zip: {e}"))?;
    let names: Vec<PathBuf> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().filter(|f| !f.is_dir()).and_then(|f| f.enclosed_name()))
        .collect();
    let common = common_top(&names);
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        // `enclosed_name` is None for absolute paths and anything escaping with `..`.
        let Some(name) = file.enclosed_name() else { continue };
        let ext = name.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !KEEP.contains(&ext.as_str()) {
            continue;
        }
        let relative = match &common {
            Some(top) => name.strip_prefix(top).unwrap_or(&name).to_path_buf(),
            None => name.clone(),
        };
        if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
            continue;
        }
        total += file.size();
        if total > MAX_EXTRACTED {
            return Err("the archive is too large to install".into());
        }
        let out = dir.join(&relative);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::new();
        (&mut file).take(MAX_EXTRACTED).read_to_end(&mut data).map_err(|e| e.to_string())?;
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The single top-level folder every file sits in, if there is one.
fn common_top(names: &[PathBuf]) -> Option<PathBuf> {
    let first = names.first()?.components().next()?;
    let top = PathBuf::from(first.as_os_str());
    let all_under = names.iter().all(|n| n.components().count() > 1 && n.starts_with(&top));
    all_under.then_some(top)
}

/// The theme stylesheets in `dir`: the `.css` files nearest the top (deeper ones are usually
/// assets or sources), sorted by name.
pub fn variants(dir: &Path) -> Vec<Variant> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(usize, PathBuf)>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, out);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("css")) {
                out.push((depth, path));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 0, &mut found);
    let Some(min) = found.iter().map(|(d, _)| *d).min() else { return Vec::new() };
    let mut variants: Vec<Variant> = found
        .into_iter()
        .filter(|(d, _)| *d == min)
        .filter_map(|(_, css)| {
            let stem = css.file_stem()?.to_string_lossy().into_owned();
            Some(Variant { name: title_case(&stem), css })
        })
        .collect();
    variants.sort_by(|a, b| a.name.cmp(&b.name));
    variants
}

/// `night-owl` → `Night Owl`.
fn title_case(stem: &str) -> String {
    stem.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The variants a gallery entry is about: a repository often bundles several themes (Typora's
/// defaults ship Github, Night, Newsprint… together), so prefer the ones named like the entry.
pub fn matching(variants: &[Variant], title: &str) -> Vec<Variant> {
    let norm = |s: &str| s.to_lowercase().replace(['-', '_', ' '], "");
    let t = norm(title);
    let exact: Vec<Variant> = variants.iter().filter(|v| norm(&v.name) == t).cloned().collect();
    if !exact.is_empty() {
        return exact;
    }
    let related: Vec<Variant> =
        variants.iter().filter(|v| norm(&v.name).contains(&t) || t.contains(&norm(&v.name))).cloned().collect();
    if related.is_empty() { variants.to_vec() } else { related }
}

/// Copies a downloaded variant into the themes folder as `<name>.css`, with the asset folder
/// Typora themes keep beside the stylesheet (`<stem>/`). Returns the installed name, which
/// avoids clashing with a built-in theme.
pub fn install(variant: &Variant, themes: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(themes)?;
    let mut name = variant.name.clone();
    if marginal_theme::BUILT_IN.iter().any(|(b, _)| b.eq_ignore_ascii_case(&name)) {
        name.push_str(" (Typora)");
    }
    std::fs::copy(&variant.css, themes.join(format!("{name}.css")))?;
    if let (Some(dir), Some(stem)) = (variant.css.parent(), variant.css.file_stem()) {
        let assets = dir.join(stem);
        if assets.is_dir() {
            copy_dir(&assets, &themes.join(&name))?;
        }
    }
    Ok(name)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// Removes an installed user theme: its stylesheet and asset folder.
pub fn uninstall(css: &Path) -> std::io::Result<()> {
    if let (Some(dir), Some(stem)) = (css.parent(), css.file_stem()) {
        let assets = dir.join(stem);
        if assets.is_dir() {
            std::fs::remove_dir_all(assets)?;
        }
    }
    std::fs::remove_file(css)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, body) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn resolves_github_links() {
        assert_eq!(resolve("https://github.com/a/b").unwrap(), "https://github.com/a/b/archive/HEAD.zip");
        assert_eq!(resolve("https://github.com/a/b/tree/dev/").unwrap(), "https://github.com/a/b/archive/dev.zip");
        assert_eq!(
            resolve("https://github.com/a/b/blob/main/themes/x.css").unwrap(),
            "https://raw.githubusercontent.com/a/b/main/themes/x.css"
        );
        assert_eq!(
            resolve("https://github.com/a/b/releases/latest").unwrap(),
            "https://github.com/a/b/archive/HEAD.zip"
        );
        assert_eq!(
            resolve("https://github.com/a/b/blob/gh-pages/media/x/x.zip?raw=true").unwrap(),
            "https://raw.githubusercontent.com/a/b/gh-pages/media/x/x.zip"
        );
        assert!(resolve("https://codeload.github.com/a/b/zip/master").is_some());
        assert!(resolve("https://example.com/my-theme").is_none());
    }

    #[test]
    fn extracts_safely_and_finds_variants() {
        let bytes = zip_of(&[
            ("repo-main/night.css", "#write { color: #ddd; }"),
            ("repo-main/night-dark.css", "#write { color: #eee; }"),
            ("repo-main/night/fonts/a.woff2", "font"),
            ("repo-main/src/build.css", "#write {}"),
            ("repo-main/../../escape.css", "#write {}"),
            ("repo-main/README.md", "readme"),
            ("repo-main/install.sh", "rm -rf /"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        extract(&bytes, dir.path()).unwrap();
        assert!(dir.path().join("night.css").exists(), "the archive's top folder is dropped");
        assert!(dir.path().join("night/fonts/a.woff2").exists());
        assert!(!dir.path().join("install.sh").exists() && !dir.path().join("README.md").exists());
        assert!(!dir.path().parent().unwrap().join("escape.css").exists(), "no path escapes");
        let names: Vec<String> = variants(dir.path()).into_iter().map(|v| v.name).collect();
        assert_eq!(names, ["Night", "Night Dark"]);
        let found = variants(dir.path());
        assert_eq!(matching(&found, "night").len(), 1, "an exact name wins");
        assert_eq!(matching(&found, "Night Dark")[0].name, "Night Dark");
        assert_eq!(matching(&found, "Unrelated").len(), 2, "no match keeps them all");

        let themes = tempfile::tempdir().unwrap();
        let v = variants(dir.path()).remove(0);
        assert_eq!(install(&v, themes.path()).unwrap(), "Night");
        assert!(themes.path().join("Night.css").exists() && themes.path().join("Night/fonts/a.woff2").exists());
        uninstall(&themes.path().join("Night.css")).unwrap();
        assert!(!themes.path().join("Night").exists());
    }

    #[test]
    fn recognizes_github_links_and_references() {
        let r = |owner: &str, name: &str, reference: Option<&str>| {
            Some(Repo { owner: owner.into(), name: name.into(), reference: reference.map(Into::into) })
        };
        assert_eq!(github_repo("https://github.com/a/b"), r("a", "b", None));
        assert_eq!(github_repo("https://github.com/a/b/archive/refs/heads/main.zip"), r("a", "b", Some("main")));
        assert_eq!(github_repo("https://github.com/a/b/archive/master.zip"), r("a", "b", Some("master")));
        assert_eq!(github_repo("https://github.com/a/b/releases/latest"), r("a", "b", None));
        assert_eq!(github_repo("https://codeload.github.com/a/b/zip/master"), r("a", "b", Some("master")));
        assert_eq!(github_repo("https://example.com/a/b"), None);
        let css = r#"@import "base/fonts.css"; @import url('x.css'); h1 { background: url("img/bg.png?v=2") }
                     @font-face { src: url(fonts/a.woff2), url(https://cdn/x.woff) } p { background: url(data:abc) }"#;
        assert_eq!(references(css), ["x.css", "img/bg.png", "fonts/a.woff2", "base/fonts.css"]);
        assert_eq!(normalize("a/b/../c/./d.css").unwrap(), "a/c/d.css");
        assert!(normalize("../x.css").is_none());
    }

    #[test]
    fn rejects_non_zips() {
        assert!(extract(b"not a zip", Path::new("/nonexistent")).is_err());
    }
}
