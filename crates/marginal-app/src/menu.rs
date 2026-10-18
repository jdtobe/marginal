//! The context menu on a table column's grip.

use marginal_doc::Align;
use vello::kurbo::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Align(Align),
    InsertLeft,
    InsertRight,
    Delete,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Align(Align::Left) => "Align left",
            MenuItem::Align(Align::Center) => "Align center",
            MenuItem::Align(Align::Right) => "Align right",
            MenuItem::Align(Align::None) => "No alignment",
            MenuItem::InsertLeft => "Insert column left",
            MenuItem::InsertRight => "Insert column right",
            MenuItem::Delete => "Delete column",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            MenuItem::InsertLeft => "⌥⌘←",
            MenuItem::InsertRight => "⌥⌘→",
            MenuItem::Delete => "⇧⌥⌘⌫",
            MenuItem::Align(_) => "",
        }
    }
}

/// The items in order; `None` is a separator.
pub const ITEMS: &[Option<MenuItem>] = &[
    Some(MenuItem::Align(Align::Left)),
    Some(MenuItem::Align(Align::Center)),
    Some(MenuItem::Align(Align::Right)),
    Some(MenuItem::Align(Align::None)),
    None,
    Some(MenuItem::InsertLeft),
    Some(MenuItem::InsertRight),
    None,
    Some(MenuItem::Delete),
];

pub struct GripMenu {
    /// The table's block index and the column the menu acts on.
    pub table: usize,
    pub col: usize,
    /// Top-left corner, in document coordinates.
    pub x: f32,
    pub y: f32,
    /// The highlighted item, for keyboard use.
    pub selected: usize,
    /// Where the last paint put the menu and its rows.
    pub frame: Option<Rect>,
    pub rows: Vec<(Rect, MenuItem)>,
}

impl GripMenu {
    pub fn new(table: usize, col: usize, x: f32, y: f32) -> Self {
        Self { table, col, x, y, selected: 0, frame: None, rows: Vec::new() }
    }

    pub fn items() -> impl Iterator<Item = MenuItem> {
        ITEMS.iter().flatten().copied()
    }

    /// Moves the highlight by one item, wrapping around.
    pub fn step(&mut self, down: bool) {
        let n = Self::items().count();
        self.selected = if down { (self.selected + 1) % n } else { (self.selected + n - 1) % n };
    }
}
