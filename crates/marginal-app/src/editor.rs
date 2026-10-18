//! Editor state for one buffer: selection, scroll, view cache, and the commands the keymap calls.

use std::time::{Duration, Instant};

use marginal_doc::{self as doc, Change, InlineFormat, SegmentKind, Selection, Transaction};
use marginal_history::EditKind;
use marginal_session::Buffer;
use marginal_theme::{Appearance, Color, Theme};
use parley::Affinity;
use parley::editing::Cursor;
use vello::Scene;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::Fill;

use crate::goto::{self, GoTo};
use crate::handles::{self, Handle, HandleKind};
use crate::menu::{GripMenu, MenuItem};
use crate::paint::{color, draw_document, draw_layout};
use crate::picker::{self, Picker, PickerInput};
use crate::view::{DocView, Fonts, Reveal, layout_document, simple_layout};

#[derive(Clone, Copy, Debug)]
struct PaintColors {
    caret: Color,
    selection: Color,
    glyph: Color,
    accent: Color,
    find_match: Color,
    shape: CaretShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretShape {
    Bar,
    Block,
    Underline,
}

pub const BLINK: Duration = Duration::from_millis(530);

pub struct Editor {
    pub buffer: Buffer,
    pub selection: Selection,
    pub theme: Theme,
    pub appearance: Appearance,
    pub scroll: f32,
    pub xray: bool,
    pub width: f32,
    pub height: f32,
    pub preedit: Option<String>,
    /// When the caret last moved or the user typed; blinking restarts from here.
    pub activity: Instant,
    /// Set whenever something worth persisting in the session (selection, scroll) changes.
    pub session_dirty: bool,
    view: Option<DocView>,
    /// Horizontal position kept across consecutive vertical moves.
    goal_x: Option<f32>,
    /// Selection when a drag started, plus the click count (1 = char, 2 = word, 3 = block).
    drag: Option<(Selection, u8)>,
    /// An empty line the last Enter added; removed again if the caret leaves it untouched.
    pending_tidy: Option<PendingTidy>,
    /// The code block language picker, when open.
    pub picker: Option<Picker>,
    /// Most recently chosen code block languages, newest first; kept in the session.
    pub recent_languages: Vec<String>,
    highlighter: marginal_highlight::Highlighter,
    /// The pointer in document coordinates, while it is over the window.
    pub pointer: Option<(f32, f32)>,
    /// A table row or column selected by its grip: ⌫ deletes it, dragging moves it.
    table_focus: Option<(usize, HandleKind)>,
    /// A grip being dragged.
    grip_drag: Option<(usize, HandleKind)>,
    /// The context menu of a column grip, when open.
    pub menu: Option<GripMenu>,
    /// Go to Heading, when open.
    pub goto: Option<GoTo>,
    /// A jump waiting for layout to scroll its target into view.
    pending_scroll_to: Option<usize>,
    /// Paint colors cached for an appearance; cleared by [`Editor::set_theme`].
    colors: Option<(Appearance, PaintColors)>,
    /// Find matches to highlight, and which one is current.
    pub highlights: Vec<std::ops::Range<usize>>,
    pub highlight_current: Option<usize>,
}

struct PendingTidy {
    /// History version right after the Enter: any other edit since cancels the tidy.
    version: usize,
    /// The inserted text, in current coordinates.
    range: std::ops::Range<usize>,
}

impl Editor {
    pub fn new(buffer: Buffer, selection: Selection, theme: Theme, appearance: Appearance) -> Self {
        let len = buffer.doc.text().len();
        let mut editor = Self {
            buffer,
            selection: Selection::new(selection.anchor.min(len), selection.caret.min(len)),
            theme,
            appearance,
            scroll: 0.0,
            xray: false,
            width: 900.0,
            height: 700.0,
            preedit: None,
            activity: Instant::now(),
            session_dirty: false,
            view: None,
            goal_x: None,
            drag: None,
            pending_tidy: None,
            picker: None,
            recent_languages: Vec::new(),
            highlighter: marginal_highlight::Highlighter::new(),
            pointer: None,
            table_focus: None,
            grip_drag: None,
            menu: None,
            goto: None,
            colors: None,
            pending_scroll_to: None,
            highlights: Vec::new(),
            highlight_current: None,
        };
        if selection == Selection::default() {
            editor.selection = Selection::caret(editor.first_content_offset());
        }
        editor
    }

    fn first_content_offset(&self) -> usize {
        self.buffer.doc.blocks().first().map_or(0, |b| {
            b.segments
                .iter()
                .find(|s| !matches!(s.kind, SegmentKind::Syntax { .. }))
                .map_or(b.range.start, |s| s.range.start)
        })
    }

    pub fn invalidate(&mut self) {
        self.view = None;
    }

    pub fn view(&mut self, fonts: &mut Fonts) -> &DocView {
        if self.view.is_none() {
            let _t = crate::trace::span(|| "  layout".into());
            fonts.load_theme_fonts(&self.theme);
            let reveal = Reveal { caret: Some(self.selection.caret), xray: self.xray, preedit: self.preedit.clone() };
            self.view = Some(layout_document(
                &self.buffer.doc,
                &self.theme,
                self.appearance,
                fonts,
                self.width,
                &reveal,
                &mut self.highlighter,
            ));
        }
        self.view.as_ref().unwrap()
    }

    fn var(&self, name: &str) -> Option<String> {
        self.theme.var(self.appearance, name).map(|v| v.trim().to_owned())
    }

    pub fn caret_shape(&self) -> CaretShape {
        match self.var("--marginal-caret-shape").as_deref() {
            Some("block") => CaretShape::Block,
            Some("underline") => CaretShape::Underline,
            _ => CaretShape::Bar,
        }
    }

    pub fn blinks(&self) -> bool {
        self.var("--marginal-caret-blink").as_deref() == Some("on")
    }

    /// Whether the caret is in the visible half of its blink cycle, and when that next changes.
    pub fn blink_phase(&self, now: Instant) -> (bool, Option<Instant>) {
        if !self.blinks() {
            return (true, None);
        }
        let elapsed = now.duration_since(self.activity);
        let cycles = elapsed.as_millis() / BLINK.as_millis();
        let next = self.activity + BLINK * (cycles as u32 + 1);
        (cycles.is_multiple_of(2), Some(next))
    }

    pub fn background(&self) -> Color {
        self.theme.style(self.appearance, "body", &[], None).background.unwrap_or(Color::WHITE)
    }

    /// A color from a theme variable.
    fn var_color(&self, name: &str) -> Option<Color> {
        let value = self.var(name)?;
        let theme = Theme::parse(&format!("p {{ color: {value} }}")).ok()?;
        Some(theme.style(Appearance::Light, "p", &[], None).color)
    }

    fn selection_color(&self, fallback: Color) -> Color {
        self.var_color("--selection").unwrap_or(fallback.with_alpha(0.25))
    }

    /// Draws the document. `base` maps editor coordinates to device pixels (scale and zoom).
    /// Replaces the theme (and drops colors cached from the old one).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.colors = None;
        self.invalidate();
    }

    /// Colors the painter needs every frame, resolved once per theme and appearance.
    fn colors(&mut self) -> PaintColors {
        if let Some((appearance, colors)) = self.colors
            && appearance == self.appearance
        {
            return colors;
        }
        let caret = self.theme.style(self.appearance, "span", &["caret"], None).color;
        let colors = PaintColors {
            caret,
            selection: self.selection_color(caret),
            glyph: self.theme.style(self.appearance, "span", &["glyph"], None).color,
            accent: self.theme.style(self.appearance, "a", &[], None).color,
            find_match: self.var_color("--find-match").unwrap_or(Color { r: 210, g: 153, b: 34, a: 82 }),
            shape: self.caret_shape(),
        };
        self.colors = Some((self.appearance, colors));
        colors
    }

