//! GFM tables: which cell an offset is in, and editing commands that compile to small line
//! patches in the table's own style. A table whose pipes line up stays lined up; a compact
//! table stays compact and only the touched cells change.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::{Align, BlockKind, Change, Document, Edit, Selection, Transaction};

/// Where the caret lands in a cell after a table command.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// After the cell's text.
    End,
    /// This many bytes into the cell's text.
    Within(usize),
}

/// A cell by row (0 is the header, then body rows; the delimiter row isn't counted) and column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

/// The table block containing `offset`, and the cell there if the offset is inside one.
pub fn cell_at(doc: &Document, offset: usize) -> Option<(usize, Cell)> {
    let i = doc.block_containing(offset)?;
    let shape = doc.blocks()[i].table.as_ref()?;
    for (row, cells) in shape.rows.iter().enumerate() {
        for (col, range) in cells.iter().enumerate() {
            if range.start <= offset && offset <= range.end {
                return Some((i, Cell { row, col }));
            }
        }
    }
    None
}

/// The editable part of a cell: its text without the padding around it. An empty cell's caret
/// position sits one space in, where typing reads naturally in the source.
pub fn cell_content(text: &str, cell: Range<usize>) -> Range<usize> {
    let s = &text[cell.clone()];
    let lead = s.len() - s.trim_start().len();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        let at = cell.start + s.len().min(1);
        return at..at;
    }
    cell.start + lead..cell.start + lead + trimmed.len()
}

/// Text made safe for a cell: pipes escaped, line breaks as `<br>`.
pub fn escape_cell_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut escaped = false;
    for c in s.chars() {
        match c {
            '|' if !escaped => out.push_str("\\|"),
            '\r' => {}
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
        escaped = c == '\\' && !escaped;
    }
    out
}

/// Whether the table at `offset` has its pipes lined up in every row.
pub fn table_is_aligned(doc: &Document, offset: usize) -> bool {
    Grid::at(doc, offset).is_some_and(|g| g.aligned)
}

/// Re-pads an aligned table after its content changed, so the pipes line up again.
pub fn realign_table(doc: &Document, offset: usize) -> Option<Transaction> {
    let old = Grid::at(doc, offset)?;
    let mut new = old.clone();
    new.aligned = true;
    new.realign();
    let tx = new.diff(&old, doc.text());
    (!tx.is_empty()).then_some(tx)
}

// ---- commands --------------------------------------------------------------------------------

/// Tab / ⇧Tab: selects the next or previous cell's content. Tab in the last cell adds a row.
pub fn table_tab(doc: &Document, sel: Selection, backward: bool) -> Option<Change> {
    let (block, cell) = cell_at(doc, sel.caret)?;
    let shape = doc.blocks()[block].table.as_ref()?;
    let cols = shape.aligns.len();
    let last_row = shape.rows.len() - 1;
    let target = match (backward, cell) {
        (true, Cell { row: 0, col: 0 }) => return Some(Change::select(sel)),
        (true, Cell { row, col: 0 }) => Cell { row: row - 1, col: cols - 1 },
        (true, Cell { row, col }) => Cell { row, col: col - 1 },
        (false, Cell { row, col }) if col + 1 < cols => Cell { row, col: col + 1 },
        (false, Cell { row, .. }) if row < last_row => Cell { row: row + 1, col: 0 },
        (false, _) => return table_insert_row_at(doc, sel, last_row + 1, 0),
    };
    Some(Change::select(select_cell(doc, block, target)))
}

/// Return: the caret moves to the cell below, adding a row at the end of the table.
pub fn table_enter(doc: &Document, sel: Selection) -> Option<Change> {
    let (block, cell) = cell_at(doc, sel.caret)?;
    let rows = doc.blocks()[block].table.as_ref()?.rows.len();
    if cell.row + 1 < rows {
        let range = cell_range(doc, block, Cell { row: cell.row + 1, col: cell.col })?;
        return Some(Change::select(Selection::caret(cell_content(doc.text(), range).end)));
    }
    table_insert_row_at(doc, sel, rows, cell.col)
}

