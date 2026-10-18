//! Table edge handles: a grip above each column and beside each body row, and a ⊕ on the
//! border nearest the pointer for inserting there. They appear when the caret is in the table
//! and fade in as the pointer approaches it.

use crate::view::DocView;

/// How far from a table the pointer starts bringing its handles in.
pub const REACH: f32 = 36.0;
/// How close to a border the pointer must be to offer an insertion there.
const BORDER_SNAP: f32 = 7.0;
/// Click radius around a handle's center.
pub const HIT: f32 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// Selects (and drags) a column.
    Column(usize),
    /// Selects (and drags) a row; dragging a body row to the top makes it the header.
    Row(usize),
    /// Inserts a column at this index.
    AddColumn(usize),
    /// Inserts a body row at this row index.
    AddRow(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Handle {
    pub kind: HandleKind,
    /// The table's block index.
    pub table: usize,
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
}

/// A table's grid lines, from its cell views.
#[derive(Clone, Debug)]
pub struct Grid {
    pub table: usize,
    /// Column edges, left to right (one more than the number of columns).
    pub xs: Vec<f32>,
    /// Row edges, top to bottom (one more than the number of rows).
    pub ys: Vec<f32>,
}

impl Grid {
    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = (self.xs[0] - x).max(x - self.xs.last().unwrap()).max(0.0);
        let dy = (self.ys[0] - y).max(y - self.ys.last().unwrap()).max(0.0);
        dx.max(dy)
    }

    /// The column whose span contains `x`, clamped to the table.
    pub fn column_at(&self, x: f32) -> usize {
        self.xs.windows(2).position(|w| x < w[1]).unwrap_or(self.xs.len() - 2)
    }

    /// The row whose span contains `y`, clamped to the table.
    pub fn row_at(&self, y: f32) -> usize {
        self.ys.windows(2).position(|w| y < w[1]).unwrap_or(self.ys.len() - 2)
    }
}

/// The grid of every table in the view.
pub fn grids(view: &DocView) -> Vec<Grid> {
    let mut grids: Vec<Grid> = Vec::new();
    for b in &view.blocks {
        let Some(cell) = b.cell else { continue };
        if grids.last().is_none_or(|g| g.table != cell.table) {
            grids.push(Grid { table: cell.table, xs: vec![b.box_x], ys: vec![b.top] });
        }
        let g = grids.last_mut().unwrap();
        if cell.row == 0 {
            g.xs.push(b.box_x + b.box_width);
        }
        if cell.col == 0 {
            g.ys.push(b.bottom);
        }
    }
    grids
}

/// The handles to show, given the pointer and which table (if any) holds the caret.
pub fn handles(view: &DocView, pointer: Option<(f32, f32)>, caret_table: Option<usize>) -> Vec<Handle> {
    let mut out = Vec::new();
    for g in grids(view) {
        let near = pointer.map_or(0.0, |(x, y)| 1.0 - (g.distance(x, y) / REACH).min(1.0));
        let alpha = if caret_table == Some(g.table) { near.max(0.55) } else { near };
        if alpha <= 0.0 {
            continue;
        }
        let (top, left) = (g.ys[0], g.xs[0]);
        for c in 0..g.xs.len() - 1 {
            out.push(Handle {
                kind: HandleKind::Column(c),
                table: g.table,
                x: (g.xs[c] + g.xs[c + 1]) / 2.0,
                y: top - 10.0,
                alpha,
            });
        }
        for r in 0..g.ys.len() - 1 {
            out.push(Handle {
                kind: HandleKind::Row(r),
                table: g.table,
                x: left - 12.0,
                y: (g.ys[r] + g.ys[r + 1]) / 2.0,
                alpha,
            });
        }
        // ⊕ only where the pointer touches a border.
        if let Some((x, y)) = pointer.filter(|&(x, y)| g.distance(x, y) < BORDER_SNAP * 2.0) {
            let nearest = |edges: &[f32], at: f32| {
                edges.iter().enumerate().map(|(i, &e)| (i, (e - at).abs())).min_by(|a, b| a.1.total_cmp(&b.1))
            };
            // The closer border wins; the top border (above the header) takes no rows.
            let col = nearest(&g.xs, x).filter(|c| c.1 < BORDER_SNAP);
            let row = nearest(&g.ys[1..], y).map(|(i, d)| (i + 1, d)).filter(|r| r.1 < BORDER_SNAP);
            match (col, row) {
                (Some((i, dx)), r) if r.is_none_or(|r| dx <= r.1) => {
                    out.push(Handle {
                        kind: HandleKind::AddColumn(i),
                        table: g.table,
                        x: g.xs[i],
                        y: top - 10.0,
                        alpha: 1.0,
                    });
                }
                (_, Some((i, _))) => {
                    out.push(Handle {
                        kind: HandleKind::AddRow(i),
                        table: g.table,
                        x: left - 12.0,
                        y: g.ys[i],
                        alpha: 1.0,
                    });
                }
                _ => {}
            }
        }
    }
    // An insertion point replaces the grip it overlaps.
    let adds: Vec<Handle> =
        out.iter().filter(|h| matches!(h.kind, HandleKind::AddColumn(_) | HandleKind::AddRow(_))).copied().collect();
    out.retain(|h| {
        matches!(h.kind, HandleKind::AddColumn(_) | HandleKind::AddRow(_))
            || !adds.iter().any(|a| (a.x - h.x).abs() < HIT * 2.0 && (a.y - h.y).abs() < HIT)
    });
    out
}

/// The handle under a point, if any.
pub fn handle_at(handles: &[Handle], x: f32, y: f32) -> Option<Handle> {
    handles
        .iter()
        .filter(|h| h.alpha > 0.2 && (h.x - x).abs() <= HIT * 1.4 && (h.y - y).abs() <= HIT)
        .min_by(|a, b| ((a.x - x).abs() + (a.y - y).abs()).total_cmp(&((b.x - x).abs() + (b.y - y).abs())))
        .copied()
}