    pub fn paint(&mut self, scene: &mut Scene, fonts: &mut Fonts, base: Affine, now: Instant) {
        let colors = self.colors();
        let (caret_color, selection_color, shape) = (colors.caret, colors.selection, colors.shape);
        let (visible, _) = self.blink_phase(now);
        let scroll = self.scroll;
        let selection = self.selection;
        let has_preedit = self.preedit.is_some();
        let (pointer, caret_table, focus, dragging) =
            (self.pointer, self.caret_table(), self.table_focus, self.grip_drag.zip(self.pointer));
        let (glyph_color, accent, match_color) = (colors.glyph, colors.accent, colors.find_match);
        let (highlights, current) = (std::mem::take(&mut self.highlights), self.highlight_current);
        let height = self.height;
        let view = self.view(fonts);
        let origin = base * Affine::translate((0.0, -scroll as f64));
        draw_document(scene, view, origin);
        let handles = handles::handles(view, pointer, caret_table);
        let grids = handles::grids(view);
        paint_handles(scene, origin, &handles, &grids, focus, dragging, glyph_color, accent, selection_color);
        let visible_range =
            view.hit_test(0.0, scroll).unwrap_or(0)..view.hit_test(f32::MAX, scroll + height).unwrap_or(usize::MAX);
        for (i, m) in
            highlights.iter().enumerate().filter(|(_, m)| m.end >= visible_range.start && m.start <= visible_range.end)
        {
            let c = if Some(i) == current { Color { a: 170, ..match_color } } else { match_color };
            for (x0, y0, x1, y1) in view.selection_rects(m.clone()) {
                scene.fill(
                    Fill::NonZero,
                    origin,
                    color(c),
                    None,
                    &Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64),
                );
            }
        }
        self.highlights = highlights;
        let view = self.view.as_ref().unwrap();
        for (x0, y0, x1, y1) in view.selection_rects(selection.range()) {
            let rect = Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64);
            scene.fill(Fill::NonZero, origin, color(selection_color), None, &rect);
        }
        if visible
            && !has_preedit
            && let Some(c) = view.caret_box(selection.caret)
        {
            let (x, y, h) = (c.x as f64, c.y as f64, c.height as f64);
            let (rect, alpha) = match shape {
                CaretShape::Block if selection.is_empty() => (Rect::new(x, y, x + c.advance as f64, y + h), 0.45),
                CaretShape::Underline if selection.is_empty() => {
                    (Rect::new(x, y + h - 2.0, x + c.advance as f64, y + h), 1.0)
                }
                _ => (Rect::new(x, y, x + 2.0, y + h), 1.0),
            };
            scene.fill(Fill::NonZero, origin, color(caret_color.with_alpha(alpha)), None, &rect);
        }
        self.paint_picker(scene, fonts, origin, selection_color);
        self.paint_menu(scene, fonts, origin, selection_color);
        self.paint_goto(scene, fonts, origin, selection_color);
    }

    // ---- go to heading -----------------------------------------------------------------------

    pub fn open_goto(&mut self) {
        self.goto = Some(GoTo::default());
        self.picker = None;
        self.menu = None;
        self.activity = Instant::now();
    }

    pub fn goto_input(&mut self, input: PickerInput) {
        let all = goto::headings(&self.buffer.doc);
        let Some(g) = &mut self.goto else { return };
        let count = g.items(&all).len();
        match input {
            PickerInput::Up => g.selected = g.selected.saturating_sub(1),
            PickerInput::Down => g.selected = (g.selected + 1).min(count.saturating_sub(1)),
            PickerInput::Backspace => {
                g.query.pop();
                g.selected = 0;
            }
            PickerInput::Text(t) => {
                g.query.push_str(&t);
                g.selected = 0;
            }
            PickerInput::Cancel => self.goto = None,
            PickerInput::Accept => {
                if let Some(h) = g.items(&all).get(g.selected) {
                    let offset = h.offset;
                    self.goto = None;
                    self.jump_to(offset);
                } else {
                    self.goto = None;
                }
            }
        }
        self.activity = Instant::now();
    }

    /// Puts the caret at `offset` and scrolls it near the top of the window.
    fn jump_to(&mut self, offset: usize) {
        self.set_selection(Selection::caret(offset));
        self.pending_scroll_to = Some(offset);
    }

    /// Applies a jump requested before layout was available (needs fonts).
    pub fn settle_scroll(&mut self, fonts: &mut Fonts) {
        if let Some(offset) = self.pending_scroll_to.take()
            && let Some(c) = self.view(fonts).caret_box(offset)
        {
            let max = (self.view(fonts).height - self.height).max(0.0);
            self.scroll = (c.y - 64.0).clamp(0.0, max);
            self.session_dirty = true;
        }
    }

    /// Draws Go to Heading centered near the top of the window.
    fn paint_goto(&mut self, scene: &mut Scene, fonts: &mut Fonts, origin: Affine, highlight: Color) {
        let Some(g) = &self.goto else { return };
        let all = goto::headings(&self.buffer.doc);
        let items = g.items(&all);
        let body = self.theme.style(self.appearance, "body", &[], None);
        let mut style = self.theme.style(self.appearance, "span", &["chip"], Some(&body));
        style.font_size = body.font_size.min(15.0);
        let muted = self.theme.style(self.appearance, "span", &["glyph"], Some(&body)).color;
        let mut placeholder = style.clone();
        placeholder.color = muted;
        let query = if g.query.is_empty() {
            simple_layout(fonts, "Go to heading…", &placeholder, None)
        } else {
            simple_layout(fonts, &g.query, &style, None)
        };
        let first = g.first_visible();
        let rows: Vec<_> = items
            .iter()
            .enumerate()
            .skip(first)
            .take(goto::VISIBLE)
            .map(|(i, h)| (i, h.offset, h.level, simple_layout(fonts, &h.text, &style, Some(420.0))))
            .collect();
        let (pad, width) = (10.0, 460.0_f64);
        let row_h = query.height() as f64 + 8.0;
        let height = pad * 2.0 + row_h * (rows.len().max(1) + 1) as f64 + 4.0;
        let x0 = ((self.width as f64 - width) / 2.0).max(8.0);
        let y0 = self.scroll as f64 + 16.0;
        let frame = Rect::new(x0, y0, x0 + width, y0 + height);
        scene.fill(Fill::NonZero, origin, color(self.background()), None, &RoundedRect::from_rect(frame, 8.0));
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(muted.with_alpha(0.6)),
            None,
            &RoundedRect::from_rect(frame, 8.0),
        );
        draw_layout(scene, &query, origin * Affine::translate((x0 + pad + 4.0, y0 + pad + 4.0)));
        let rule_y = y0 + pad + row_h + 2.0;
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(muted.with_alpha(0.4)),
            None,
            &Line::new((x0 + pad, rule_y), (x0 + width - pad, rule_y)),
        );
        let mut hits = Vec::new();
        if rows.is_empty() {
            let none = simple_layout(fonts, "No matching headings", &placeholder, None);
            draw_layout(scene, &none, origin * Affine::translate((x0 + pad + 4.0, rule_y + 6.0)));
        }
        for (n, (i, offset, level, layout)) in rows.iter().enumerate() {
            let top = rule_y + 4.0 + row_h * n as f64;
            let rect = Rect::new(x0 + 4.0, top, x0 + width - 4.0, top + row_h);
            if *i == g.selected {
                scene.fill(Fill::NonZero, origin, color(highlight), None, &RoundedRect::from_rect(rect, 4.0));
            }
            let indent = (*level as f64 - 1.0) * 14.0;
            let mark = simple_layout(fonts, &"#".repeat(*level as usize), &placeholder, None);
            let ty = top + (row_h - layout.height() as f64) / 2.0;
            draw_layout(scene, &mark, origin * Affine::translate((x0 + pad + 4.0 + indent, ty)));
            draw_layout(
                scene,
                layout,
                origin * Affine::translate((x0 + pad + 4.0 + indent + mark.width() as f64 + 8.0, ty)),
            );
            hits.push((rect, *offset));
        }
        let g = self.goto.as_mut().unwrap();
        g.frame = Some(frame);
        g.rows = hits;
    }

    /// Draws the column grip's context menu and records where its rows landed.
    fn paint_menu(&mut self, scene: &mut Scene, fonts: &mut Fonts, origin: Affine, highlight: Color) {
        let Some(menu) = &self.menu else { return };
        let body = self.theme.style(self.appearance, "body", &[], None);
        let style = self.theme.style(self.appearance, "span", &["chip"], Some(&body));
        let muted = self.theme.style(self.appearance, "span", &["glyph"], Some(&body)).color;
        let current =
            self.buffer.doc.blocks().get(menu.table).and_then(|b| b.table.as_ref()?.aligns.get(menu.col).copied());
        let mut short_style = style.clone();
        short_style.color = muted;
        // Each row: a check for the current alignment, the label, the shortcut.
        let rows: Vec<Option<(MenuItem, _, _)>> = crate::menu::ITEMS
            .iter()
            .map(|item| {
                item.map(|item| {
                    let check = if item == MenuItem::Align(current.unwrap_or(marginal_doc::Align::None)) {
                        "✓ "
                    } else {
                        "  "
                    };
                    let label = simple_layout(fonts, &format!("{check}{}", item.label()), &style, None);
                    let short = simple_layout(fonts, item.shortcut(), &short_style, None);
                    (item, label, short)
                })
            })
            .collect();
        let (pad, row_h, sep_h) = (6.0, style.font_size as f64 * 1.9, 9.0);
        let widest = rows.iter().flatten().map(|(_, l, s)| l.width() + s.width() + 28.0).fold(0.0_f32, f32::max) as f64;
        let width = widest + pad * 2.0 + 8.0;
        let height = pad * 2.0 + rows.iter().map(|r| if r.is_some() { row_h } else { sep_h }).sum::<f64>();
        let (x0, y0) = (menu.x as f64, menu.y as f64);
        let frame = Rect::new(x0, y0, x0 + width, y0 + height);
        scene.fill(Fill::NonZero, origin, color(self.background()), None, &RoundedRect::from_rect(frame, 6.0));
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(muted.with_alpha(0.6)),
            None,
            &RoundedRect::from_rect(frame, 6.0),
        );
        let mut top = y0 + pad;
        let mut hits = Vec::new();
        let mut index = 0;
        for row in rows {
            match row {
                None => {
                    let y = top + sep_h / 2.0;
                    scene.stroke(
                        &Stroke::new(1.0),
                        origin,
                        color(muted.with_alpha(0.3)),
                        None,
                        &Line::new((x0 + pad, y), (x0 + width - pad, y)),
                    );
                    top += sep_h;
                }
                Some((item, label, short)) => {
                    let rect = Rect::new(x0 + 3.0, top, x0 + width - 3.0, top + row_h);
                    if index == menu.selected {
                        scene.fill(Fill::NonZero, origin, color(highlight), None, &RoundedRect::from_rect(rect, 3.0));
                    }
                    let ty = top + (row_h - label.height() as f64) / 2.0;
                    draw_layout(scene, &label, origin * Affine::translate((x0 + pad + 4.0, ty)));
                    draw_layout(
                        scene,
                        &short,
                        origin * Affine::translate((x0 + width - pad - 4.0 - short.width() as f64, ty)),
                    );
                    hits.push((rect, item));
                    top += row_h;
                    index += 1;
                }
            }
        }
        let menu = self.menu.as_mut().unwrap();
        menu.frame = Some(frame);
        menu.rows = hits;
    }

    /// Right-click: opens the menu of a column grip under the pointer. Returns whether to redraw.
    pub fn context_click(&mut self, fonts: &mut Fonts, x: f32, y: f32) -> bool {
        let (x, y) = (x, y + self.scroll);
        let had_menu = self.menu.take().is_some();
        match self.handle_at(fonts, x, y) {
            Some(Handle { kind: HandleKind::Column(col), table, x: hx, y: hy, .. }) => {
                self.menu = Some(GripMenu::new(table, col, hx - 10.0, hy + 10.0));
                self.table_focus = Some((table, HandleKind::Column(col)));
                true
            }
            _ => had_menu,
        }
    }

    /// Opens the grip menu of column `col` in the first table (for snapshots).
    pub fn open_grip_menu(&mut self, fonts: &mut Fonts, col: usize) -> bool {
        let caret_table = self.caret_table();
        let view = self.view(fonts);
        let Some(h) = handles::handles(view, None, caret_table).into_iter().find(|h| h.kind == HandleKind::Column(col))
        else {
            return false;
        };
        self.context_click(fonts, h.x, h.y - self.scroll)
    }

    /// Keys while the grip menu is open.
    pub fn menu_input(&mut self, input: PickerInput) {
        let Some(menu) = &mut self.menu else { return };
        match input {
            PickerInput::Up => menu.step(false),
            PickerInput::Down => menu.step(true),
            PickerInput::Accept => {
                if let Some(item) = GripMenu::items().nth(menu.selected) {
                    self.choose_menu(item);
                }
            }
            PickerInput::Cancel => self.menu = None,
            PickerInput::Backspace | PickerInput::Text(_) => {}
        }
        self.activity = Instant::now();
    }

    fn choose_menu(&mut self, item: MenuItem) {
        let Some(menu) = self.menu.take() else { return };
        let (table, col) = (menu.table, menu.col);
        match item {
            MenuItem::Align(a) => {
                self.table_command_at(table, 0, col, |d, s| doc::table_set_align(d, s, col, a));
                self.table_focus = Some((table, HandleKind::Column(col)));
            }
            MenuItem::InsertLeft | MenuItem::InsertRight => {
                let right = item == MenuItem::InsertRight;
                self.table_command_at(table, 0, col, |d, s| doc::table_insert_column(d, s, right));
                self.table_focus = None;
            }
            MenuItem::Delete => {
                self.table_command_at(table, 0, col, doc::table_delete_column);
                self.table_focus = None;
            }
        }
    }

    /// The table block holding the caret, if any.
    fn caret_table(&self) -> Option<usize> {
        doc::cell_at(&self.buffer.doc, self.selection.caret).map(|(b, _)| b)
    }

    /// Tracks the pointer (window coordinates) for table handles. Returns whether to redraw.
    pub fn pointer_moved(&mut self, fonts: &mut Fonts, at: Option<(f32, f32)>) -> bool {
        let at = at.map(|(x, y)| (x, y + self.scroll));
        let near = |view: &DocView, p: Option<(f32, f32)>| {
            p.is_some_and(|(x, y)| {
                handles::grids(view).iter().any(|g| {
                    g.xs[0] - handles::REACH * 2.0 < x
                        && x < g.xs.last().unwrap() + handles::REACH * 2.0
                        && g.ys[0] - handles::REACH * 2.0 < y
                        && y < g.ys.last().unwrap() + handles::REACH * 2.0
                })
            })
        };
        let before = self.pointer;
        self.pointer = at;
        let view = self.view(fonts);
        near(view, before) || near(view, at) || self.grip_drag.is_some()
    }

    fn handle_at(&mut self, fonts: &mut Fonts, x: f32, y: f32) -> Option<Handle> {
        let caret_table = self.caret_table();
        let pointer = self.pointer;
        let view = self.view(fonts);
        handles::handle_at(&handles::handles(view, pointer, caret_table), x, y)
    }

    /// The caret at the end of a cell's text.
    fn caret_in_cell(&self, table: usize, row: usize, col: usize) -> Option<Selection> {
        let cell = self.buffer.doc.blocks().get(table)?.table.as_ref()?.rows.get(row)?.get(col)?.clone();
        Some(Selection::caret(doc::cell_content(self.buffer.doc.text(), cell).end))
    }

    /// Runs a table command with the caret placed in a given cell first.
    fn table_command_at(
        &mut self,
        table: usize,
        row: usize,
        col: usize,
        f: impl FnOnce(&doc::Document, Selection) -> Option<Change>,
    ) {
        if let Some(sel) = self.caret_in_cell(table, row, col)
            && let Some(change) = f(&self.buffer.doc, sel)
        {
            self.selection = sel;
            self.run(change);
        }
    }

    fn click_handle(&mut self, h: Handle) {
        match h.kind {
            HandleKind::AddColumn(0) => {
                self.table_command_at(h.table, 0, 0, |d, s| doc::table_insert_column(d, s, false))
            }
            HandleKind::AddColumn(i) => {
                self.table_command_at(h.table, 0, i - 1, |d, s| doc::table_insert_column(d, s, true))
            }
            HandleKind::AddRow(r) => self.table_command_at(h.table, r - 1, 0, |d, s| doc::table_insert_row(d, s, true)),
            kind @ (HandleKind::Column(_) | HandleKind::Row(_)) => {
                self.table_focus = Some((h.table, kind));
                self.grip_drag = Some((h.table, kind));
                self.activity = Instant::now();
            }
        }
    }

    /// Drops a dragged grip: moves its row or column to where the pointer is.
    fn drop_grip(&mut self, fonts: &mut Fonts) {
        let (Some((table, kind)), Some((x, y))) = (self.grip_drag.take(), self.pointer) else { return };
        let Some(grid) = handles::grids(self.view(fonts)).into_iter().find(|g| g.table == table) else { return };
        let (from, to) = match kind {
            HandleKind::Column(c) => (c, grid.column_at(x)),
            HandleKind::Row(r) => (r, grid.row_at(y)),
            _ => return,
        };
        let mut at = from;
        let mut first = true;
        while at != to {
            let forward = to > at;
            let (row, col) = if matches!(kind, HandleKind::Column(_)) { (0, at) } else { (at, 0) };
            let Some(sel) = self.caret_in_cell(table, row, col) else { break };
            let change = match kind {
                HandleKind::Column(_) => doc::table_move_column(&self.buffer.doc, sel, forward),
                _ => doc::table_move_row(&self.buffer.doc, sel, !forward),
            };
            let Some(change) = change.filter(|c| !c.tx.is_empty()) else { break };
            // The whole drag undoes as one step.
            let kind = if first { EditKind::Command } else { EditKind::Amend };
            if let Err(e) = self.buffer.edit(change.tx, sel, change.selection, kind) {
                eprintln!("journal write failed: {e}");
            }
            self.selection = change.selection;
            first = false;
            at = if forward { at + 1 } else { at - 1 };
        }
        self.table_focus = Some((
            table,
            if matches!(kind, HandleKind::Column(_)) { HandleKind::Column(at) } else { HandleKind::Row(at) },
        ));
        self.touched();
    }

    /// Deletes the row or column selected by its grip. Returns whether one was selected.
    fn delete_focused(&mut self) -> bool {
        let Some((table, kind)) = self.table_focus.take() else { return false };
        match kind {
            HandleKind::Column(c) => self.table_command_at(table, 0, c, doc::table_delete_column),
            HandleKind::Row(r) => self.table_command_at(table, r, 0, doc::table_delete_row),
            _ => {}
        }
        true
    }

    /// Esc: drops a grip selection. Returns whether there was one.
    pub fn clear_table_focus(&mut self) -> bool {
        self.grip_drag = None;
        self.table_focus.take().is_some()
    }

    /// Draws the language picker under its chip and records where its rows landed.
    fn paint_picker(&mut self, scene: &mut Scene, fonts: &mut Fonts, origin: Affine, highlight: Color) {
        if self.picker.is_none() {
            return;
        }
        let caret = self.selection.caret;
        let Some((chip, block_style)) = self
            .view(fonts)
            .block_for_offset(caret)
            .and_then(|b| Some((b.chip.as_ref()?.chip_bounds(), b.style.clone())))
        else {
            return;
        };
        let theme = &self.theme;
        let style = theme.style(self.appearance, "span", &["chip"], Some(&block_style));
        let muted = theme.style(self.appearance, "span", &["glyph"], Some(&block_style)).color;
        let background = self.background();
        let picker = self.picker.as_ref().unwrap();
        let items = picker.items(&self.recent_languages);
        let first = picker.first_visible();
        let query = if picker.query.is_empty() { "language…" } else { picker.query.as_str() };
        let mut query_style = style.clone();
        if picker.query.is_empty() {
            query_style.color = muted;
        }
        let query_layout = simple_layout(fonts, query, &query_style, None);
        let layouts: Vec<_> = items
            .iter()
            .enumerate()
            .skip(first)
            .take(picker::VISIBLE)
            .map(|(i, item)| (i, item.clone(), simple_layout(fonts, item, &style, None)))
            .collect();

        let (pad, gap) = (8.0, 4.0);
        let row_h = query_layout.height() as f64 + 4.0;
        let widest = layouts.iter().map(|l| l.2.width()).chain([query_layout.width()]).fold(0.0_f32, f32::max) as f64;
        let width = (widest + pad * 2.0).max(150.0);
        let height = pad * 2.0 + row_h * (layouts.len() + 1) as f64 + gap;
        let x0 = chip.x1 - width;
        let y0 = chip.y1 + 4.0;
        let frame = Rect::new(x0, y0, x0 + width, y0 + height);
        scene.fill(Fill::NonZero, origin, color(background), None, &RoundedRect::from_rect(frame, 6.0));
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(muted.with_alpha(0.6)),
            None,
            &RoundedRect::from_rect(frame, 6.0),
        );

        let text_at = |top: f64| origin * Affine::translate((x0 + pad, y0 + top + 2.0));
        draw_layout(scene, &query_layout, text_at(pad));
        let rule_y = y0 + pad + row_h + gap / 2.0;
        let rule = Line::new((x0 + pad, rule_y), (x0 + width - pad, rule_y));
        scene.stroke(&Stroke::new(1.0), origin, color(muted.with_alpha(0.4)), None, &rule);

        let recents = if picker.query.trim().is_empty() { self.recent_languages.len() } else { 0 };
        let mut rows = Vec::new();
        for (n, (i, item, layout)) in layouts.iter().enumerate() {
            let top = pad + gap + row_h * (n + 1) as f64;
            let row = Rect::new(x0 + 3.0, y0 + top, x0 + width - 3.0, y0 + top + row_h);
            if *i == picker.selected {
                scene.fill(Fill::NonZero, origin, color(highlight), None, &RoundedRect::from_rect(row, 3.0));
            }
            // A faint line between the recent languages and the rest.
            if recents > 0 && *i == recents && n > 0 {
                let line = Line::new((x0 + pad, y0 + top), (x0 + width - pad, y0 + top));
                scene.stroke(&Stroke::new(1.0), origin, color(muted.with_alpha(0.25)), None, &line);
            }
            draw_layout(scene, layout, text_at(top));
            rows.push((row, item.clone()));
        }
        let picker = self.picker.as_mut().unwrap();
        picker.frame = Some(frame);
        picker.rows = rows;
    }

    /// Caret rectangle in window coordinates (for placing the IME candidate window).
    pub fn caret_window_rect(&mut self, fonts: &mut Fonts) -> Option<(f32, f32, f32, f32)> {
        let scroll = self.scroll;
        let caret = self.selection.caret;
        let c = self.view(fonts).caret_box(caret)?;
        Some((c.x, c.y - scroll, c.advance, c.height))
    }

    // ---- editing -----------------------------------------------------------------------------

    fn touched(&mut self) {
        self.activity = Instant::now();
        self.session_dirty = true;
        self.invalidate();
    }

    pub fn run(&mut self, change: Change) {
        self.pending_tidy = None;
        let before = self.selection;
        // An aligned table is re-padded after edits inside it, in the same undo step.
        let realign = !change.tx.is_empty() && doc::table_is_aligned(&self.buffer.doc, before.caret);
        if !change.tx.is_empty() {
            let kind = if change.typing { EditKind::Typing } else { EditKind::Command };
            if let Err(e) = self.buffer.edit(change.tx, before, change.selection, kind) {
                eprintln!("journal write failed: {e}");
            }
        } else {
            self.buffer.history.break_group();
        }
        self.selection = change.selection;
        if realign && let Some(tx) = doc::realign_table(&self.buffer.doc, self.selection.caret) {
            let mapped = Selection::new(
                tx.map_offset(self.selection.anchor, doc::Bias::Before),
                tx.map_offset(self.selection.caret, doc::Bias::Before),
            );
            if let Err(e) = self.buffer.edit(tx, self.selection, mapped, EditKind::Amend) {
                eprintln!("journal write failed: {e}");
            }
            self.selection = mapped;
        }
        self.goal_x = None;
        self.touched();
    }

    /// Whether the caret is in a table cell.
    pub fn in_table(&self) -> bool {
        doc::cell_at(&self.buffer.doc, self.selection.caret).is_some()
    }

    pub fn type_text(&mut self, text: &str) {
        self.preedit = None;
        let escaped = if self.in_table() { doc::escape_cell_text(text) } else { text.to_owned() };
        let mut change = doc::insert_text(&self.buffer.doc, self.selection, &escaped);
        // A typed `|` written as `\|` is still one keystroke of typing.
        if escaped != text && self.selection.is_empty() && text.chars().count() == 1 {
            change.typing = true;
        }
        self.run(change);
    }

    /// Runs a table command if the caret is in a table. Returns whether it was.
    pub fn table_command(&mut self, f: impl FnOnce(&doc::Document, Selection) -> Option<Change>) -> bool {
        match f(&self.buffer.doc, self.selection) {
            Some(change) => {
                self.run(change);
                true
            }
            None => false,
        }
    }

    pub fn newline(&mut self) {
        if self.table_command(doc::table_enter) {
            return;
        }
        let before_version = self.buffer.history.version();
        let previous = self.pending_tidy.take();
        let collapsed = self.selection.is_empty();
        let change = doc::insert_newline(&self.buffer.doc, self.selection);
        // Only a pure insertion that leaves the caret on an empty line can be tidied later.
        let inserted = match change.tx.edits() {
            [e] if e.range.is_empty() && collapsed => Some(e.range.start..e.range.start + e.insert.len()),
            _ => None,
        };
        self.run(change);
        let text = self.buffer.doc.text();
        let caret = self.selection.caret;
        let on_empty_line = match self.buffer.doc.block_containing(caret) {
            None => true,
            Some(i) => self.buffer.doc.blocks()[i].is_empty_content(text),
        };
        if let (Some(mut range), true) = (inserted, on_empty_line) {
            // Consecutive Enters on empty lines tidy away together.
            if let Some(p) = previous.filter(|p| p.version == before_version && p.range.end == range.start) {
                range.start = p.range.start;
            }
            self.pending_tidy = Some(PendingTidy { version: self.buffer.history.version(), range });
        }
    }

    /// Removes the empty line from the last Enter if the caret is leaving it untouched.
    fn tidy_before_moving(&mut self, target: Selection) -> Selection {
        let Some(p) = self.pending_tidy.take() else { return target };
        let caret_stays = p.range.start <= target.caret && target.caret <= p.range.end;
        if self.buffer.history.version() != p.version || caret_stays {
            if caret_stays {
                self.pending_tidy = Some(p);
            }
            return target;
        }
        let tx = Transaction::delete(p.range);
        let mapped = Selection::new(
            tx.map_offset(target.anchor, marginal_doc::Bias::After),
            tx.map_offset(target.caret, marginal_doc::Bias::After),
        );
        if let Err(e) = self.buffer.edit(tx, self.selection, mapped, EditKind::Amend) {
            eprintln!("journal write failed: {e}");
        }
        self.invalidate();
        mapped
    }

    /// Shift+Enter.
    pub fn line_break(&mut self) {
        // Table cells can't hold real line breaks.
        if self.in_table() {
            return self.type_text("\n");
        }
        let change = doc::insert_line_break(&self.buffer.doc, self.selection);
        self.run(change);
    }

    /// ⌘K.
    pub fn toggle_link(&mut self) {
        let change = doc::toggle_link(&self.buffer.doc, self.selection);
        self.run(change);
    }

    pub fn backspace(&mut self) {
        if self.delete_focused() {
            return;
        }
        let change = doc::delete_backward(&self.buffer.doc, self.selection);
        self.run(change);
    }

    pub fn delete_forward(&mut self) {
        if self.delete_focused() {
            return;
        }
        let change = doc::delete_forward(&self.buffer.doc, self.selection);
        self.run(change);
    }

    /// ⌥⌫ / ⌘⌫: deletes back to the previous word or line start.
    pub fn delete_back_by(&mut self, fonts: &mut Fonts, unit: Unit) {
        if self.selection.is_empty() {
            let target = self.target(fonts, Direction::Back, unit);
            self.selection = Selection::new(self.selection.caret, target);
        }
        self.backspace();
    }

    pub fn toggle(&mut self, format: InlineFormat) {
        let change = doc::toggle_inline(&self.buffer.doc, self.selection, format);
        self.run(change);
    }

    pub fn heading(&mut self, level: u8) {
        let change = doc::set_heading(&self.buffer.doc, self.selection, level);
        self.run(change);
    }

    pub fn shift_heading(&mut self, more_prominent: bool) {
        let change = doc::shift_heading(&self.buffer.doc, self.selection, more_prominent);
        self.run(change);
    }

    pub fn undo(&mut self) {
        self.pending_tidy = None;
        match self.buffer.undo() {
            Ok(Some(sel)) => self.selection = sel,
            Ok(None) => {}
            Err(e) => eprintln!("journal write failed: {e}"),
        }
        self.touched();
    }

    pub fn redo(&mut self) {
        self.pending_tidy = None;
        match self.buffer.redo() {
            Ok(Some(sel)) => self.selection = sel,
            Ok(None) => {}
            Err(e) => eprintln!("journal write failed: {e}"),
        }
        self.touched();
    }

    pub fn selected_text(&self) -> Option<String> {
        let r = self.selection.range();
        (!r.is_empty()).then(|| self.buffer.doc.text()[r].to_owned())
    }

    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        let r = self.selection.range();
        self.run(Change { tx: Transaction::delete(r.clone()), selection: Selection::caret(r.start), typing: false });
        Some(text)
    }

    pub fn paste(&mut self, text: &str) {
        if self.in_table() {
            if !self.table_command(|d, s| doc::table_paste(d, s, text)) {
                self.type_text(text);
            }
            return;
        }
        let change = doc::paste(&self.buffer.doc, self.selection, text);
        self.run(change);
    }

    pub fn select_all(&mut self) {
        self.set_selection(Selection::new(0, self.buffer.doc.text().len()));
    }

    // ---- language picker -----------------------------------------------------------------------

    /// Opens the language picker if the caret is in a fenced code block.
    pub fn open_picker(&mut self) -> bool {
        let doc = &self.buffer.doc;
        let in_fence =
            doc.block_containing(self.selection.caret).and_then(|i| doc::code_language_range(doc, i)).is_some();
        if in_fence {
            self.picker = Some(Picker::default());
            self.activity = Instant::now();
        }
        in_fence
    }

    pub fn picker_input(&mut self, input: PickerInput) {
        let Some(p) = &mut self.picker else { return };
        let items = p.items(&self.recent_languages);
        match input {
            PickerInput::Up => p.selected = p.selected.saturating_sub(1),
            PickerInput::Down => p.selected = (p.selected + 1).min(items.len().saturating_sub(1)),
            PickerInput::Backspace => {
                p.query.pop();
                p.selected = 0;
            }
            PickerInput::Text(t) => {
                p.query.push_str(&t);
                p.selected = 0;
            }
            PickerInput::Cancel => self.picker = None,
            PickerInput::Accept => match items.get(p.selected) {
                Some(choice) if !choice.is_empty() => self.choose_language(&choice.clone()),
                _ => self.picker = None,
            },
        }
        self.activity = Instant::now();
    }

    /// Sets the language of the code block at the caret and closes the picker.
    pub fn choose_language(&mut self, choice: &str) {
        self.picker = None;
        let lang = picker::language_for(choice);
        picker::remember(&mut self.recent_languages, lang);
        let change = doc::set_code_language(&self.buffer.doc, self.selection, lang);
        self.run(change);
    }

    // ---- movement ----------------------------------------------------------------------------

    pub fn set_selection(&mut self, sel: Selection) {
        let sel = self.tidy_before_moving(sel);
        let text = self.buffer.doc.text();
        let clamp = |mut o: usize| {
            o = o.min(text.len());
            while !text.is_char_boundary(o) {
                o -= 1;
            }
            o
        };
        let sel = Selection::new(clamp(sel.anchor), clamp(sel.caret));
        if sel != self.selection {
            self.picker = None;
            self.table_focus = None;
            self.selection = sel;
            self.buffer.history.break_group();
            self.touched();
        }
    }

    /// Moves the caret, or extends the selection when `extend` is set.
    pub fn motion(&mut self, fonts: &mut Fonts, dir: Direction, unit: Unit, extend: bool) {
        let sel = self.selection;
        let vertical = unit == Unit::Line && matches!(dir, Direction::Up | Direction::Down);
        // Collapsing a selection with ←/→ lands on its edge rather than moving past it.
        let target = if !extend && !sel.is_empty() && unit == Unit::Char {
            match dir {
                Direction::Back | Direction::Up => sel.range().start,
                _ => sel.range().end,
            }
        } else {
            self.target(fonts, dir, unit)
        };
        let goal = self.goal_x;
        let anchor = if extend { sel.anchor } else { target };
        self.set_selection(Selection::new(anchor, target));
        // Vertical moves keep their column; any other move starts a new one.
        self.goal_x = if vertical { goal.or(self.goal_x) } else { None };
    }

    fn target(&mut self, fonts: &mut Fonts, dir: Direction, unit: Unit) -> usize {
        let caret = self.selection.caret;
        let len = self.buffer.doc.text().len();
        if unit == Unit::Document {
            return if matches!(dir, Direction::Back | Direction::Up) { 0 } else { len };
        }
        if unit == Unit::Line && matches!(dir, Direction::Up | Direction::Down) {
            return self.vertical_target(fonts, dir == Direction::Down);
        }
        let view = self.view(fonts);
        let Some(i) = view.blocks.iter().position(|b| {
            let r = b.source_range();
            r.start <= caret && caret <= r.end
        }) else {
            return caret;
        };
        let b = &view.blocks[i];
        let idx = b.display_index(caret);
        let cursor = Cursor::from_byte_index(&b.layout, idx, Affinity::Downstream);
        let forward = matches!(dir, Direction::Forward | Direction::Down);
        let moved = match unit {
            Unit::Char if forward => cursor.next_visual(&b.layout),
            Unit::Char => cursor.previous_visual(&b.layout),
            Unit::Word if forward => cursor.next_visual_word(&b.layout),
            Unit::Word => cursor.previous_visual_word(&b.layout),
            Unit::Line => {
                let sel = parley::editing::Selection::from(cursor);
                if forward { sel.line_end(&b.layout, false).focus() } else { sel.line_start(&b.layout, false).focus() }
            }
            Unit::Document => unreachable!(),
        };
        if moved.index() != idx || unit == Unit::Line {
            return b.source_offset(moved.index(), moved.affinity());
        }
        // At a block edge: step into the neighbor.
        let neighbor = if forward { view.blocks.get(i + 1) } else { i.checked_sub(1).and_then(|j| view.blocks.get(j)) };
        match neighbor {
            Some(n) if forward => n.source_offset(0, Affinity::Downstream),
            Some(n) => n.source_offset(usize::MAX / 2, Affinity::Upstream).min(n.source_range().end),
            None => caret,
        }
    }

    fn vertical_target(&mut self, fonts: &mut Fonts, down: bool) -> usize {
        let caret = self.selection.caret;
        let goal = self.goal_x;
        // Past the first or last line, go to the document's start or end, as on the Mac.
        let edge = if down { self.buffer.doc.text().len() } else { 0 };
        let view = self.view(fonts);
        let Some(c) = view.caret_box(caret) else { return caret };
        let x = goal.unwrap_or(c.x);
        let target = view.vertical_neighbor(caret, x, down).unwrap_or(edge);
        self.goal_x = Some(x);
        target
    }

    pub fn mouse_down(&mut self, fonts: &mut Fonts, x: f32, y: f32, extend: bool, clicks: u8) {
        let scroll = self.scroll;
        let point = (x as f64, (y + scroll) as f64);
        if let Some(p) = &self.picker {
            if let Some((_, choice)) = p.rows.iter().find(|(r, _)| r.contains(point)) {
                let choice = choice.clone();
                self.choose_language(&choice);
                return;
            }
            if p.frame.is_some_and(|f| f.contains(point)) {
                return;
            }
            self.picker = None;
        } else if self.view(fonts).chip_at(point.0 as f32, point.1 as f32) && self.open_picker() {
            return;
        }
        if let Some(g) = &self.goto {
            if let Some(&(_, offset)) = g.rows.iter().find(|(r, _)| r.contains(point)) {
                self.goto = None;
                self.jump_to(offset);
                return;
            }
            if g.frame.is_some_and(|f| f.contains(point)) {
                return;
            }
            self.goto = None;
        }
        if let Some(menu) = &self.menu {
            if let Some(&(_, item)) = menu.rows.iter().find(|(r, _)| r.contains(point)) {
                self.choose_menu(item);
                return;
            }
            if menu.frame.is_some_and(|f| f.contains(point)) {
                return;
            }
            self.menu = None;
        }
        if let Some(h) = self.handle_at(fonts, point.0 as f32, point.1 as f32) {
            self.click_handle(h);
            return;
        }
        self.table_focus = None;
        let Some(at) = self.view(fonts).hit_test(x, y + scroll) else { return };
        let sel = match clicks {
            2 => self.word_selection(at),
            3.. => self.block_selection(at),
            _ if extend => Selection::new(self.selection.anchor, at),
            _ => Selection::caret(at),
        };
        self.drag = Some((sel, clicks));
        self.set_selection(sel);
    }

    pub fn mouse_drag(&mut self, fonts: &mut Fonts, x: f32, y: f32) {
        let Some((start, clicks)) = self.drag else { return };
        let scroll = self.scroll;
        let Some(at) = self.view(fonts).hit_test(x, y + scroll) else { return };
        let unit = match clicks {
            2 => self.word_selection(at),
            3.. => self.block_selection(at),
            _ => Selection::caret(at),
        };
        // Extend by whole units in the drag direction.
        let (lo, hi) = (start.range(), unit.range());
        let sel = if hi.start < lo.start {
            Selection::new(lo.end, hi.start)
        } else {
            Selection::new(lo.start, hi.end.max(lo.end))
        };
        self.set_selection(sel);
    }

    pub fn mouse_up(&mut self, fonts: &mut Fonts) {
        self.drag = None;
        if self.grip_drag.is_some() {
            self.drop_grip(fonts);
        }
    }

    fn word_selection(&self, at: usize) -> Selection {
        use unicode_segmentation::UnicodeSegmentation;
        let text = self.buffer.doc.text();
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        text[line_start..line_end]
            .split_word_bound_indices()
            .map(|(i, w)| line_start + i..line_start + i + w.len())
            .find(|r| r.start <= at && at < r.end.max(r.start + 1))
            .map_or(Selection::caret(at), |r| Selection::new(r.start, r.end))
    }

    fn block_selection(&self, at: usize) -> Selection {
        let text = self.buffer.doc.text();
        match self.buffer.doc.block_containing(at) {
            Some(i) => {
                let b = &self.buffer.doc.blocks()[i];
                Selection::new(b.content_start(text), b.content_end(text))
            }
            None => Selection::caret(at),
        }
    }

    pub fn scroll_by(&mut self, fonts: &mut Fonts, dy: f32) {
        let max = (self.view(fonts).height - self.height).max(0.0);
        let next = (self.scroll + dy).clamp(0.0, max);
        if next != self.scroll {
            self.scroll = next;
            self.session_dirty = true;
        }
    }

    /// Scrolls just enough to keep the caret on screen.
    pub fn reveal_caret(&mut self, fonts: &mut Fonts) {
        let caret = self.selection.caret;
        let Some(c) = self.view(fonts).caret_box(caret) else { return };
        let margin = 48.0;
        if c.y - margin < self.scroll {
            self.scroll = (c.y - margin).max(0.0);
        } else if c.y + c.height + margin > self.scroll + self.height {
            self.scroll = c.y + c.height + margin - self.height;
        }
    }

    pub fn title(&self) -> String {
        let name = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        if self.buffer.is_dirty() { format!("{name} — Edited") } else { name }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Char,
    Word,
    /// A visual line (wrapped), or a line up/down for vertical directions.
    Line,
    Document,
}