/// Adds a body row above or below the caret's row (below the header when the caret is in it).
pub fn table_insert_row(doc: &Document, sel: Selection, below: bool) -> Option<Change> {
    let (_, cell) = cell_at(doc, sel.caret)?;
    let at = if below || cell.row == 0 { cell.row + 1 } else { cell.row };
    table_insert_row_at(doc, sel, at, cell.col)
}

fn table_insert_row_at(doc: &Document, sel: Selection, row: usize, col: usize) -> Option<Change> {
    let old = Grid::at(doc, sel.caret)?;
    let mut new = old.clone();
    let line = Grid::line_of(row).min(new.lines.len());
    let mut fresh = new.lines[line - 1].clone();
    fresh.origin = None;
    fresh.slots = (0..new.cols()).map(|_| new.fresh("")).collect();
    new.lines.insert(line, fresh);
    Some(new.commit(&old, doc.text(), Some((line, col, Place::End))))
}

/// Removes the caret's row. The header row stays; deleting it would make the table invalid.
pub fn table_delete_row(doc: &Document, sel: Selection) -> Option<Change> {
    let (_, cell) = cell_at(doc, sel.caret)?;
    if cell.row == 0 {
        return Some(Change::select(sel));
    }
    let old = Grid::at(doc, sel.caret)?;
    let mut new = old.clone();
    let line = Grid::line_of(cell.row);
    new.lines.remove(line);
    let target = if line < new.lines.len() { line } else { line - 1 };
    let target = if target == 1 { 0 } else { target };
    Some(new.commit(&old, doc.text(), Some((target, cell.col, Place::End))))
}

/// ⌥⇧↑ / ⌥⇧↓ in a table: moves the caret's row, keeping the caret in it. Moving a body row
/// above the header makes it the new header.
pub fn table_move_row(doc: &Document, sel: Selection, up: bool) -> Option<Change> {
    let (block, cell) = cell_at(doc, sel.caret)?;
    let rows = doc.blocks()[block].table.as_ref()?.rows.len();
    let other = if up { cell.row.checked_sub(1)? } else { cell.row + 1 };
    if other >= rows {
        return Some(Change::select(sel));
    }
    let old = Grid::at(doc, sel.caret)?;
    let mut new = old.clone();
    let (a, b) = (Grid::line_of(cell.row), Grid::line_of(other));
    let (slots_a, slots_b) = (new.lines[a].slots.clone(), new.lines[b].slots.clone());
    new.lines[a].slots = slots_b;
    new.lines[b].slots = slots_a;
    let within = sel.caret.saturating_sub(cell_content(doc.text(), cell_range(doc, block, cell)?).start);
    Some(new.commit(&old, doc.text(), Some((b, cell.col, Place::Within(within)))))
}

/// Adds a column left or right of the caret's column.
pub fn table_insert_column(doc: &Document, sel: Selection, right: bool) -> Option<Change> {
    let (_, cell) = cell_at(doc, sel.caret)?;
    let col = if right { cell.col + 1 } else { cell.col };
    let old = Grid::at(doc, sel.caret)?;
    let mut new = old.clone();
    new.normalize();
    new.aligns.insert(col, Align::None);
    for (i, line) in new.lines.iter_mut().enumerate() {
        let slot = if i == 1 { old.fresh_delim(Align::None, 3) } else { old.fresh("") };
        line.slots.insert(col, slot);
    }
    Some(new.commit(&old, doc.text(), Some((Grid::line_of(cell.row), col, Place::End))))
}

/// Removes the caret's column, unless it is the only one.
pub fn table_delete_column(doc: &Document, sel: Selection) -> Option<Change> {
    let (_, cell) = cell_at(doc, sel.caret)?;
    let old = Grid::at(doc, sel.caret)?;
    if old.cols() < 2 {
        return Some(Change::select(sel));
    }
    let mut new = old.clone();
    new.normalize();
    new.aligns.remove(cell.col);
    for line in &mut new.lines {
        line.slots.remove(cell.col);
    }
    let col = cell.col.min(new.cols() - 1);
    Some(new.commit(&old, doc.text(), Some((Grid::line_of(cell.row), col, Place::End))))
}

