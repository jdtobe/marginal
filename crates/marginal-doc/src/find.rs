//! Find and replace over the source text: plain text by default, regex when asked.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::{Change, Document, Edit, Selection, Transaction};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FindOptions {
    /// Treat the query as a regular expression (otherwise it is literal text).
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Finder {
    re: Regex,
    options: FindOptions,
}

impl Finder {
    /// Compiles `query`. An invalid regex returns its error message, worded for the user.
    pub fn new(query: &str, options: FindOptions) -> Result<Self, String> {
        let pattern = if options.regex { query.to_owned() } else { regex::escape(query) };
        let pattern = if options.whole_word { format!(r"\b(?:{pattern})\b") } else { pattern };
        let re =
            RegexBuilder::new(&pattern).case_insensitive(!options.case_sensitive).multi_line(true).build().map_err(
                |e| match e {
                    regex::Error::Syntax(s) => s.lines().last().unwrap_or("invalid pattern").trim().to_owned(),
                    other => other.to_string(),
                },
            )?;
        Ok(Self { re, options })
    }

    /// Every non-empty match in `text`.
    pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
        self.re.find_iter(text).map(|m| m.range()).filter(|r| !r.is_empty()).collect()
    }

    /// What `range` (a match) becomes: `$1`, `${name}` and `$$` expand for regex queries;
    /// plain queries replace literally.
    pub fn replacement(&self, text: &str, range: Range<usize>, with: &str) -> String {
        if !self.options.regex {
            return with.to_owned();
        }
        // Re-run at the match start so captures (and lookaround-free anchors) line up.
        match self.re.captures_at(text, range.start).filter(|c| c.get(0).is_some_and(|m| m.range() == range)) {
            Some(caps) => {
                let mut out = String::new();
                caps.expand(with, &mut out);
                out
            }
            None => with.to_owned(),
        }
    }

    /// Replaces one match, leaving the caret after the replacement.
    pub fn replace_one(&self, doc: &Document, range: Range<usize>, with: &str) -> Change {
        let insert = self.replacement(doc.text(), range.clone(), with);
        let caret = range.start + insert.len();
        Change::edit(Transaction::new(vec![Edit { range, insert }]), Selection::caret(caret))
    }

    /// Replaces every match in one transaction (one undo step). `None` when nothing matches.
    pub fn replace_all(&self, doc: &Document, sel: Selection, with: &str) -> Option<Change> {
        let text = doc.text();
        let edits: Vec<Edit> = self
            .find_all(text)
            .into_iter()
            .map(|range| Edit { insert: self.replacement(text, range.clone(), with), range })
            .filter(|e| text[e.range.clone()] != e.insert)
            .collect();
        if edits.is_empty() {
            return None;
        }
        let tx = Transaction::new(edits);
        let selection = Selection::caret(tx.map_offset(sel.caret, crate::Bias::Before));
        Some(Change::edit(tx, selection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(regex: bool, case_sensitive: bool, whole_word: bool) -> FindOptions {
        FindOptions { regex, case_sensitive, whole_word }
    }

    #[test]
    fn plain_text_is_literal_and_case_insensitive_by_default() {
        let f = Finder::new("a.b", FindOptions::default()).unwrap();
        assert_eq!(f.find_all("A.B axb a.b"), [0..3, 8..11]);
        let f = Finder::new("Word", opts(false, true, false)).unwrap();
        assert_eq!(f.find_all("word Word"), vec![5..9]);
        let f = Finder::new("cat", opts(false, false, true)).unwrap();
        assert_eq!(f.find_all("cat concat cat."), [0..3, 11..14]);
    }

    #[test]
    fn regex_groups_expand_in_replacements() {
        let doc = Document::new("Rams 1995; revised 2009.\n");
        let f = Finder::new(r"(\d{4})", opts(true, false, false)).unwrap();
        let mut d = doc.clone();
        let change = f.replace_all(&doc, Selection::caret(0), "c. $1").unwrap();
        d.apply(&change.tx);
        assert_eq!(d.text(), "Rams c. 1995; revised c. 2009.\n");
        // A plain query replaces `$1` literally.
        let f = Finder::new("1995", FindOptions::default()).unwrap();
        assert_eq!(f.replacement(doc.text(), 5..9, "$1"), "$1");
    }

    #[test]
    fn bad_regex_reports_a_message() {
        let err = Finder::new("(unclosed", opts(true, false, false)).unwrap_err();
        assert!(err.contains("unclosed"), "{err}");
        assert!(Finder::new("(unclosed", FindOptions::default()).is_ok(), "plain text never fails");
    }

    #[test]
    fn replace_one_and_no_op_replace_all() {
        let doc = Document::new("aa");
        let f = Finder::new("a", FindOptions::default()).unwrap();
        let change = f.replace_one(&doc, 1..2, "bb");
        assert_eq!(change.selection, Selection::caret(3));
        assert!(f.replace_all(&doc, Selection::caret(0), "a").is_none(), "nothing changes");
    }
}
