//! Marginal's document model.
//!
//! The source text is the single source of truth. Everything else (blocks, segments, inline
//! elements) is derived from it and rebuilt after each [`Transaction`]. Nothing in this crate
//! ever rewrites bytes it wasn't explicitly asked to change.

mod commands;
mod edit;
mod find;
mod parse;
mod table;

pub use commands::{
    Change, InlineFormat, code_language_range, delete_backward, delete_forward, heading_level, insert_line_break,
    insert_newline, insert_text, newline, paste, set_code_language, set_heading, shift_heading, toggle_inline,
    toggle_link,
};
pub use edit::{Bias, Edit, Selection, Transaction};
pub use find::{FindOptions, Finder};
pub use parse::{
    Align, Block, BlockKind, Container, ContainerKind, Inline, InlineKind, InlineStyle, Segment, SegmentKind,
    TableShape, parse_blocks, parser_options,
};
pub use table::{
    Cell, cell_at, cell_content, escape_cell_text, realign_table, table_delete_column, table_delete_row, table_enter,
    table_guard_delete, table_insert_column, table_insert_row, table_is_aligned, table_move_column, table_move_row,
    table_paste, table_set_align, table_tab,
};

#[derive(Clone, Debug)]
pub struct Document {
    text: String,
    blocks: Vec<Block>,
}

impl Document {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let blocks = parse_blocks(&text);
        Self { text, blocks }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Index of the block containing `offset`, or the nearest one before it.
    pub fn block_at(&self, offset: usize) -> Option<usize> {
        match self.blocks.binary_search_by(|b| {
            if b.range.end < offset {
                std::cmp::Ordering::Less
            } else if b.range.start > offset {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => Some(i - 1),
        }
    }

    /// Applies `tx` and returns its inverse.
    pub fn apply(&mut self, tx: &Transaction) -> Transaction {
        let inverse = tx.apply(&mut self.text);
        // Full reparse: pulldown-cmark handles ~100 KB in about a millisecond. Incremental
        // reparsing of the affected top-level blocks can slot in here without API changes.
        self.blocks = parse_blocks(&self.text);
        inverse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariants every parse must satisfy, for any input.
    fn check_invariants(src: &str) {
        let doc = Document::new(src);
        let mut prev_end = 0;
        for b in doc.blocks() {
            assert!(b.range.start >= prev_end, "blocks overlap or are out of order in {src:?}: {:#?}", doc.blocks());
            assert!(b.range.end <= src.len());
            prev_end = b.range.end;
            let mut pos = b.range.start;
            for s in &b.segments {
                assert_eq!(s.range.start, pos, "segments must tile the block in {src:?}: {b:#?}");
                assert!(src.is_char_boundary(s.range.start) && src.is_char_boundary(s.range.end));
                if let SegmentKind::Text { .. } = s.kind {
                    assert!(!s.range.is_empty() || b.range.is_empty());
                }
                pos = s.range.end;
            }
            assert_eq!(pos, b.range.end, "segments must end at block end in {src:?}");
        }
    }

    fn visible(doc: &Document, i: usize) -> String {
        let b = &doc.blocks()[i];
        b.segments
            .iter()
            .map(|s| match &s.kind {
                SegmentKind::Text { .. } => doc.text()[s.range.clone()].to_owned(),
                SegmentKind::Replaced { text, .. } => text.clone(),
                SegmentKind::Break { .. } => " ".into(),
                SegmentKind::Syntax { .. } => String::new(),
            })
            .collect()
    }

    #[test]
    fn heading_and_emphasis() {
        let doc = Document::new("## Hello **bold** world\n");
        assert_eq!(doc.blocks().len(), 1);
        let b = &doc.blocks()[0];
        assert_eq!(b.kind, BlockKind::Heading { level: 2, setext: false });
        assert_eq!(visible(&doc, 0), "Hello bold world");
        let strong = b.inlines.iter().position(|i| i.kind == InlineKind::Strong).unwrap();
        let owned: Vec<_> = b
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::Syntax { owner: Some(strong) })
            .map(|s| &doc.text()[s.range.clone()])
            .collect();
        assert_eq!(owned, ["**", "**"]);
    }

    #[test]
    fn quote_list_and_tasks() {
        let src = "> quoted\n> line two\n\n- [x] done\n- [ ] todo\n\n1. first\n";
        let doc = Document::new(src);
        let kinds: Vec<_> = doc.blocks().iter().map(|b| b.containers.last().map(|c| c.kind.clone())).collect();
        assert_eq!(kinds[0], Some(ContainerKind::Quote));
        assert_eq!(visible(&doc, 0), "quoted line two");
        assert_eq!(kinds[1], Some(ContainerKind::ListItem { ordered: None, task: Some(true) }));
        assert_eq!(visible(&doc, 1), "done");
        assert_eq!(kinds[2], Some(ContainerKind::ListItem { ordered: None, task: Some(false) }));
        assert_eq!(kinds[3], Some(ContainerKind::ListItem { ordered: Some(1), task: None }));
        let marker = &doc.blocks()[1].containers[0].marker;
        assert_eq!(&src[marker.clone()], "- [x] ");
    }

    #[test]
    fn code_entities_and_links() {
        let doc = Document::new("Use `x` &amp; [site](https://a.b) \\*not em\\*\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(visible(&doc, 0), "Use x & site *not em*");
        assert_eq!(doc.blocks()[1].kind, BlockKind::CodeBlock { info: "rust".into(), fenced: true });
        assert_eq!(visible(&doc, 1), "fn main() {}\n");
    }

    #[test]
    fn edits_keep_untouched_bytes() {
        let src = "# Title\r\n\r\nSome *odd*   spacing\t\r\n";
        let mut doc = Document::new(src);
        let inv = doc.apply(&Transaction::insert(2, "My "));
        assert_eq!(doc.text(), "# My Title\r\n\r\nSome *odd*   spacing\t\r\n");
        doc.apply(&inv);
        assert_eq!(doc.text(), src);
    }

    #[test]
    fn empty_containers_get_a_block() {
        let src = "- a\n- \n\n>\n";
        let doc = Document::new(src);
        let empties: Vec<_> = doc.blocks().iter().filter(|b| b.range.is_empty()).collect();
        assert_eq!(empties.len(), 2, "{:#?}", doc.blocks());
        assert_eq!(empties[0].range.start, 6, "after the `- ` marker");
        assert!(matches!(empties[1].containers[0].kind, ContainerKind::Quote));
    }

    #[test]
    fn fixtures_hold_invariants() {
        for src in [
            "",
            "\n\n",
            "plain",
            "# h\nSetext\n===\n",
            "* a\n\n  b\n* c\n",
            "> - [ ] task in quote\n>   more\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n",
            "---\ntitle: x\n---\n\nbody\n",
            "Text[^1].\n\n[^1]: Note.\n",
            "$x^2$ and $$y$$\n",
            "<div>\nhtml\n</div>\n",
            "line  \nhard break\\\nagain\n",
            "***\n",
            "    indented code\n",
            "![alt *text*](img.png)\n",
            "~~gone~~ ^sup^ ~sub~\n",
        ] {
            check_invariants(src);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2000))]
        #[test]
        fn random_markdown_holds_invariants(src in r"([#>*_`\-+\[\]()!$~|\\ \n\t:xé1.]|\[\^1\]|```|- \[ \] ){0,60}") {
            check_invariants(&src);
        }
    }
}