/// Moves the caret's column one place left or right.
pub fn table_move_column(doc: &Document, sel: Selection, right: bool) -> Option<Change> {
    let (_, cell) = cell_at(doc, sel.caret)?;
    let old = Grid::at(doc, sel.caret)?;
    let other = if right { cell.col + 1 } else { cell.col.checked_sub(1)? };
    if other >= old.cols() {
        return Some(Change::select(sel));
    }
    let mut new = old.clone();
    new.normalize();
    new.aligns.swap(cell.col, other);
    for line in &mut new.lines {
        line.slots.swap(cell.col, other);
    }
    Some(new.commit(&old, doc.text(), Some((Grid::line_of(cell.row), other, Place::End))))
}

/// Sets how a column's cells align, by rewriting its delimiter (`:--`, `:-:`, `--:`).
pub fn table_set_align(doc: &Document, sel: Selection, col: usize, align: Align) -> Option<Change> {
    let old = Grid::at(doc, sel.caret)?;
    if col >= old.cols() {
        return None;
    }
    let mut new = old.clone();
    new.normalize();
    new.aligns[col] = align;
    let width = new.lines[1].slots[col].trim().len().max(3);
    new.lines[1].slots[col] = new.fresh_delim(align, width);
    let mut change = new.commit(&old, doc.text(), None);
    change.selection = Selection::new(
        change.tx.map_offset(sel.anchor, crate::Bias::After),
        change.tx.map_offset(sel.caret, crate::Bias::After),
    );
    Some(change)
}

/// Pasting tab- or line-separated text into a table fills cells from the caret's cell onward,
/// adding rows and columns as needed. Plain text without separators returns `None`.
pub fn table_paste(doc: &Document, sel: Selection, text: &str) -> Option<Change> {
    let text = text.strip_suffix('\n').unwrap_or(text).replace("\r\n", "\n");
    if !text.contains(['\t', '\n']) {
        return None;
    }
    let (_, cell) = cell_at(doc, sel.caret)?;
    let old = Grid::at(doc, sel.caret)?;
    let mut new = old.clone();
    new.normalize();
    let values: Vec<Vec<&str>> = text.split('\n').map(|l| l.split('\t').collect()).collect();
    let need_cols = cell.col + values.iter().map(Vec::len).max().unwrap_or(0);
    while new.cols() < need_cols {
        new.aligns.push(Align::None);
        for (i, line) in new.lines.iter_mut().enumerate() {
            line.slots.push(if i == 1 { old.fresh_delim(Align::None, 3) } else { old.fresh("") });
        }
    }
    for (r, row) in values.iter().enumerate() {
        let line = Grid::line_of(cell.row + r);
        while new.lines.len() <= line {
            let mut fresh = new.lines[new.lines.len() - 1].clone();
            fresh.origin = None;
            fresh.slots = (0..new.cols()).map(|_| new.fresh("")).collect();
            new.lines.push(fresh);
        }
        for (c, value) in row.iter().enumerate() {
            new.lines[line].slots[cell.col + c] = new.fresh(&escape_cell_text(value.trim()));
        }
    }
    let last_row = cell.row + values.len() - 1;
    let last_col = cell.col + values.last().map_or(1, Vec::len) - 1;
    Some(new.commit(&old, doc.text(), Some((Grid::line_of(last_row), last_col, Place::End))))
}

