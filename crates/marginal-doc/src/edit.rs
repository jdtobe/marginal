use std::ops::Range;

use serde::{Deserialize, Serialize};

/// A single replacement of a byte range of the source with new text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub range: Range<usize>,
    pub insert: String,
}

/// Caret and anchor, as source byte offsets. The selected text lies between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: usize,
    pub caret: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self { anchor: at, caret: at }
    }

    pub fn new(anchor: usize, caret: usize) -> Self {
        Self { anchor, caret }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }
}

/// Which side an offset sticks to when text is inserted exactly at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Before,
    After,
}

/// A set of non-overlapping edits, expressed in pre-edit byte offsets, applied atomically.
///
/// Every change to a document — typing, WYSIWYG commands, merges, undo — is a `Transaction`,
/// which is what lets the source stay byte-exact everywhere it wasn't touched.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    edits: Vec<Edit>,
}

impl Transaction {
    /// Builds a transaction from edits in any order. Panics if any two edits overlap.
    pub fn new(mut edits: Vec<Edit>) -> Self {
        edits.retain(|e| !(e.range.is_empty() && e.insert.is_empty()));
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        for pair in edits.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "overlapping edits: {:?} and {:?}",
                pair[0].range,
                pair[1].range
            );
        }
        Self { edits }
    }

    pub fn replace(range: Range<usize>, insert: impl Into<String>) -> Self {
        Self::new(vec![Edit { range, insert: insert.into() }])
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::replace(at..at, text)
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self::replace(range, "")
    }

    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Applies the edits to `text` and returns the transaction that undoes them.
    ///
    /// Panics if an edit is out of bounds or splits a UTF-8 character.
    pub fn apply(&self, text: &mut String) -> Transaction {
        for e in &self.edits {
            assert!(
                text.is_char_boundary(e.range.start) && text.is_char_boundary(e.range.end),
                "edit {:?} is not on char boundaries",
                e.range
            );
        }
        let mut inverse = Vec::with_capacity(self.edits.len());
        let mut shift: isize = 0;
        for e in &self.edits {
            let start = (e.range.start as isize + shift) as usize;
            inverse.push(Edit { range: start..start + e.insert.len(), insert: text[e.range.clone()].to_owned() });
            shift += e.insert.len() as isize - e.range.len() as isize;
        }
        // Back to front so earlier offsets stay valid.
        for e in self.edits.iter().rev() {
            text.replace_range(e.range.clone(), &e.insert);
        }
        Transaction { edits: inverse }
    }

    /// Maps a pre-edit offset to its post-edit position.
    pub fn map_offset(&self, offset: usize, bias: Bias) -> usize {
        let mut shift: isize = 0;
        for e in &self.edits {
            if offset < e.range.start || (offset == e.range.start && bias == Bias::Before && e.range.is_empty()) {
                break;
            }
            if offset < e.range.end || (offset == e.range.end && e.range.is_empty()) {
                // Inside a replaced range: snap to the start or end of the inserted text.
                let start = (e.range.start as isize + shift) as usize;
                return match bias {
                    Bias::Before if offset == e.range.start => start,
                    _ => start + e.insert.len(),
                };
            }
            shift += e.insert.len() as isize - e.range.len() as isize;
        }
        (offset as isize + shift) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_invert() {
        let mut s = String::from("hello world");
        let tx = Transaction::new(vec![
            Edit { range: 6..11, insert: "there".into() },
            Edit { range: 0..0, insert: "> ".into() },
        ]);
        let inv = tx.apply(&mut s);
        assert_eq!(s, "> hello there");
        inv.apply(&mut s);
        assert_eq!(s, "hello world");
    }

    #[test]
    fn map_offsets() {
        let tx = Transaction::insert(3, "XY");
        assert_eq!(tx.map_offset(2, Bias::After), 2);
        assert_eq!(tx.map_offset(3, Bias::Before), 3);
        assert_eq!(tx.map_offset(3, Bias::After), 5);
        assert_eq!(tx.map_offset(4, Bias::After), 6);
        let del = Transaction::delete(2..5);
        assert_eq!(del.map_offset(3, Bias::After), 2);
        assert_eq!(del.map_offset(6, Bias::After), 3);
    }
}