/// Draws table grips (dots), insertion points (⊕), the grip-selected row or column, and where a
/// dragged grip would drop.
#[allow(clippy::too_many_arguments)]
fn paint_handles(
    scene: &mut Scene,
    origin: Affine,
    handles: &[Handle],
    grids: &[handles::Grid],
    focus: Option<(usize, HandleKind)>,
    dragging: Option<((usize, HandleKind), (f32, f32))>,
    muted: Color,
    accent: Color,
    highlight: Color,
) {
    use vello::kurbo::Circle;
    if let Some((table, kind)) = focus
        && let Some(g) = grids.iter().find(|g| g.table == table)
    {
        let rect = match kind {
            HandleKind::Column(c) if c + 1 < g.xs.len() => {
                Some(Rect::new(g.xs[c] as f64, g.ys[0] as f64, g.xs[c + 1] as f64, *g.ys.last().unwrap() as f64))
            }
            HandleKind::Row(r) if r + 1 < g.ys.len() => {
                Some(Rect::new(g.xs[0] as f64, g.ys[r] as f64, *g.xs.last().unwrap() as f64, g.ys[r + 1] as f64))
            }
            _ => None,
        };
        if let Some(rect) = rect {
            scene.fill(Fill::NonZero, origin, color(highlight), None, &rect);
        }
    }
    if let Some(((table, kind), (x, y))) = dragging
        && let Some(g) = grids.iter().find(|g| g.table == table)
    {
        let line = match kind {
            HandleKind::Column(c) => {
                let to = g.column_at(x);
                let bx = if to > c { g.xs[to + 1] } else { g.xs[to] };
                Line::new((bx as f64, g.ys[0] as f64), (bx as f64, *g.ys.last().unwrap() as f64))
            }
            _ => {
                let (r, to) = (
                    match kind {
                        HandleKind::Row(r) => r,
                        _ => 0,
                    },
                    g.row_at(y),
                );
                let by = if to > r { g.ys[to + 1] } else { g.ys[to] };
                Line::new((g.xs[0] as f64, by as f64), (*g.xs.last().unwrap() as f64, by as f64))
            }
        };
        scene.stroke(&Stroke::new(2.0), origin, color(accent), None, &line);
    }
    for h in handles {
        let (x, y) = (h.x as f64, h.y as f64);
        match h.kind {
            HandleKind::AddColumn(_) | HandleKind::AddRow(_) => {
                scene.fill(Fill::NonZero, origin, color(accent.with_alpha(h.alpha)), None, &Circle::new((x, y), 7.0));
                let ink = color(Color::WHITE.with_alpha(h.alpha));
                scene.stroke(&Stroke::new(1.6), origin, ink, None, &Line::new((x - 3.5, y), (x + 3.5, y)));
                scene.stroke(&Stroke::new(1.6), origin, ink, None, &Line::new((x, y - 3.5), (x, y + 3.5)));
            }
            HandleKind::Column(_) | HandleKind::Row(_) => {
                let focused = focus == Some((h.table, h.kind));
                let c = color(if focused { accent } else { muted.with_alpha(h.alpha) });
                let horizontal = matches!(h.kind, HandleKind::Column(_));
                for i in -1..=1 {
                    let d = i as f64 * 4.5;
                    let center = if horizontal { (x + d, y) } else { (x, y + d) };
                    scene.fill(Fill::NonZero, origin, c, None, &Circle::new(center, 1.7));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marginal_session::Store;

    struct Harness {
        editor: Editor,
        fonts: Fonts,
        /// The caret mark: `|`, or `‸` in sources that contain pipes.
        mark: char,
        _dir: tempfile::TempDir,
    }

    /// An editor over `src`, with the caret at the `|` mark.
    fn harness(src: &str) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path()).unwrap();
        let mark = if src.contains('‸') { '‸' } else { '|' };
        let caret = src.find(mark).expect("mark the caret with | or ‸");
        let buffer = store.create_buffer(None, &src.replacen(mark, "", 1)).unwrap();
        let mut editor = Editor::new(buffer, Selection::caret(caret), Theme::terminal(), Appearance::Dark);
        editor.selection = Selection::caret(caret);
        Harness { editor, fonts: Fonts::new(), mark, _dir: dir }
    }

    impl Harness {
        fn text(&self) -> String {
            let mut t = self.editor.buffer.doc.text().to_owned();
            t.insert(self.editor.selection.caret, self.mark);
            t
        }
        fn keys(&mut self, s: &str) {
            for c in s.chars() {
                match c {
                    '\n' => self.editor.newline(),
                    '\u{8}' => self.editor.backspace(),
                    c => self.editor.type_text(&c.to_string()),
                }
            }
        }
        fn right(&mut self, n: usize) {
            for _ in 0..n {
                self.editor.motion(&mut self.fonts, Direction::Forward, Unit::Char, false);
            }
        }
    }

    #[test]
    fn markdown_autoformats_as_you_type() {
        let mut h = harness("intro\n\n|");
        h.keys("## Plan\n- one\ntwo\n\n");
        assert_eq!(h.text(), "intro\n\n## Plan\n\n- one\n- two\n|");
        let kinds: Vec<_> = h.editor.buffer.doc.blocks().iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds[1], marginal_doc::BlockKind::Heading { level: 2, setext: false });
    }

    #[test]
    fn typing_undoes_as_one_step() {
        let mut h = harness("a|");
        h.keys("bcd");
        h.editor.undo();
        assert_eq!(h.text(), "a|");
        h.editor.redo();
        assert_eq!(h.text(), "abcd|");
    }

    #[test]
    fn arrows_step_through_revealed_syntax() {
        let mut h = harness("x| **b** y");
        let mut seen = Vec::new();
        for _ in 0..9 {
            h.right(1);
            seen.push(h.editor.selection.caret);
        }
        // Every byte of `**b**` is reachable once it's revealed, so edits can land anywhere;
        // the last move stops at the end of the text.
        assert_eq!(seen, vec![2, 3, 4, 5, 6, 7, 8, 9, 9]);
    }

    #[test]
    fn caret_between_blocks_gets_its_own_line() {
        let mut h = harness("para|\n");
        h.editor.newline();
        let caret = h.editor.selection.caret;
        let view = h.editor.view(&mut h.fonts);
        let para_bottom = view.blocks[0].bottom;
        let c = view.caret_box(caret).unwrap();
        assert!(c.y >= para_bottom, "caret ({}) should sit below the paragraph ({para_bottom})", c.y);
    }

    #[test]
    fn preedit_shows_inline_without_editing_the_document() {
        let mut h = harness("ab|c");
        h.editor.preedit = Some("にほ".into());
        h.editor.invalidate();
        let view = h.editor.view(&mut h.fonts);
        assert!(view.caret_box(2).is_some());
        assert_eq!(h.editor.buffer.doc.text(), "abc");
        h.editor.type_text("日本");
        assert_eq!(h.text(), "ab日本|c");
        assert!(h.editor.preedit.is_none());
    }

    #[test]
    fn backspace_demotes_a_heading_then_joins() {
        let mut h = harness("a\n\n## |b\n");
        h.keys("\u{8}");
        assert_eq!(h.text(), "a\n\n|b\n");
        h.keys("\u{8}");
        assert_eq!(h.text(), "a|b\n");
    }

    #[test]
    fn unused_empty_lines_are_tidied_with_the_enter() {
        let mut h = harness("one|\n\ntwo\n");
        h.keys("\n");
        assert_eq!(h.text(), "one\n\n|\n\ntwo\n");
        h.editor.motion(&mut h.fonts, Direction::Down, Unit::Document, false);
        assert_eq!(h.editor.buffer.doc.text(), "one\n\ntwo\n", "the untouched line is removed");
        h.editor.undo();
        assert_eq!(h.editor.buffer.doc.text(), "one\n\ntwo\n", "and it undoes with the Enter as one step");
        assert!(!h.editor.buffer.history.can_undo());
    }

    #[test]
    fn typed_lines_and_existing_blank_lines_are_kept() {
        let mut h = harness("one|\n");
        h.keys("\nx");
        h.editor.motion(&mut h.fonts, Direction::Up, Unit::Document, false);
        assert_eq!(h.editor.buffer.doc.text(), "one\n\nx\n");
        let mut h = harness("one\n\n\n|\n\ntwo\n");
        h.editor.motion(&mut h.fonts, Direction::Up, Unit::Document, false);
        assert_eq!(h.editor.buffer.doc.text(), "one\n\n\n\n\ntwo\n", "blank lines already in the file stay");
    }

    #[test]
    fn empty_list_item_left_behind_is_tidied() {
        let mut h = harness("- a|\n");
        h.keys("\n");
        assert_eq!(h.text(), "- a\n- |\n");
        h.editor.motion(&mut h.fonts, Direction::Up, Unit::Document, false);
        assert_eq!(h.editor.buffer.doc.text(), "- a\n");
    }

    #[test]
    fn up_arrow_crosses_block_margins() {
        let src = "# Title\n\nabc\n\n```rust\nfn a\nfn b\n```\n\n- one\n- two|\n";
        let mut h = harness(src);
        let mut lines = Vec::new();
        for _ in 0..8 {
            h.editor.motion(&mut h.fonts, Direction::Up, Unit::Line, false);
            let text = h.editor.buffer.doc.text();
            let c = h.editor.selection.caret;
            lines.push(text[..c].matches('\n').count());
        }
        assert_eq!(lines, [9, 6, 5, 2, 0, 0, 0, 0], "source line of the caret after each ↑");
        assert_eq!(h.editor.selection.caret, 0, "↑ on the first line goes to the start");
        let mut down = Vec::new();
        for _ in 0..6 {
            h.editor.motion(&mut h.fonts, Direction::Down, Unit::Line, false);
            let c = h.editor.selection.caret;
            down.push(h.editor.buffer.doc.text()[..c].matches('\n').count());
        }
        assert_eq!(down, [2, 5, 6, 9, 10, 11]);
    }

    #[test]
    fn language_picker_rewrites_the_fence() {
        let mut h = harness("```rust\nfn |a\n```\n");
        assert!(h.editor.open_picker());
        h.editor.picker_input(PickerInput::Text("py".into()));
        h.editor.picker_input(PickerInput::Accept);
        assert!(h.editor.picker.is_none());
        assert_eq!(h.text(), "```python\nfn |a\n```\n");
        assert_eq!(h.editor.recent_languages, ["python"]);
        h.editor.undo();
        assert_eq!(h.editor.buffer.doc.text(), "```rust\nfn a\n```\n", "one undo step");

        let mut h = harness("plain |text\n");
        assert!(!h.editor.open_picker(), "only in fenced code");
    }

    #[test]
    fn typing_in_an_aligned_table_keeps_it_aligned() {
        let mut h = harness("| a‸ | b |\n|---|---|\n| c | d |\n");
        h.keys("bc|");
        assert_eq!(h.text(), "| abc\\|‸ | b |\n|-------|---|\n| c     | d |\n");
        h.editor.undo();
        assert_eq!(h.editor.buffer.doc.text(), "| a | b |\n|---|---|\n| c | d |\n", "typing and padding undo together");
    }

    #[test]
    fn arrows_and_keys_move_between_cells() {
        let mut h = harness("intro\n\n| a‸ | bb |\n|---|----|\n| c | d  |\n\nend\n");
        h.editor.motion(&mut h.fonts, Direction::Down, Unit::Line, false);
        assert_eq!(h.text(), "intro\n\n| a | bb |\n|---|----|\n| c‸ | d  |\n\nend\n", "↓ stays in the column");
        h.right(1);
        assert_eq!(h.text(), "intro\n\n| a | bb |\n|---|----|\n| c | ‸d  |\n\nend\n", "→ crosses into the next cell");
        h.editor.motion(&mut h.fonts, Direction::Up, Unit::Line, false);
        assert!(h.text().contains("| ‸bb |"), "↑ goes to the header of the same column: {}", h.text());
        h.editor.motion(&mut h.fonts, Direction::Up, Unit::Line, false);
        assert!(h.text().starts_with("‸intro") || h.text().starts_with("intro‸"), "↑ leaves the table: {}", h.text());
        let mut h = harness("| a‸ | b |\n|---|---|\n");
        h.editor.newline();
        assert_eq!(h.text(), "| a | b |\n|---|---|\n| ‸  |   |\n", "Return adds a row below");
    }

    /// The handles of the first table, with the pointer at `at` (document coordinates).
    fn table_handles(h: &mut Harness, at: (f32, f32)) -> (Vec<Handle>, handles::Grid) {
        h.editor.pointer_moved(&mut h.fonts, Some(at));
        let caret_table = h.editor.caret_table();
        let view = h.editor.view(&mut h.fonts);
        (handles::handles(view, Some(at), caret_table), handles::grids(view).remove(0))
    }

    fn click(h: &mut Harness, x: f32, y: f32) {
        h.editor.pointer_moved(&mut h.fonts, Some((x, y)));
        h.editor.mouse_down(&mut h.fonts, x, y, false, 1);
        h.editor.mouse_up(&mut h.fonts);
    }

    #[test]
    fn border_handles_insert_rows_and_columns() {
        let mut h = harness("| a‸ | b |\n|---|---|\n| c | d |\n");
        let grid = table_handles(&mut h, (0.0, 0.0)).1;
        // Pointer on the border between the columns offers a ⊕ above it.
        let (handles, _) = table_handles(&mut h, (grid.xs[1], grid.ys[1] + 4.0));
        let add = handles.iter().find(|x| x.kind == HandleKind::AddColumn(1)).expect("⊕ on the column border");
        click(&mut h, add.x, add.y);
        assert_eq!(h.editor.buffer.doc.text(), "| a |   | b |\n|---|---|---|\n| c |   | d |\n");
        h.editor.undo();
        let (handles, _) = table_handles(&mut h, (grid.xs[0] + 4.0, grid.ys[2]));
        let add = handles.iter().find(|x| x.kind == HandleKind::AddRow(2)).expect("⊕ on the bottom border");
        click(&mut h, add.x, add.y);
        assert_eq!(h.editor.buffer.doc.text(), "| a | b |\n|---|---|\n| c | d |\n|   |   |\n");
    }

    #[test]
    fn grips_select_drag_and_delete() {
        let mut h = harness("| a‸ | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n");
        let (handles, grid) = table_handles(&mut h, (0.0, 0.0));
        let grip = handles.iter().find(|x| x.kind == HandleKind::Column(0)).unwrap();
        // Drag column a onto column c.
        h.editor.pointer_moved(&mut h.fonts, Some((grip.x, grip.y)));
        h.editor.mouse_down(&mut h.fonts, grip.x, grip.y, false, 1);
        h.editor.pointer_moved(&mut h.fonts, Some((grid.xs[3] - 2.0, grid.ys[1])));
        h.editor.mouse_up(&mut h.fonts);
        assert_eq!(h.editor.buffer.doc.text(), "| b | c | a |\n|---|---|---|\n| 2 | 3 | 1 |\n| 5 | 6 | 4 |\n");
        h.editor.undo();
        assert_eq!(
            h.editor.buffer.doc.text(),
            "| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n",
            "one undo"
        );
        // Select row 1 by its grip, then ⌫ deletes it.
        let (handles, _) = table_handles(&mut h, (0.0, 0.0));
        let grip = handles.iter().find(|x| x.kind == HandleKind::Row(1)).unwrap();
        click(&mut h, grip.x, grip.y);
        h.editor.backspace();
        assert_eq!(h.editor.buffer.doc.text(), "| a | b | c |\n|---|---|---|\n| 4 | 5 | 6 |\n");
    }

    #[test]
    fn grip_menu_aligns_inserts_and_deletes() {
        let mut h = harness("| a‸ | b |\n|---|---|\n| c | d |\n");
        assert!(h.editor.open_grip_menu(&mut h.fonts, 1));
        h.editor.menu_input(PickerInput::Down);
        h.editor.menu_input(PickerInput::Accept);
        assert_eq!(h.editor.buffer.doc.text(), "| a | b |\n|---|:-:|\n| c | d |\n", "align center");
        assert!(h.editor.menu.is_none());
        assert!(h.editor.open_grip_menu(&mut h.fonts, 0));
        for _ in 0..6 {
            h.editor.menu_input(PickerInput::Down);
        }
        h.editor.menu_input(PickerInput::Accept);
        assert_eq!(h.editor.buffer.doc.text(), "| b |\n|:-:|\n| d |\n", "delete column");
    }

    #[test]
    fn vertical_motion_keeps_its_column() {
        let mut h = harness("abcdef|\n\nab\n\nabcdef\n");
        h.editor.motion(&mut h.fonts, Direction::Down, Unit::Line, false);
        assert_eq!(h.editor.selection.caret, 10, "clamped to the short line's end");
        h.editor.motion(&mut h.fonts, Direction::Down, Unit::Line, false);
        assert_eq!(h.editor.selection.caret, 18, "returns to the original column");
    }
}