/// Keeps deletions inside cells: Backspace at a cell's start or Delete at its end does nothing,
/// and a selection across cells clears the selected text in each cell instead of the pipes.
/// `None` means the ordinary deletion is fine.
pub fn table_guard_delete(doc: &Document, sel: Selection, backward: bool) -> Option<Change> {
    let text = doc.text();
    let block = doc.block_containing(sel.range().start)?;
    let b = &doc.blocks()[block];
    let shape = b.table.as_ref()?;
    let r = sel.range();
    if r.end > b.range.end || (r.start == b.range.start && r.end == b.range.end && !r.is_empty()) {
        return None; // the whole table, or beyond it: an ordinary deletion
    }
    let contents: Vec<Range<usize>> = shape.rows.iter().flatten().map(|c| cell_content(text, c.clone())).collect();
    if sel.is_empty() {
        let inside = contents.iter().find(|c| c.start <= r.start && r.start <= c.end)?;
        let at_edge = if backward { r.start == inside.start } else { r.start == inside.end };
        return at_edge.then(|| Change::select(sel));
    }
    if contents.iter().any(|c| c.start <= r.start && r.end <= c.end) {
        return None;
    }
    let edits: Vec<Edit> = contents
        .iter()
        .filter_map(|c| {
            let cut = r.start.max(c.start)..r.end.min(c.end);
            (cut.start < cut.end).then(|| Edit { range: cut, insert: String::new() })
        })
        .collect();
    let tx = Transaction::new(edits);
    let caret = tx.map_offset(r.start, crate::Bias::After);
    Some(Change::edit(tx, Selection::caret(caret)))
}

fn cell_range(doc: &Document, block: usize, cell: Cell) -> Option<Range<usize>> {
    doc.blocks()[block].table.as_ref()?.rows.get(cell.row)?.get(cell.col).cloned()
}

fn select_cell(doc: &Document, block: usize, cell: Cell) -> Selection {
    match cell_range(doc, block, cell) {
        Some(range) => {
            let c = cell_content(doc.text(), range);
            Selection::new(c.start, c.end)
        }
        None => Selection::caret(doc.blocks()[block].range.end),
    }
}

// ---- the line model --------------------------------------------------------------------------

/// One source line of a table, split at its unescaped pipes.
#[derive(Clone, Debug)]
struct Line {
    /// Where this line's text started in the source, for lines that already exist.
    origin: Option<Range<usize>>,
    /// What separates this line from the previous one: the line break plus any container prefix
    /// (`> `, list indentation). Empty for the first line.
    sep: String,
    indent: String,
    lead: bool,
    slots: Vec<String>,
    trail: bool,
    tail: String,
}

impl Line {
    fn parse(text: &str) -> Self {
        let body = text.trim_start();
        let indent = text[..text.len() - body.len()].to_owned();
        let mut pipes = Vec::new();
        let mut escaped = false;
        for (i, b) in body.bytes().enumerate() {
            match b {
                b'|' if !escaped => pipes.push(i),
                _ => {}
            }
            escaped = b == b'\\' && !escaped;
        }
        let end = body.trim_end().len();
        let lead = pipes.first() == Some(&0);
        let trail = pipes.last().is_some_and(|&p| p + 1 == end) && (pipes.len() > 1 || !lead);
        let mut slots = Vec::new();
        let mut start = if lead { 1 } else { 0 };
        let inner = if lead { &pipes[1..] } else { &pipes[..] };
        let inner = if trail { &inner[..inner.len() - 1] } else { inner };
        for &p in inner {
            slots.push(body[start..p].to_owned());
            start = p + 1;
        }
        let (last_end, tail) = if trail { (end - 1, &body[end..]) } else { (end, &body[end..]) };
        slots.push(body[start..last_end].to_owned());
        Self { origin: None, sep: String::new(), indent, lead, slots, trail, tail: tail.to_owned() }
    }

    fn render(&self) -> String {
        let mut out = self.indent.clone();
        if self.lead {
            out.push('|');
        }
        out.push_str(&self.slots.join("|"));
        if self.trail {
            out.push('|');
        }
        out.push_str(&self.tail);
        out
    }

