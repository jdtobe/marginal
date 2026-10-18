//! The themes the reader can pick: the built-ins plus any `.css` in the user's themes folder,
//! which reload when their file changes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use marginal_theme::{BUILT_IN, Theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeEntry {
    pub name: String,
    /// `None` for a built-in theme.
    pub path: Option<PathBuf>,
}

pub struct Themes {
    pub folder: PathBuf,
    pub entries: Vec<ThemeEntry>,
    /// The selected theme and, for a user theme, the file time it was loaded at.
    selected: String,
    loaded_at: Option<SystemTime>,
}

impl Themes {
    pub fn new(folder: PathBuf, selected: Option<String>) -> Self {
        let mut themes = Self {
            folder,
            entries: Vec::new(),
            selected: selected.unwrap_or_else(|| BUILT_IN[0].0.to_owned()),
            loaded_at: None,
        };
        themes.rescan();
        if !themes.entries.iter().any(|e| e.name == themes.selected) {
            themes.selected = BUILT_IN[0].0.to_owned();
        }
        themes
    }

    pub fn selected(&self) -> &str {
        &self.selected
    }

    /// Lists the built-ins and the folder's `.css` files (sorted by name). Returns whether the
    /// list changed.
    pub fn rescan(&mut self) -> bool {
        let mut entries: Vec<ThemeEntry> =
            BUILT_IN.iter().map(|(name, _)| ThemeEntry { name: (*name).to_owned(), path: None }).collect();
        let mut user: Vec<ThemeEntry> = std::fs::read_dir(&self.folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("css")))
            .filter_map(|p| {
                let name = p.file_stem()?.to_string_lossy().into_owned();
                Some(ThemeEntry { name, path: Some(p) })
            })
            .collect();
        user.sort_by_key(|e| e.name.to_lowercase());
        entries.extend(user);
        let changed = entries != self.entries;
        self.entries = entries;
        changed
    }

    /// Loads a theme by name. A user theme that fails to parse keeps the reader on the current
    /// theme and returns the error.
    pub fn load(&mut self, name: &str) -> Result<Theme, String> {
        let entry = self.entries.iter().find(|e| e.name == name).cloned().ok_or(format!("no theme named {name}"))?;
        let theme = match &entry.path {
            None => Theme::built_in(name).ok_or(format!("no theme named {name}"))?,
            Some(path) => {
                let css = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let theme = Theme::parse(&css).map_err(|e| format!("{}: {e}", path.display()))?;
                for d in &theme.diagnostics {
                    eprintln!("{}:{}: {}", path.display(), d.line, d.message);
                }
                theme
            }
        };
        self.selected = entry.name;
        self.loaded_at = entry.path.as_deref().and_then(modified);
        Ok(theme)
    }

    /// The selected theme again, if it is a user theme whose file changed since it was loaded.
    pub fn reload_if_changed(&mut self) -> Option<Result<Theme, String>> {
        let path = self.entries.iter().find(|e| e.name == self.selected)?.path.clone()?;
        let now = modified(&path);
        if now == self.loaded_at {
            return None;
        }
        let name = self.selected.clone();
        Some(self.load(&name))
    }

    /// Creates the themes folder (with a starter theme to copy from) and returns it.
    pub fn ensure_folder(&self) -> std::io::Result<&Path> {
        std::fs::create_dir_all(&self.folder)?;
        let readme = self.folder.join("README.txt");
        if !readme.exists() {
            std::fs::write(
                &readme,
                "Put .css theme files here; they appear in View ▸ Theme and reload when saved.\n\
                 Start from a copy of Terminal.css.example (rename it to .css).\n\
                 Themes use a CSS subset: element and class selectors (h1, blockquote, li.task,\n\
                 .glyph, .hl-keyword), #write / body for the page, :root variables, and\n\
                 @media (prefers-color-scheme: dark|light). Typora themes partly work as-is.\n",
            )?;
            std::fs::write(self.folder.join("Terminal.css.example"), marginal_theme::TERMINAL)?;
        }
        Ok(&self.folder)
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_themes_list_load_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let mut themes = Themes::new(dir.path().to_path_buf(), Some("Missing".into()));
        assert_eq!(themes.selected(), "Terminal", "unknown names fall back to the default");
        assert_eq!(themes.entries.len(), BUILT_IN.len());

        let file = dir.path().join("Mine.css");
        std::fs::write(&file, "#write { color: #111111; background: #eeeeee; }").unwrap();
        assert!(themes.rescan());
        let theme = themes.load("Mine").unwrap();
        assert_eq!(themes.selected(), "Mine");
        let body = theme.style(marginal_theme::Appearance::Light, "body", &[], None);
        assert_eq!(body.background, Some(marginal_theme::Color::rgb(0xee, 0xee, 0xee)));
        assert!(themes.reload_if_changed().is_none(), "unchanged");

        // A different modification time triggers a reload.
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::write(&file, "#write { color: #222222; background: #dddddd; }").unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(later).unwrap();
        assert!(themes.reload_if_changed().is_some_and(|r| r.is_ok()));
    }
}
