//! Editing commands. Each turns a document and selection into a [`Change`]: a minimal
//! [`Transaction`] plus the selection afterward. They never touch bytes outside the element
//! being edited, and they follow the document's own conventions (line endings, `*` vs `_`,
//! list markers) rather than imposing new ones.

use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::{Bias, Block, BlockKind, ContainerKind, Document, Edit, InlineKind, SegmentKind, Selection, Transaction};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub tx: Transaction,
    pub selection: Selection,
    /// Plain typing or deleting, which undo groups together.
    pub typing: bool,
}

impl Change {
    pub(crate) fn edit(tx: Transaction, selection: Selection) -> Self {
        Self { tx, selection, typing: false }
    }

    fn typing(tx: Transaction, selection: Selection) -> Self {
        Self { tx, selection, typing: true }
    }

    /// Only moves the selection.
    pub(crate) fn select(selection: Selection) -> Self {
        Self { tx: Transaction::default(), selection, typing: false }
    }

    /// Applies `tx` and maps `selection` through it.
    fn mapped(tx: Transaction, selection: Selection) -> Self {
        let selection =
            Selection::new(tx.map_offset(selection.anchor, Bias::After), tx.map_offset(selection.caret, Bias::After));
        Self::edit(tx, selection)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineFormat {
    Strong,
    Emphasis,
    Strike,
    Code,
}

// ---- helpers ---------------------------------------------------------------------------------

/// The document's line ending, so inserted lines match the rest of the file.
pub fn newline(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn trim_newlines_end(text: &str, range: Range<usize>) -> usize {
    range.start + text[range].trim_end_matches(['\n', '\r']).len()
}

fn prev_grapheme(text: &str, at: usize) -> Option<usize> {
    GraphemeCursor::new(at, text.len(), true).prev_boundary(text, 0).ok().flatten()
}

fn next_grapheme(text: &str, at: usize) -> Option<usize> {
    GraphemeCursor::new(at, text.len(), true).next_boundary(text, 0).ok().flatten()
}

impl Block {
    /// Where editable content begins: after the block's own syntax (`## `, fences).
    pub fn content_start(&self, text: &str) -> usize {
        match self.segments.iter().find(|s| !matches!(s.kind, SegmentKind::Syntax { owner: None })) {
            Some(s) => s.range.start,
            None => trim_newlines_end(text, self.range.clone()),
        }
    }

    /// Where editable content ends, before trailing syntax and line breaks.
    pub fn content_end(&self, text: &str) -> usize {
        match self.segments.iter().rev().find(|s| !matches!(s.kind, SegmentKind::Syntax { owner: None })) {
            Some(s) => trim_newlines_end(text, s.range.clone()).max(self.content_start(text)),
            None => self.content_start(text),
        }
    }

    pub fn is_empty_content(&self, text: &str) -> bool {
        self.content_start(text) >= self.content_end(text)
    }
}

impl Document {
    /// The block whose range contains `offset` (inclusive of its end).
    pub fn block_containing(&self, offset: usize) -> Option<usize> {
        self.blocks().iter().position(|b| b.range.start <= offset && offset <= b.range.end)
    }

    /// Whether block `i` is the first leaf inside its innermost container.
    fn first_in_container(&self, i: usize) -> bool {
        let Some(c) = self.blocks()[i].containers.last() else { return false };
        i == 0 || !self.blocks()[i - 1].containers.iter().any(|p| p.id == c.id)
    }
}

/// The prefix that continues a block's containers on a new line: `> ` for quotes, spaces under
/// a list marker. Derived from the block's own first line so nesting and spacing match.
fn continuation_prefix(text: &str, block: &Block) -> String {
    let start = line_start(text, block.range.start);
    text[start..block.range.start].chars().map(|c| if c == '>' { '>' } else { ' ' }).collect()
}

/// The marker for the item after one whose marker is `marker`: `- ` stays `- `, `9. ` becomes
/// `10. `, and tasks start unchecked.
fn next_marker(marker: &str) -> String {
    let digits: String = marker.chars().take_while(char::is_ascii_digit).collect();
    let rest = &marker[digits.len()..];
    let (head, task) = match rest.find('[') {
        Some(i) => (&rest[..i], true),
        None => (rest, false),
    };
    let number = digits.parse::<u64>().map(|n| (n + 1).to_string()).unwrap_or_default();
    let mut out = format!("{number}{head}");
    if task {
        out.push_str("[ ] ");
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

// ---- typing ----------------------------------------------------------------------------------

/// Replaces the selection with `insert`, converting line breaks to the document's style.
pub fn insert_text(doc: &Document, sel: Selection, insert: &str) -> Change {
    let nl = newline(doc.text());
    let insert =
        if nl == "\r\n" { insert.replace("\r\n", "\n").replace('\n', "\r\n") } else { insert.replace("\r\n", "\n") };
    let range = sel.range();
    let caret = range.start + insert.len();
    let typing = sel.is_empty() && insert.graphemes(true).count() == 1 && !insert.contains('\n');
    let tx = Transaction::replace(range, insert);
    Change { tx, selection: Selection::caret(caret), typing }
}

pub fn delete_backward(doc: &Document, sel: Selection) -> Change {
    if let Some(change) = crate::table_guard_delete(doc, sel, true) {
        return change;
    }
    let text = doc.text();
    if !sel.is_empty() {
        let r = sel.range();
        return Change::typing(Transaction::delete(r.clone()), Selection::caret(r.start));
    }
    let at = sel.caret;
    if let Some(i) = doc.block_containing(at)
        && doc.blocks()[i].content_start(text) == at
    {
        return demote_or_join(doc, i, at);
    }
    match prev_grapheme(text, at) {
        Some(prev) => Change::typing(Transaction::delete(prev..at), Selection::caret(prev)),
        None => Change::select(sel),
    }
}

/// Backspace at the start of a block: first strip its formatting, then join it to the previous.
fn demote_or_join(doc: &Document, i: usize, at: usize) -> Change {
    let text = doc.text();
    let nl = newline(text);
    let b = &doc.blocks()[i];
    match &b.kind {
        BlockKind::Heading { setext: false, .. } => {
            let tx = Transaction::delete(b.range.start..b.content_start(text));
            return Change::edit(tx, Selection::caret(b.range.start));
        }
        BlockKind::Heading { setext: true, .. } => {
            let underline = b.content_end(text)..trim_newlines_end(text, b.range.clone());
            return Change::edit(Transaction::delete(underline), Selection::caret(at));
        }
        BlockKind::CodeBlock { .. } | BlockKind::Table | BlockKind::FrontMatter | BlockKind::Html | BlockKind::Rule => {
            return Change::select(Selection::caret(at));
        }
        BlockKind::Paragraph => {}
    }

    if doc.first_in_container(i) {
        let c = b.containers.last().unwrap();
        match &c.kind {
            ContainerKind::ListItem { .. } => {
                // Becomes a paragraph; a blank line keeps it from being read as part of the
                // previous item.
                let mut edits = vec![Edit { range: c.marker.clone(), insert: String::new() }];
                let ls = line_start(text, c.marker.start);
                let prev_line_blank = ls == 0 || text[line_start(text, ls - 1)..ls].trim().is_empty();
                let lead = &text[ls..c.marker.start];
                if !prev_line_blank {
                    edits.push(Edit { range: ls..ls, insert: format!("{}{nl}", lead.trim_end()) });
                }
                let tx = Transaction::new(edits);
                let caret = tx.map_offset(c.marker.end, Bias::Before);
                return Change::edit(tx, Selection::caret(caret));
            }
            ContainerKind::Quote => {
                return Change::mapped(unquote(text, b), Selection::caret(at));
            }
            ContainerKind::Footnote { .. } => return Change::select(Selection::caret(at)),
        }
    }

    // Join with the previous block.
    let Some(prev) = i.checked_sub(1).map(|p| &doc.blocks()[p]) else { return Change::select(Selection::caret(at)) };
    match prev.kind {
        BlockKind::Rule => Change::mapped(Transaction::delete(prev.range.clone()), Selection::caret(at)),
        BlockKind::CodeBlock { .. } | BlockKind::Table | BlockKind::FrontMatter | BlockKind::Html => {
            Change::select(Selection::caret(prev.content_end(text)))
        }
        _ => {
            let end = prev.content_end(text);
            Change::edit(Transaction::delete(end..at), Selection::caret(end))
        }
    }
}

/// Removes one level of `>` from every line of a block.
fn unquote(text: &str, b: &Block) -> Transaction {
    let mut edits = Vec::new();
    let mut ls = line_start(text, b.range.start);
    let end = b.range.end.max(ls + 1);
    while ls < end && ls <= text.len() {
        let line = &text[ls..];
        let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
        if line[lead..].starts_with('>') {
            let gt = ls + lead;
            let stop = if text[gt + 1..].starts_with(' ') { gt + 2 } else { gt + 1 };
            edits.push(Edit { range: gt..stop, insert: String::new() });
        }
        match text[ls..].find('\n') {
            Some(n) => ls += n + 1,
            None => break,
        }
    }
    Transaction::new(edits)
}

pub fn delete_forward(doc: &Document, sel: Selection) -> Change {
    if let Some(change) = crate::table_guard_delete(doc, sel, false) {
        return change;
    }
    let text = doc.text();
    if !sel.is_empty() {
        let r = sel.range();
        return Change::typing(Transaction::delete(r.clone()), Selection::caret(r.start));
    }
    let at = sel.caret;
    if let Some(i) = doc.block_containing(at) {
        let b = &doc.blocks()[i];
        if b.content_end(text) == at
            && let Some(next) = doc.blocks().get(i + 1)
        {
            let joinable = matches!(b.kind, BlockKind::Paragraph | BlockKind::Heading { .. })
                && matches!(next.kind, BlockKind::Paragraph | BlockKind::Heading { .. });
            if !joinable {
                return Change::select(sel);
            }
            return Change::edit(Transaction::delete(at..next.content_start(text)), Selection::caret(at));
        }
    }
    match next_grapheme(text, at) {
        Some(next) if next > at => Change::typing(Transaction::delete(at..next), Selection::caret(at)),
        _ => Change::select(sel),
    }
}

/// Enter: continues lists, splits paragraphs, keeps code indentation, exits empty containers.
pub fn insert_newline(doc: &Document, sel: Selection) -> Change {
    let text = doc.text();
    let nl = newline(text);
    let range = sel.range();
    let at = range.start;
    let insert_at = |s: String| {
        let caret = at + s.len();
        Change::edit(Transaction::replace(range.clone(), s), Selection::caret(caret))
    };
    let Some(i) = doc.block_containing(at) else { return insert_at(nl.to_owned()) };
    let b = &doc.blocks()[i];
    let prefix = continuation_prefix(text, b);
    let blank_prefix = prefix.trim_end().to_owned();

    match &b.kind {
        BlockKind::CodeBlock { .. } | BlockKind::Html | BlockKind::FrontMatter | BlockKind::Table => {
            let ls = line_start(text, at);
            let indent: String = text[ls..at].chars().take_while(|c| matches!(c, ' ' | '\t' | '>')).collect();
            return insert_at(format!("{nl}{indent}"));
        }
        BlockKind::Rule => return insert_at(nl.to_owned()),
        BlockKind::Heading { .. }
            if at == b.content_start(text) && !b.is_empty_content(text) && b.containers.is_empty() =>
        {
            // New paragraph above; the caret stays in the heading.
            let tx = Transaction::insert(b.range.start, format!("{nl}{nl}"));
            return Change::edit(tx, Selection::caret(at + 2 * nl.len()));
        }
        BlockKind::Heading { .. } | BlockKind::Paragraph => {}
    }

    if let Some(c) = b.containers.last() {
        match &c.kind {
            ContainerKind::ListItem { .. } if doc.first_in_container(i) || b.kind == BlockKind::Paragraph => {
                if b.is_empty_content(text) {
                    // Enter on an empty item leaves the list.
                    let tx = Transaction::delete(c.marker.clone());
                    return Change::edit(tx, Selection::caret(c.marker.start));
                }
                let ls = line_start(text, c.marker.start);
                let lead = &text[ls..c.marker.start];
                let marker = next_marker(&text[c.marker.clone()]);
                return insert_at(format!("{nl}{lead}{marker}"));
            }
            ContainerKind::Quote if b.is_empty_content(text) => {
                return Change::mapped(unquote(text, b), Selection::caret(at));
            }
            _ => {}
        }
    }
    insert_at(format!("{nl}{blank_prefix}{nl}{prefix}"))
}

/// Shift+Enter: a hard line break inside the paragraph, written as a trailing backslash so it
/// survives editors that strip trailing whitespace.
pub fn insert_line_break(doc: &Document, sel: Selection) -> Change {
    let text = doc.text();
    let nl = newline(text);
    let range = sel.range();
    let prefix =
        doc.block_containing(range.start).map(|i| continuation_prefix(text, &doc.blocks()[i])).unwrap_or_default();
    let insert = format!("\\{nl}{prefix}");
    let caret = range.start + insert.len();
    Change::edit(Transaction::replace(range, insert), Selection::caret(caret))
}

/// Pastes `insert`. A URL pasted over selected text links that text instead of replacing it.
pub fn paste(doc: &Document, sel: Selection, insert: &str) -> Change {
    let url = insert.trim();
    let selected = &doc.text()[sel.range()];
    let is_url =
        ["http://", "https://", "mailto:"].iter().any(|p| url.starts_with(p)) && !url.contains(char::is_whitespace);
    if !sel.is_empty() && is_url && !selected.contains('\n') {
        let link = format!("[{selected}]({url})");
        let caret = sel.range().start + link.len();
        return Change::edit(Transaction::replace(sel.range(), link), Selection::caret(caret));
    }
    Change { typing: false, ..insert_text(doc, sel, insert) }
}

// ---- formatting ------------------------------------------------------------------------------

/// ⌘K: links the selection (or the word at the caret) and puts the caret in the destination;
/// inside an existing link, removes the link and keeps its text.
pub fn toggle_link(doc: &Document, sel: Selection) -> Change {
    let text = doc.text();
    let range = sel.range();
    let Some(bi) = doc.block_containing(range.start) else { return Change::select(sel) };
    let b = &doc.blocks()[bi];
    let existing = b
        .inlines
        .iter()
        .enumerate()
        .filter(|(_, i)| {
            matches!(i.kind, InlineKind::Link { .. }) && i.range.start <= range.start && range.end <= i.range.end
        })
        .min_by_key(|(_, i)| i.range.len());
    if let Some((idx, _)) = existing {
        let edits: Vec<Edit> = b
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::Syntax { owner: Some(idx) })
            .map(|s| Edit { range: s.range.clone(), insert: String::new() })
            .collect();
        return Change::mapped(Transaction::new(edits), sel);
    }
    let target = if range.is_empty() { word_at(text, b, range.start).unwrap_or(range.clone()) } else { range.clone() };
    let tx = Transaction::new(vec![
        Edit { range: target.start..target.start, insert: "[".into() },
        Edit { range: target.end..target.end, insert: "]()".into() },
    ]);
    // An empty link puts the caret in the text; otherwise in the destination.
    let caret = if target.is_empty() { target.start + 1 } else { target.end + 3 };
    Change::edit(tx, Selection::caret(caret))
}

fn inline_kind_matches(kind: &InlineKind, format: InlineFormat) -> bool {
    matches!(
        (kind, format),
        (InlineKind::Strong, InlineFormat::Strong)
            | (InlineKind::Emphasis, InlineFormat::Emphasis)
            | (InlineKind::Strike, InlineFormat::Strike)
            | (InlineKind::Code, InlineFormat::Code)
    )
}

/// The delimiter the document already favors for this format.
fn delimiter(doc: &Document, format: InlineFormat) -> &'static str {
    let text = doc.text();
    let (star, underscore) =
        doc.blocks().iter().flat_map(|b| &b.inlines).filter(|i| inline_kind_matches(&i.kind, format)).fold(
            (0, 0),
            |(s, u), i| match text.as_bytes()[i.range.start] {
                b'_' => (s, u + 1),
                _ => (s + 1, u),
            },
        );
    match format {
        InlineFormat::Strong if underscore > star => "__",
        InlineFormat::Strong => "**",
        InlineFormat::Emphasis if underscore > star => "_",
        InlineFormat::Emphasis => "*",
        InlineFormat::Strike => "~~",
        InlineFormat::Code => "`",
    }
}

/// ⌘B / ⌘I / …: removes the format if the selection is already inside it, otherwise wraps the
/// selection (or the word at the caret) in it.
pub fn toggle_inline(doc: &Document, sel: Selection, format: InlineFormat) -> Change {
    let text = doc.text();
    let range = sel.range();
    let Some(bi) = doc.block_containing(range.start) else { return Change::select(sel) };
    let b = &doc.blocks()[bi];

    let existing = b
        .inlines
        .iter()
        .enumerate()
        .filter(|(_, i)| {
            inline_kind_matches(&i.kind, format) && i.range.start <= range.start && range.end <= i.range.end
        })
        .min_by_key(|(_, i)| i.range.len());
    if let Some((idx, _)) = existing {
        let edits: Vec<Edit> = b
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::Syntax { owner: Some(idx) })
            .map(|s| Edit { range: s.range.clone(), insert: String::new() })
            .collect();
        return Change::mapped(Transaction::new(edits), sel);
    }

    let mut target = range.clone();
    if target.is_empty() {
        if let Some(word) = word_at(text, b, range.start) {
            target = word;
        }
    } else {
        // Delimiters must hug the text, so leave surrounding spaces outside.
        let slice = &text[target.clone()];
        target.start += slice.len() - slice.trim_start().len();
        target.end -= slice.len() - slice.trim_end().len();
        if target.start > target.end {
            target = range.clone();
        }
    }
    let d = delimiter(doc, format);
    let tx = Transaction::new(vec![
        Edit { range: target.start..target.start, insert: d.to_owned() },
        Edit { range: target.end..target.end, insert: d.to_owned() },
    ]);
    let selection = if sel.is_empty() {
        Selection::caret(sel.caret + d.len())
    } else {
        Selection::new(target.start + d.len(), target.end + d.len())
    };
    Change::edit(tx, selection)
}

fn word_at(text: &str, b: &Block, at: usize) -> Option<Range<usize>> {
    let start = b.content_start(text);
    let end = b.content_end(text).max(start);
    text[start..end]
        .split_word_bound_indices()
        .map(|(i, w)| (start + i..start + i + w.len(), w))
        .find(|(r, w)| r.start <= at && at <= r.end && w.chars().any(char::is_alphanumeric))
        .map(|(r, _)| r)
}

/// Heading level of the block at the caret: 0 for a paragraph, `None` for other blocks.
pub fn heading_level(doc: &Document, sel: Selection) -> Option<u8> {
    let b = &doc.blocks()[doc.block_containing(sel.caret)?];
    match b.kind {
        BlockKind::Heading { level, .. } => Some(level),
        BlockKind::Paragraph => Some(0),
        _ => None,
    }
}

/// ⌘1–⌘6, ⌘0: makes the block at the caret a heading of `level` (0 = paragraph).
pub fn set_heading(doc: &Document, sel: Selection, level: u8) -> Change {
    let text = doc.text();
    let Some(i) = doc.block_containing(sel.caret) else { return Change::select(sel) };
    let b = &doc.blocks()[i];
    let marks = if level == 0 { String::new() } else { format!("{} ", "#".repeat(level.min(6) as usize)) };
    let mut edits = Vec::new();
    match b.kind {
        BlockKind::Heading { setext: false, .. } => {
            edits.push(Edit { range: b.range.start..b.content_start(text), insert: marks });
        }
        BlockKind::Heading { setext: true, .. } => {
            edits.push(Edit {
                range: b.content_end(text)..trim_newlines_end(text, b.range.clone()),
                insert: String::new(),
            });
            edits.push(Edit { range: b.range.start..b.range.start, insert: marks });
        }
        BlockKind::Paragraph if level > 0 => edits.push(Edit { range: b.range.start..b.range.start, insert: marks }),
        _ => return Change::select(sel),
    }
    Change::mapped(Transaction::new(edits), sel)
}

/// ⌘] / ⌘[: one heading level more or less prominent.
pub fn shift_heading(doc: &Document, sel: Selection, more_prominent: bool) -> Change {
    let Some(level) = heading_level(doc, sel) else { return Change::select(sel) };
    let next = match (level, more_prominent) {
        (0, true) => 6,
        (1, true) => 1,
        (l, true) => l - 1,
        (0, false) => 0,
        (6, false) => 0,
        (l, false) => l + 1,
    };
    set_heading(doc, sel, next)
}

/// The source range of a fenced code block's language: the first word after the opening fence.
/// Empty, just after the fence, when the block has no language.
pub fn code_language_range(doc: &Document, block: usize) -> Option<Range<usize>> {
    let b = doc.blocks().get(block)?;
    let BlockKind::CodeBlock { fenced: true, .. } = b.kind else { return None };
    let text = doc.text();
    let line_end = text[b.range.start..].find('\n').map_or(text.len(), |i| b.range.start + i);
    let line = &text[b.range.start..line_end];
    let indent = line.len() - line.trim_start_matches(' ').len();
    let fence = line[indent..].chars().next().filter(|c| *c == '`' || *c == '~')?;
    let after_fence = indent + line[indent..].len() - line[indent..].trim_start_matches(fence).len();
    let word_start = after_fence + line[after_fence..].len() - line[after_fence..].trim_start().len();
    let word_end = line[word_start..].find(char::is_whitespace).map_or(line.len(), |i| word_start + i);
    let start = b.range.start + if word_start == word_end { after_fence } else { word_start };
    Some(start..b.range.start + word_end.max(after_fence))
}

/// Sets the language of the fenced code block at the caret, keeping the rest of the info string.
/// An empty `lang` removes it.
pub fn set_code_language(doc: &Document, sel: Selection, lang: &str) -> Change {
    let Some(range) = doc.block_containing(sel.caret).and_then(|i| code_language_range(doc, i)) else {
        return Change::select(sel);
    };
    if doc.text()[range.clone()] == *lang {
        return Change::select(sel);
    }
    Change::mapped(Transaction::new(vec![Edit { range, insert: lang.to_owned() }]), sel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies a command to `src` with the caret (and optional anchor) marked by `|`, returning
    /// the result with the new caret marked the same way.
    fn run(src: &str, f: impl Fn(&Document, Selection) -> Change) -> String {
        let marks: Vec<usize> = src.match_indices('|').map(|(i, _)| i).collect();
        let clean = src.replace('|', "");
        let sel = match marks.as_slice() {
            [c] => Selection::caret(*c),
            [a, c] => Selection::new(*a, *c - 1),
            _ => panic!("mark the caret with |"),
        };
        let mut doc = Document::new(clean);
        let change = f(&doc, sel);
        doc.apply(&change.tx);
        let mut out = doc.text().to_owned();
        let s = change.selection;
        if s.is_empty() {
            out.insert(s.caret, '|');
        } else {
            let r = s.range();
            out.insert(r.end, '|');
            out.insert(r.start, '|');
        }
        out
    }

    #[test]
    fn code_language_rewrites_only_the_first_word() {
        let set = |lang: &'static str| move |d: &Document, s| set_code_language(d, s, lang);
        assert_eq!(run("```rust\nfn |a\n```\n", set("python")), "```python\nfn |a\n```\n");
        assert_eq!(run("```\nfn |a\n```\n", set("rust")), "```rust\nfn |a\n```\n");
        assert_eq!(run("~~~~ rs title=\"x\"\n|a\n~~~~\n", set("rust")), "~~~~ rust title=\"x\"\n|a\n~~~~\n");
        assert_eq!(run("```rust\n|a\n```\n", set("")), "```\n|a\n```\n");
        assert_eq!(run("- x\n\n  ```sh\n  |ls\n  ```\n", set("bash")), "- x\n\n  ```bash\n  |ls\n  ```\n");
        assert_eq!(run("para|\n", set("rust")), "para|\n", "not a code block");
    }

    #[test]
    fn backspace_demotes_then_joins() {
        assert_eq!(run("a\n\n## |Title\n", delete_backward), "a\n\n|Title\n");
        assert_eq!(run("a\n\n|Title\n", delete_backward), "a|Title\n");
        assert_eq!(run("- a\n- |b\n", delete_backward), "- a\n\n|b\n");
        assert_eq!(run("> |quoted\n> more\n", delete_backward), "|quoted\nmore\n");
        assert_eq!(run("ab|c", delete_backward), "a|c");
        assert_eq!(run("👨‍👩‍👧|", delete_backward), "|", "whole grapheme cluster");
    }

    #[test]
    fn setext_heading_demotes() {
        assert_eq!(run("|Title\n=====\n", delete_backward), "|Title\n");
    }

    #[test]
    fn enter_continues_and_exits_lists() {
        assert_eq!(run("- a|\n", insert_newline), "- a\n- |\n");
        assert_eq!(run("9. a|\n", insert_newline), "9. a\n10. |\n");
        assert_eq!(run("- [x] done|\n", insert_newline), "- [x] done\n- [ ] |\n");
        assert_eq!(run("> - a|\n", insert_newline), "> - a\n> - |\n");
        assert_eq!(run("- a\n- |\n", insert_newline), "- a\n|\n");
    }

    #[test]
    fn enter_splits_paragraphs_in_containers() {
        assert_eq!(run("fo|o\n", insert_newline), "fo\n\n|o\n");
        assert_eq!(run("> fo|o\n", insert_newline), "> fo\n>\n> |o\n");
        assert_eq!(run("a\r\n\r\nfo|o\r\n", insert_newline), "a\r\n\r\nfo\r\n\r\n|o\r\n", "keeps CRLF");
        assert_eq!(run("```\n    x|\n```\n", insert_newline), "```\n    x\n    |\n```\n");
        assert_eq!(run("# |Title\n", insert_newline), "\n\n# |Title\n");
    }

    #[test]
    fn toggle_bold_wraps_and_unwraps() {
        let bold = |d: &Document, s| toggle_inline(d, s, InlineFormat::Strong);
        assert_eq!(run("say |hello| there", bold), "say **|hello|** there");
        assert_eq!(run("say **hel|lo** there", bold), "say hel|lo there");
        assert_eq!(run("say hel|lo", bold), "say **hel|lo**", "no selection: the word at the caret");
        assert_eq!(run("__x__ and |y|", bold), "__x__ and __|y|__", "matches the document's style");
        assert_eq!(run("a| b |c", bold), "a **|b|** c", "spaces stay outside the markers");
    }

    #[test]
    fn heading_levels() {
        assert_eq!(run("Ti|tle\n", |d, s| set_heading(d, s, 2)), "## Ti|tle\n");
        assert_eq!(run("## Ti|tle\n", |d, s| set_heading(d, s, 0)), "Ti|tle\n");
        assert_eq!(run("### Ti|tle\n", |d, s| shift_heading(d, s, true)), "## Ti|tle\n");
        assert_eq!(run("Ti|tle\n---\n", |d, s| set_heading(d, s, 3)), "### Ti|tle\n");
    }

    #[test]
    fn links() {
        assert_eq!(run("see |docs| here", toggle_link), "see [docs](|) here");
        assert_eq!(run("see do|cs", toggle_link), "see [docs](|)");
        assert_eq!(run("see [do|cs](https://x.y) here", toggle_link), "see do|cs here");
        assert_eq!(run("see |docs|", |d, s| paste(d, s, "https://x.y")), "see [docs](https://x.y)|");
        assert_eq!(run("see |", |d, s| paste(d, s, "https://x.y")), "see https://x.y|");
    }

    #[test]
    fn line_breaks_keep_container_prefixes() {
        assert_eq!(run("> a|b\n", insert_line_break), "> a\\\n> |b\n");
        assert_eq!(run("a|b", insert_line_break), "a\\\n|b");
    }

    #[test]
    fn insert_matches_line_endings() {
        assert_eq!(run("a\r\nb|", |d, s| insert_text(d, s, "x\ny")), "a\r\nbx\r\ny|");
    }
}