    /// The content range of slot `col` within the rendered line.
    fn content_in(&self, col: usize) -> Range<usize> {
        let mut pos = self.indent.len() + usize::from(self.lead);
        for s in self.slots.iter().take(col) {
            pos += s.len() + 1;
        }
        match self.slots.get(col) {
            Some(s) => cell_content(&format!("{}{s}", " ".repeat(pos)), pos..pos + s.len()),
            None => pos..pos,
        }
    }

    /// Unescaped pipe positions, in grapheme columns.
    fn pipe_columns(&self) -> Vec<usize> {
        let text = self.render();
        let mut cols = Vec::new();
        let mut escaped = false;
        for (i, g) in text.graphemes(true).enumerate() {
            if g == "|" && !escaped {
                cols.push(i);
            }
            escaped = g == "\\" && !escaped;
        }
        cols
    }
}

#[derive(Clone, Debug)]
struct Grid {
    lines: Vec<Line>,
    aligns: Vec<Align>,
    aligned: bool,
    /// Spaces between a pipe and cell text, and around delimiter dashes.
    pad: usize,
    delim_pad: usize,
}

fn width(s: &str) -> usize {
    s.graphemes(true).count()
}

fn delim_marks(align: Align, n: usize) -> String {
    let n = n.max(3);
    match align {
        Align::None => "-".repeat(n),
        Align::Left => format!(":{}", "-".repeat(n - 1)),
        Align::Right => format!("{}:", "-".repeat(n - 1)),
        Align::Center => format!(":{}:", "-".repeat(n - 2)),
    }
}

impl Grid {
    fn at(doc: &Document, offset: usize) -> Option<Self> {
        let block = doc.block_containing(offset)?;
        let b = &doc.blocks()[block];
        if b.kind != BlockKind::Table {
            return None;
        }
        let shape = b.table.as_ref()?;
        let text = doc.text();
        let first_line_start = text[..b.range.start].rfind('\n').map_or(0, |i| i + 1);
        let prefix = &text[first_line_start..b.range.start];
        let mut lines = Vec::new();
        let mut pos = b.range.start;
        while pos < b.range.end {
            let end = text[pos..b.range.end].find('\n').map_or(b.range.end, |i| pos + i);
            let content_end = if text[..end].ends_with('\r') { end - 1 } else { end };
            let mut line = Line::parse(&text[pos..content_end]);
            line.origin = Some(pos..content_end);
            if let Some(prev) = lines.last() {
                let prev: &Line = prev;
                line.sep = text[prev.origin.as_ref().unwrap().end..pos].to_owned();
            }
            lines.push(line);
            if end >= b.range.end {
                break;
            }
            // Skip the container prefix on the next line.
            let next = end + 1;
            let skip = if text[next..].starts_with(prefix) {
                prefix.len()
            } else {
                text[next..].bytes().take(prefix.len()).take_while(|b| matches!(b, b' ' | b'>' | b'\t')).count()
            };
            pos = (next + skip).min(b.range.end);
            if text[pos..b.range.end].trim().is_empty() {
                break;
            }
        }
        if lines.len() < 2 {
            return None;
        }
        let columns: Vec<Vec<usize>> = lines.iter().map(Line::pipe_columns).collect();
        let aligned = columns[0].len() >= 2 && columns.iter().all(|c| *c == columns[0]);
        let leading = |s: &str| (s.len() - s.trim_start_matches(' ').len()).min(1);
        let pad = lines[0].slots.first().map_or(1, |s| leading(s));
        let delim_pad = lines[1].slots.first().map_or(0, |s| leading(s));
        Some(Self { lines, aligns: shape.aligns.clone(), aligned, pad, delim_pad })
    }

    /// The source line index of a table row: the delimiter sits between the header and the body.
    fn line_of(row: usize) -> usize {
        if row == 0 { 0 } else { row + 1 }
    }

    fn cols(&self) -> usize {
        self.aligns.len()
    }

    fn fresh(&self, content: &str) -> String {
        let pad = " ".repeat(self.pad);
        format!("{pad}{content}{pad}")
    }

