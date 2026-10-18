//! Undo history as a tree.
//!
//! Undoing and then editing starts a new branch instead of discarding the old one, so no state
//! a document has ever been in is lost; redo follows the most recently visited branch. Each node
//! groups one or more steps (e.g. a burst of typing) that undo and redo together.

pub use marginal_doc::Selection;
use marginal_doc::{Document, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Step {
    forward: Transaction,
    inverse: Transaction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Node {
    parent: usize,
    steps: Vec<Step>,
    before: Selection,
    after: Selection,
    /// Milliseconds since the Unix epoch of the first and last step.
    started: u64,
    updated: u64,
    /// The child redo should go to: the most recently created or visited one.
    redo: Option<usize>,
}

/// How an edit relates to the one before it, for grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditKind {
    /// Typing or deleting characters: merges with an immediately preceding edit of the same kind.
    Typing,
    /// Anything else (paste, formatting, structural commands): always its own undo step.
    Command,
    /// A follow-up that belongs to the previous step, such as tidying away an empty line that
    /// the previous step added: undoes together with it.
    Amend,
}

/// Edits closer together than this, of kind [`EditKind::Typing`], undo as one.
pub const GROUP_MS: u64 = 1200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Node 0 is the root (the document as first opened) and has no steps.
    nodes: Vec<Node>,
    current: usize,
    /// Whether the current node may absorb the next typing edit.
    open_group: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        let root = Node {
            parent: 0,
            steps: Vec::new(),
            before: Selection::default(),
            after: Selection::default(),
            started: 0,
            updated: 0,
            redo: None,
        };
        Self { nodes: vec![root], current: 0, open_group: false }
    }

    /// Applies `tx` to `doc` and records it. Returns the post-edit selection unchanged, for chaining.
    pub fn apply(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
        before: Selection,
        after: Selection,
        kind: EditKind,
        now_ms: u64,
    ) {
        if tx.is_empty() {
            return;
        }
        let inverse = doc.apply(&tx);
        let step = Step { forward: tx, inverse };
        let cur = &mut self.nodes[self.current];
        if kind == EditKind::Amend && self.current != 0 {
            cur.steps.push(step);
            cur.after = after;
            return;
        }
        if kind == EditKind::Typing
            && self.open_group
            && self.current != 0
            && now_ms.saturating_sub(cur.updated) < GROUP_MS
        {
            cur.steps.push(step);
            cur.after = after;
            cur.updated = now_ms;
            return;
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: self.current,
            steps: vec![step],
            before,
            after,
            started: now_ms,
            updated: now_ms,
            redo: None,
        });
        self.nodes[self.current].redo = Some(id);
        self.current = id;
        self.open_group = kind == EditKind::Typing;
    }

    /// Ends the current typing group, e.g. after the caret is moved by hand.
    pub fn break_group(&mut self) {
        self.open_group = false;
    }

    pub fn can_undo(&self) -> bool {
        self.current != 0
    }

    pub fn can_redo(&self) -> bool {
        self.nodes[self.current].redo.is_some()
    }

    /// Undoes the current node. Returns the selection to restore.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Selection> {
        if self.current == 0 {
            return None;
        }
        let node = &self.nodes[self.current];
        for step in node.steps.iter().rev() {
            doc.apply(&step.inverse);
        }
        let selection = node.before;
        let id = self.current;
        self.current = node.parent;
        self.nodes[self.current].redo = Some(id);
        self.open_group = false;
        Some(selection)
    }

    /// Redoes along the most recent branch. Returns the selection to restore.
    pub fn redo(&mut self, doc: &mut Document) -> Option<Selection> {
        let id = self.nodes[self.current].redo?;
        let node = &self.nodes[id];
        for step in &node.steps {
            doc.apply(&step.forward);
        }
        self.current = id;
        self.open_group = false;
        Some(self.nodes[id].after)
    }

    /// Total number of recorded nodes, including the root.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    /// An opaque marker of the current state; equal markers mean identical text.
    pub fn version(&self) -> usize {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(at: usize) -> Selection {
        Selection::caret(at)
    }

    #[test]
    fn typing_groups_and_undoes_together() {
        let mut doc = Document::new("");
        let mut h = History::new();
        for (i, c) in "abc".chars().enumerate() {
            h.apply(
                &mut doc,
                Transaction::insert(i, c.to_string()),
                sel(i),
                sel(i + 1),
                EditKind::Typing,
                100 * i as u64,
            );
        }
        assert_eq!(doc.text(), "abc");
        assert_eq!(h.undo(&mut doc), Some(sel(0)));
        assert_eq!(doc.text(), "");
        assert_eq!(h.redo(&mut doc), Some(sel(3)));
        assert_eq!(doc.text(), "abc");
    }

    #[test]
    fn pauses_and_commands_split_groups() {
        let mut doc = Document::new("");
        let mut h = History::new();
        h.apply(&mut doc, Transaction::insert(0, "a"), sel(0), sel(1), EditKind::Typing, 0);
        h.apply(&mut doc, Transaction::insert(1, "b"), sel(1), sel(2), EditKind::Typing, GROUP_MS + 1);
        h.apply(&mut doc, Transaction::insert(2, "**"), sel(2), sel(4), EditKind::Command, GROUP_MS + 2);
        h.undo(&mut doc);
        assert_eq!(doc.text(), "ab");
        h.undo(&mut doc);
        assert_eq!(doc.text(), "a");
    }

    #[test]
    fn amendments_undo_with_their_step() {
        let mut doc = Document::new("a");
        let mut h = History::new();
        h.apply(&mut doc, Transaction::insert(1, "\n\n"), sel(1), sel(3), EditKind::Command, 0);
        h.apply(&mut doc, Transaction::delete(1..3), sel(3), sel(1), EditKind::Amend, 9_999);
        assert_eq!(doc.text(), "a");
        h.undo(&mut doc);
        assert_eq!(doc.text(), "a");
        assert!(!h.can_undo(), "one step in all");
    }

    #[test]
    fn branches_are_kept() {
        let mut doc = Document::new("x");
        let mut h = History::new();
        h.apply(&mut doc, Transaction::insert(1, "1"), sel(1), sel(2), EditKind::Command, 0);
        h.undo(&mut doc);
        h.apply(&mut doc, Transaction::insert(1, "2"), sel(1), sel(2), EditKind::Command, 1);
        assert_eq!(doc.text(), "x2");
        assert_eq!(h.len(), 3, "the undone branch still exists");
        h.undo(&mut doc);
        assert_eq!(doc.text(), "x");
        // Redo follows the most recent branch.
        h.redo(&mut doc);
        assert_eq!(doc.text(), "x2");
    }
}
