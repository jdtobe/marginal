//! Syntax highlighting for fenced code blocks, using tree-sitter grammars.
//!
//! The output is a list of byte ranges tagged with capture names like `keyword` or
//! `function.method`. Themes decide the colors; this crate knows nothing about them.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

/// Capture names we ask grammars for. A grammar's own names map to the longest matching prefix,
/// so `function.method.call` becomes `function.method`.
pub const CAPTURES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// A highlighted range of the code, in bytes, and the capture that applies to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub capture: &'static str,
}

/// The canonical grammar name for a code block's language word, if one is bundled.
pub fn grammar_for(lang: &str) -> Option<&'static str> {
    Some(match lang.trim().to_ascii_lowercase().as_str() {
        "bash" | "sh" | "shell" | "zsh" | "console" => "bash",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "cxx" | "hpp" => "cpp",
        "css" => "css",
        "go" | "golang" => "go",
        "html" | "htm" | "xhtml" => "html",
        "java" => "java",
        "javascript" | "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "json" | "jsonc" => "json",
        "python" | "py" | "python3" => "python",
        "ruby" | "rb" => "ruby",
        "rust" | "rs" => "rust",
        "swift" => "swift",
        "toml" => "toml",
        "typescript" | "ts" | "tsx" => "typescript",
        "yaml" | "yml" => "yaml",
        _ => return None,
    })
}

fn configuration(grammar: &str) -> Option<HighlightConfiguration> {
    let joined = |a: &str, b: &str| format!("{a}\n{b}");
    let (language, name, highlights, injections, locals) = match grammar {
        "bash" => {
            (tree_sitter_bash::LANGUAGE.into(), "bash", tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(), "", String::new())
        }
        "c" => (tree_sitter_c::LANGUAGE.into(), "c", tree_sitter_c::HIGHLIGHT_QUERY.to_owned(), "", String::new()),
        // C++ extends C; earlier patterns win, so its own come first.
        "cpp" => (
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            joined(tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY),
            "",
            String::new(),
        ),
        "css" => {
            (tree_sitter_css::LANGUAGE.into(), "css", tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(), "", String::new())
        }
        "go" => (tree_sitter_go::LANGUAGE.into(), "go", tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(), "", String::new()),
        "html" => (
            tree_sitter_html::LANGUAGE.into(),
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        "java" => (
            tree_sitter_java::LANGUAGE.into(),
            "java",
            tree_sitter_java::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            joined(tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY),
            "",
            tree_sitter_javascript::LOCALS_QUERY.to_owned(),
        ),
        "json" => (
            tree_sitter_json::LANGUAGE.into(),
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        "ruby" => (
            tree_sitter_ruby::LANGUAGE.into(),
            "ruby",
            tree_sitter_ruby::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_ruby::LOCALS_QUERY.to_owned(),
        ),
        "rust" => (
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        "swift" => (
            tree_sitter_swift::LANGUAGE.into(),
            "swift",
            tree_sitter_swift::HIGHLIGHTS_QUERY.to_owned(),
            "",
            tree_sitter_swift::LOCALS_QUERY.to_owned(),
        ),
        "toml" => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            "toml",
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        // TypeScript builds on JavaScript's queries, which come first in its own grammar config.
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "typescript",
            joined(tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
            "",
            joined(tree_sitter_javascript::LOCALS_QUERY, tree_sitter_typescript::LOCALS_QUERY),
        ),
        "yaml" => (
            tree_sitter_yaml::LANGUAGE.into(),
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
            "",
            String::new(),
        ),
        _ => return None,
    };
    let mut config = match HighlightConfiguration::new(language, name, &highlights, injections, &locals) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("highlight query for {grammar} failed: {e}");
            return None;
        }
    };
    config.configure(CAPTURES);
    Some(config)
}

/// Highlights code blocks, compiling each grammar's queries on first use and caching results.
#[derive(Default)]
pub struct Highlighter {
    configs: HashMap<&'static str, Option<HighlightConfiguration>>,
    inner: TsHighlighter,
    cache: HashMap<u64, Vec<Span>>,
}

/// Results kept before the cache starts over.
const CACHE_LIMIT: usize = 512;

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Highlight spans for `code` written in `lang` (any alias). Empty when the language isn't
    /// bundled or the code doesn't parse into anything highlightable.
    pub fn highlight(&mut self, lang: &str, code: &str) -> Vec<Span> {
        let Some(grammar) = grammar_for(lang) else { return Vec::new() };
        let key = {
            let mut h = DefaultHasher::new();
            (grammar, code).hash(&mut h);
            h.finish()
        };
        if let Some(spans) = self.cache.get(&key) {
            return spans.clone();
        }
        let Some(config) = self.configs.entry(grammar).or_insert_with(|| configuration(grammar)).as_ref() else {
            return Vec::new();
        };
        let mut spans = Vec::new();
        if let Ok(events) = self.inner.highlight(config, code.as_bytes(), None, None, |_| None) {
            let mut stack = Vec::new();
            for event in events {
                match event {
                    Ok(HighlightEvent::HighlightStart(h)) => stack.push(h.0),
                    Ok(HighlightEvent::HighlightEnd) => {
                        stack.pop();
                    }
                    Ok(HighlightEvent::Source { start, end }) => {
                        if let Some(&capture) = stack.last() {
                            match spans.last_mut() {
                                // Merge pieces of one token split by nested captures of the same kind.
                                Some(Span { range, capture: c }) if range.end == start && *c == CAPTURES[capture] => {
                                    range.end = end
                                }
                                _ => spans.push(Span { range: start..end, capture: CAPTURES[capture] }),
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(key, spans.clone());
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured<'a>(code: &'a str, spans: &[Span], capture: &str) -> Vec<&'a str> {
        spans.iter().filter(|s| s.capture == capture).map(|s| &code[s.range.clone()]).collect()
    }

    #[test]
    fn rust_keywords_and_functions() {
        let mut h = Highlighter::new();
        let code = "fn main() {\n    let x = render(&doc); // done\n}";
        let spans = h.highlight("rs", code);
        assert!(captured(code, &spans, "keyword").contains(&"fn"));
        assert!(captured(code, &spans, "keyword").contains(&"let"));
        assert!(captured(code, &spans, "function").contains(&"main"));
        assert_eq!(captured(code, &spans, "comment"), ["// done"]);
    }

    #[test]
    fn every_bundled_grammar_compiles() {
        let mut h = Highlighter::new();
        for (lang, code) in [
            ("bash", "echo \"hi\" # c"),
            ("c", "int main(void) { return 0; }"),
            ("cpp", "class A { public: int x; };"),
            ("css", "a { color: red; }"),
            ("go", "func main() { fmt.Println(\"x\") }"),
            ("html", "<p class=\"x\">hi</p>"),
            ("java", "class A { void f() {} }"),
            ("js", "const f = () => 1;"),
            ("json", "{\"a\": 1}"),
            ("python", "def f():\n    return 1"),
            ("ruby", "def f; end"),
            ("swift", "func f() -> Int { return 1 }"),
            ("toml", "a = \"b\""),
            ("ts", "let x: number = 1;"),
            ("yaml", "a: b"),
        ] {
            assert!(!h.highlight(lang, code).is_empty(), "{lang} produced no highlights");
        }
    }

    #[test]
    fn unknown_languages_are_plain() {
        assert!(Highlighter::new().highlight("klingon", "qapla'").is_empty());
        assert_eq!(grammar_for("Python3"), Some("python"));
    }
}