    fn fresh_delim(&self, align: Align, n: usize) -> String {
        let pad = " ".repeat(self.delim_pad);
        format!("{pad}{}{pad}", delim_marks(align, n))
    }

    /// Gives every line at least one slot per column. Extra cells, which GFM ignores, are kept.
    fn normalize(&mut self) {
        let cols = self.cols();
        for i in 0..self.lines.len() {
            let fill = if i == 1 { self.fresh_delim(Align::None, 3) } else { self.fresh("") };
            let line = &mut self.lines[i];
            while line.slots.len() < cols {
                line.slots.push(fill.clone());
            }
        }
    }

    /// Pads every cell so the pipes line up.
    fn realign(&mut self) {
        let cols = self.cols();
        // The delimiter needs at least three dashes: width + 2·pad − 2·delim_pad ≥ 3.
        let min = (3 + 2 * self.delim_pad).saturating_sub(2 * self.pad);
        let mut widths = vec![min; cols];
        for (i, line) in self.lines.iter().enumerate() {
            if i == 1 {
                continue;
            }
            for (c, s) in line.slots.iter().take(cols).enumerate() {
                widths[c] = widths[c].max(width(s.trim()));
            }
        }
        let pad = " ".repeat(self.pad);
        let dpad = " ".repeat(self.delim_pad);
        for (i, line) in self.lines.iter_mut().enumerate() {
            for ((slot, &w), &align) in line.slots.iter_mut().zip(&widths).zip(&self.aligns) {
                *slot = if i == 1 {
                    format!("{dpad}{}{dpad}", delim_marks(align, (w + 2 * self.pad).saturating_sub(2 * self.delim_pad)))
                } else {
                    let content = slot.trim();
                    let fill = " ".repeat(w - width(content));
                    format!("{pad}{content}{fill}{pad}")
                };
            }
        }
    }

    /// The minimal line patches that turn `old` into `self`.
    fn diff(&self, old: &Grid, text: &str) -> Transaction {
        let old_texts: Vec<String> = old.lines.iter().map(|l| text[l.origin.clone().unwrap()].to_owned()).collect();
        let new_texts: Vec<String> = self.lines.iter().map(Line::render).collect();
        let same = |i: usize, j: usize| old_texts[i] == new_texts[j] && old.lines[i].sep == self.lines[j].sep;
        let (n_old, n_new) = (old_texts.len(), new_texts.len());
        let mut p = 0;
        while p < n_old.min(n_new) && same(p, p) {
            p += 1;
        }
        let mut s = 0;
        while s < n_old.min(n_new) - p && same(n_old - 1 - s, n_new - 1 - s) {
            s += 1;
        }
        let (mid_old, mid_new) = (n_old - p - s, n_new - p - s);
        let paired = mid_old.min(mid_new);
        let mut edits = Vec::new();
        for k in 0..paired {
            let range = old.lines[p + k].origin.clone().unwrap();
            if let Some(e) = minimal_edit(&old_texts[p + k], &new_texts[p + k], range.start) {
                edits.push(e);
            }
        }
        if mid_new > paired {
            // Insert the extra lines after the last paired (or common) line.
            let after = (p + paired).checked_sub(1).map_or(0, |i| old.lines[i].origin.as_ref().unwrap().end);
            let insert: String =
                (p + paired..p + mid_new).map(|j| format!("{}{}", self.lines[j].sep, new_texts[j])).collect();
            edits.push(Edit { range: after..after, insert });
        }
        if mid_old > paired {
            let from = old.lines[p + paired - 1].origin.as_ref().unwrap().end;
            let to = old.lines[p + mid_old - 1].origin.as_ref().unwrap().end;
            edits.push(Edit { range: from..to, insert: String::new() });
        }
        Transaction::new(edits)
    }

