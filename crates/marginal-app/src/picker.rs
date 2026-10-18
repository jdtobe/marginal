//! The code block language picker: a small popover under the language chip.

use vello::kurbo::Rect;

/// Languages offered when nothing is typed, after the recent ones.
pub const LANGUAGES: &[&str] = &[
    "bash",
    "c",
    "cpp",
    "css",
    "diff",
    "go",
    "html",
    "java",
    "javascript",
    "json",
    "markdown",
    "math",
    "mermaid",
    "python",
    "ruby",
    "rust",
    "sql",
    "swift",
    "toml",
    "typescript",
    "yaml",
];

/// Shown as a choice; picking it removes the language.
pub const PLAIN: &str = "plain";

/// How many recently used languages to remember.
pub const RECENTS: usize = 5;

/// Rows visible at once; the list scrolls with the selection.
pub const VISIBLE: usize = 8;

#[derive(Default)]
pub struct Picker {
    pub query: String,
    pub selected: usize,
    /// Where the last paint put the popover and its rows, in document coordinates.
    pub frame: Option<Rect>,
    pub rows: Vec<(Rect, String)>,
}

pub enum PickerInput {
    Up,
    Down,
    Accept,
    Cancel,
    Backspace,
    Text(String),
}

impl Picker {
    /// The choices for the current query. With no query: recents first, then the rest. With a
    /// query: matches, best first, or the query itself when nothing matches.
    pub fn items(&self, recents: &[String]) -> Vec<String> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            let rest = LANGUAGES.iter().chain([&PLAIN]).filter(|l| !recents.iter().any(|r| r == *l));
            return recents.iter().cloned().chain(rest.map(|l| l.to_string())).collect();
        }
        let mut matches: Vec<&str> = LANGUAGES.iter().chain([&PLAIN]).copied().filter(|l| l.contains(&q)).collect();
        matches.sort_by_key(|l| (l.find(&q), l.len()));
        if matches.is_empty() {
            return vec![self.query.trim().to_owned()];
        }
        matches.into_iter().map(str::to_owned).collect()
    }

    /// The first row shown, so the selection stays in view.
    pub fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE - 1)
    }
}

/// The info-string word for a choice: "plain" means no language.
pub fn language_for(choice: &str) -> &str {
    if choice == PLAIN { "" } else { choice }
}

/// Moves `lang` to the front of `recents`.
pub fn remember(recents: &mut Vec<String>, lang: &str) {
    if lang.is_empty() {
        return;
    }
    recents.retain(|r| r != lang);
    recents.insert(0, lang.to_owned());
    recents.truncate(RECENTS);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(query: &str) -> Picker {
        Picker { query: query.into(), ..Default::default() }
    }

    #[test]
    fn recents_come_first_without_a_query() {
        let items = picker("").items(&["rust".into(), "bash".into()]);
        assert_eq!(&items[..3], ["rust", "bash", "c"]);
        assert_eq!(items.iter().filter(|i| *i == "rust").count(), 1);
        assert_eq!(items.last().unwrap(), PLAIN);
    }

    #[test]
    fn queries_rank_prefix_matches_first() {
        assert_eq!(picker("script").items(&[])[..2], ["javascript", "typescript"]);
        assert_eq!(picker("ru").items(&[])[..2], ["ruby", "rust"]);
        assert_eq!(picker("zig").items(&[]), ["zig"], "unknown names are offered as typed");
    }

    #[test]
    fn remembering_dedupes_and_caps() {
        let mut r: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).into();
        remember(&mut r, "c");
        assert_eq!(r, ["c", "a", "b", "d", "e"]);
        remember(&mut r, "f");
        assert_eq!(r, ["f", "c", "a", "b", "d"]);
        remember(&mut r, "");
        assert_eq!(r.len(), RECENTS);
    }
}
