//! Go to Heading: a list of the document's headings, filtered as you type.

use marginal_doc::{BlockKind, Document, SegmentKind};
use vello::kurbo::Rect;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// Where the heading's text starts in the source.
    pub offset: usize,
}

/// Every heading in the document, in order, as displayed (without `#` marks).
pub fn headings(doc: &Document) -> Vec<Heading> {
    doc.blocks()
        .iter()
        .filter_map(|b| {
            let BlockKind::Heading { level, .. } = b.kind else { return None };
            let mut text = String::new();
            let mut offset = None;
            for s in &b.segments {
                match &s.kind {
                    SegmentKind::Text { .. } => text.push_str(&doc.text()[s.range.clone()]),
                    SegmentKind::Replaced { text: t, .. } => text.push_str(t),
                    SegmentKind::Break { .. } => text.push(' '),
                    SegmentKind::Syntax { .. } => continue,
                }
                offset.get_or_insert(s.range.start);
            }
            let text = text.trim().to_owned();
            Some(Heading { level, text, offset: offset.unwrap_or(b.range.start) })
        })
        .collect()
}

/// Rows shown at once.
pub const VISIBLE: usize = 12;

#[derive(Default)]
pub struct GoTo {
    pub query: String,
    pub selected: usize,
    pub frame: Option<Rect>,
    pub rows: Vec<(Rect, usize)>,
}

impl GoTo {
    /// The headings matching the query: every word of it, in any order, ignoring case.
    pub fn items(&self, all: &[Heading]) -> Vec<Heading> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        all.iter().filter(|h| words.iter().all(|w| h.text.to_lowercase().contains(w))).cloned().collect()
    }

    pub fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_filters_headings() {
        let doc = Document::new("# On *Margins*\n\ntext\n\n## Code &amp; Tables\n\nSetext\n---\n");
        let all = headings(&doc);
        let texts: Vec<_> = all.iter().map(|h| (h.level, h.text.as_str())).collect();
        assert_eq!(texts, [(1, "On Margins"), (2, "Code & Tables"), (2, "Setext")]);
        assert_eq!(&doc.text()[all[0].offset..all[0].offset + 2], "On");
        let goto = GoTo { query: "tab co".into(), ..Default::default() };
        assert_eq!(goto.items(&all).len(), 1);
    }
}