    /// Patches the source and puts the caret in `(line, col)`'s content (selecting it if asked).
    fn commit(mut self, old: &Grid, text: &str, target: Option<(usize, usize, Place)>) -> Change {
        for i in 1..self.lines.len() {
            if self.lines[i].sep.is_empty() {
                self.lines[i].sep =
                    old.lines.get(i.min(old.lines.len() - 1).max(1)).map_or("\n".into(), |l| l.sep.clone());
            }
        }
        if self.aligned {
            self.realign();
        }
        let tx = self.diff(old, text);
        let start = old.lines[0].origin.as_ref().unwrap().start;
        let selection = match target {
            Some((line, col, place)) => {
                let line = line.min(self.lines.len() - 1);
                let mut at = start;
                for (j, l) in self.lines.iter().enumerate().take(line + 1) {
                    if j > 0 {
                        at += l.sep.len();
                    }
                    if j < line {
                        at += l.render().len();
                    }
                }
                let content = self.lines[line].content_in(col);
                match place {
                    Place::End => Selection::caret(at + content.end),
                    Place::Within(n) => Selection::caret(at + (content.start + n).min(content.end)),
                }
            }
            None => Selection::caret(start),
        };
        Change::edit(tx, selection)
    }
}

/// One edit replacing only what differs between two versions of a line starting at `at`.
fn minimal_edit(old: &str, new: &str, at: usize) -> Option<Edit> {
    if old == new {
        return None;
    }
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, a), _)| i + a.len_utf8());
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let suffix = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>()
        .min(old_rest.len())
        .min(new_rest.len());
    Some(Edit { range: at + prefix..at + old.len() - suffix, insert: new_rest[..new_rest.len() - suffix].to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a table command on `src` (caret at `|` written as `‸`), returning the new text with
    /// the caret marked the same way.
    fn run(src: &str, f: impl Fn(&Document, Selection) -> Option<Change>) -> String {
        let caret = src.find('‸').expect("mark the caret with ‸");
        let mut doc = Document::new(src.replace('‸', ""));
        let change = f(&doc, Selection::caret(caret)).expect("command applies");
        doc.apply(&change.tx);
        let mut out = doc.text().to_owned();
        let r = change.selection.range();
        if r.is_empty() {
            out.insert(r.start, '‸');
        } else {
            out.insert(r.end, '»');
            out.insert(r.start, '«');
        }
        out
    }

    const ALIGNED: &str = "| Element | Glyph |\n|---------|-------|\n| Heading | #     |\n| Table   | \\|    |\n";

    #[test]
    fn finds_cells() {
        let doc = Document::new(ALIGNED);
        let at = ALIGNED.find("Heading").unwrap();
        assert_eq!(cell_at(&doc, at).map(|c| c.1), Some(Cell { row: 1, col: 0 }));
        assert!(table_is_aligned(&doc, at));
        assert!(!table_is_aligned(&Document::new("|a|b|\n|-|-|\n|ccc|d|\n"), 1));
    }

    #[test]
    fn tab_selects_cells_and_adds_a_row_at_the_end() {
        assert_eq!(
            run("| a‸ | b |\n|---|---|\n| c | d |\n", |d, s| table_tab(d, s, false)),
            "| a | «b» |\n|---|---|\n| c | d |\n"
        );
        assert_eq!(
            run("| a | b‸ |\n|---|---|\n| c | d |\n", |d, s| table_tab(d, s, false)),
            "| a | b |\n|---|---|\n| «c» | d |\n"
        );
        assert_eq!(
            run("| a | b |\n|---|---|\n| c | d‸ |\n", |d, s| table_tab(d, s, true)),
            "| a | b |\n|---|---|\n| «c» | d |\n"
        );
        assert_eq!(
            run("| a | b |\n|---|---|\n| c | d‸ |\n", |d, s| table_tab(d, s, false)),
            "| a | b |\n|---|---|\n| c | d |\n| ‸  |   |\n"
        );
    }

    #[test]
    fn enter_moves_down_then_adds_a_row() {
        assert_eq!(run("| a‸ | b |\n|---|---|\n| c | d |\n", table_enter), "| a | b |\n|---|---|\n| c‸ | d |\n");
        assert_eq!(
            run("| a | b |\n|---|---|\n| c | d‸ |\n", table_enter),
            "| a | b |\n|---|---|\n| c | d |\n|   | ‸  |\n"
        );
    }

    #[test]
    fn aligned_tables_stay_aligned() {
        let src = "| Element | Glyph |\n|---------|-------|\n| Heading‸ | #     |\n";
        assert_eq!(
            run(src, |d, s| table_insert_row(d, s, true)),
            "| Element | Glyph |\n|---------|-------|\n| Heading | #     |\n| ‸        |       |\n"
        );
        // Typing makes a cell wider; realigning pads only that column.
        let mut doc = Document::new("| a | b |\n|---|---|\n| c | d |\n");
        doc.apply(&Transaction::insert(2, "aaaa"));
        let tx = realign_table(&doc, 2).unwrap();
        doc.apply(&tx);
        assert_eq!(doc.text(), "| aaaaa | b |\n|-------|---|\n| c     | d |\n");
        assert!(tx.edits().iter().all(|e| !e.insert.contains('b')), "column b untouched: {tx:?}");
    }

    #[test]
    fn compact_tables_stay_compact() {
        assert_eq!(run("|a|b‸|\n|-|-|\n|cc|d|\n", |d, s| table_insert_row(d, s, true)), "|a|b|\n|-|-|\n||‸|\n|cc|d|\n");
    }

    #[test]
    fn rows_and_columns_move_and_delete() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3‸ | 4 |\n";
        assert_eq!(run(src, |d, s| table_move_row(d, s, true)), "| a | b |\n|---|---|\n| 3‸ | 4 |\n| 1 | 2 |\n");
        assert_eq!(run(src, table_delete_row), "| a | b |\n|---|---|\n| 1‸ | 2 |\n");
        assert_eq!(
            run("| a | b |\n|---|---|\n| 1‸ | 2 |\n", |d, s| table_move_row(d, s, true)),
            "| 1‸ | 2 |\n|---|---|\n| a | b |\n",
            "a body row can become the header"
        );
        assert_eq!(run(src, |d, s| table_move_column(d, s, true)), "| b | a |\n|---|---|\n| 2 | 1 |\n| 4 | 3‸ |\n");
        assert_eq!(run(src, table_delete_column), "| b |\n|---|\n| 2 |\n| 4‸ |\n");
        assert_eq!(
            run(src, |d, s| table_insert_column(d, s, true)),
            "| a |   | b |\n|---|---|---|\n| 1 |   | 2 |\n| 3 | ‸  | 4 |\n"
        );
    }

    #[test]
    fn alignment_rewrites_the_delimiter() {
        let src = "| a‸ | b |\n|---|---|\n";
        assert_eq!(run(src, |d, s| table_set_align(d, s, 1, Align::Center)), "| a‸ | b |\n|---|:-:|\n");
    }

    #[test]
    fn deleting_stays_inside_cells() {
        let src = "| ab | cd |\n|---|---|\n";
        let doc = Document::new(src);
        let at_start = Selection::caret(src.find("cd").unwrap());
        assert_eq!(table_guard_delete(&doc, at_start, true).unwrap().tx, Transaction::default());
        let across = Selection::new(src.find('b').unwrap(), src.find('d').unwrap());
        let mut d2 = doc.clone();
        d2.apply(&table_guard_delete(&doc, across, true).unwrap().tx);
        assert_eq!(d2.text(), "| a | d |\n|---|---|\n");
    }

    #[test]
    fn pasting_rows_fills_and_grows_the_table() {
        let src = "| a | b |\n|---|---|\n| ‸ |  |\n";
        assert_eq!(
            run(src, |d, s| table_paste(d, s, "1\t2\t3\n4\t5|6\t7\n")),
            "| a | b |  |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5\\|6 | 7‸ |\n"
        );
    }

    #[test]
    fn escaping() {
        assert_eq!(escape_cell_text("a|b\\|c\nd"), "a\\|b\\|c<br>d");
    }
}
